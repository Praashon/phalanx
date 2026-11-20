use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use phalanx_lang::{OptLevel, SourceMap, compile};
use phalanx_sim::{
    ArenaConfig, CombatEvent, Simulation, encode_replay, record_match, verify_replay,
};
use phalanx_tourney::{BotEntry, TournamentConfig, TournamentFormat, run_tournament};
use phalanx_vm::disasm::{disassemble, format_instruction};
use phalanx_vm::{ExecStatus, VerifiedProgram, encode_program};
use serde::{Deserialize, Serialize};

use crate::bots::{BUILTIN_BOTS, ROGUE_LOOP_SRC};

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");
const MAX_HTTP_BYTES: usize = 2 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Handle to a running background HTTP studio server (supports clean shutdown in tests).
pub struct ServerHandle {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// Bound socket address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Signals the accept loop to stop and joins the background server thread.
    pub fn shutdown(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// Spawns the Phalanx Studio HTTP server on `addr` (supports `"127.0.0.1:0"` for tests).
pub fn spawn_server_on(addr: &str) -> io::Result<ServerHandle> {
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);

    let join = thread::spawn(move || {
        for stream_res in listener.incoming() {
            if flag.load(Ordering::Acquire) {
                break;
            }
            if let Ok(stream) = stream_res {
                if flag.load(Ordering::Acquire) {
                    break;
                }
                thread::spawn(move || {
                    let _ = handle_connection(stream);
                });
            }
        }
    });

    Ok(ServerHandle {
        addr: local_addr,
        shutdown,
        join: Some(join),
    })
}

fn bind_with_fallback(host: &str, requested_port: u16) -> io::Result<TcpListener> {
    if requested_port == 0 {
        return TcpListener::bind((host, 0));
    }
    match TcpListener::bind((host, requested_port)) {
        Ok(l) => Ok(l),
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            for offset in 1..=20u16 {
                if let Some(p) = requested_port.checked_add(offset) {
                    match TcpListener::bind((host, p)) {
                        Ok(l) => return Ok(l),
                        Err(e) if e.kind() == io::ErrorKind::AddrInUse => continue,
                        Err(e) => return Err(e),
                    }
                }
            }
            TcpListener::bind((host, 0))
        }
        Err(err) => Err(err),
    }
}

/// Starts the blocking Phalanx Web Studio HTTP server at `http://{host}:{port}`.
pub fn start_server(host: &str, port: u16) -> Result<(), String> {
    let listener =
        bind_with_fallback(host, port).map_err(|e| format!("failed to bind {host}:{port}: {e}"))?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("failed to inspect socket address: {e}"))?;

    println!("phalanx studio listening on http://{bound}");
    if port != 0 && bound.port() != port {
        println!("note: requested port {port} was busy; bound fallback port");
    }
    println!("press Ctrl+C to stop");

    for stream_res in listener.incoming() {
        match stream_res {
            Ok(stream) => {
                thread::spawn(move || {
                    let _ = handle_connection(stream);
                });
            }
            Err(err) => {
                eprintln!("accept error: {err}");
            }
        }
    }
    Ok(())
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn handle_connection(mut stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;

    let req = match read_http_request(&mut stream) {
        Ok(r) => r,
        Err((status, msg)) => {
            return write_json_error(&mut stream, status, &msg);
        }
    };

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/" | "/index.html") => write_http_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            INDEX_HTML.as_bytes(),
        ),
        ("GET", "/app.css") => write_http_response(
            &mut stream,
            "200 OK",
            "text/css; charset=utf-8",
            APP_CSS.as_bytes(),
        ),
        ("GET", "/app.js") => write_http_response(
            &mut stream,
            "200 OK",
            "application/javascript; charset=utf-8",
            APP_JS.as_bytes(),
        ),
        ("GET", "/api/bots") => {
            let bots = builtin_bot_dtos();
            write_json(&mut stream, "200 OK", &bots)
        }
        ("POST", "/api/compile") => {
            let parsed: CompileReq = match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => {
                    return write_json_error(
                        &mut stream,
                        "400 Bad Request",
                        &format!("invalid JSON: {e}"),
                    );
                }
            };
            let resp = handle_compile(parsed);
            write_json(&mut stream, "200 OK", &resp)
        }
        ("POST", "/api/match") => {
            let parsed: MatchReq = match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => {
                    return write_json_error(
                        &mut stream,
                        "400 Bad Request",
                        &format!("invalid JSON: {e}"),
                    );
                }
            };
            let resp = handle_match(parsed);
            write_json(&mut stream, "200 OK", &resp)
        }
        ("POST", "/api/trace-tick") => {
            let parsed: TraceTickReq = match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => {
                    return write_json_error(
                        &mut stream,
                        "400 Bad Request",
                        &format!("invalid JSON: {e}"),
                    );
                }
            };
            let resp = handle_trace_tick(parsed);
            write_json(&mut stream, "200 OK", &resp)
        }
        ("POST", "/api/tourney") => {
            let parsed: TourneyReq = match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => {
                    return write_json_error(
                        &mut stream,
                        "400 Bad Request",
                        &format!("invalid JSON: {e}"),
                    );
                }
            };
            let resp = handle_tourney(parsed);
            write_json(&mut stream, "200 OK", &resp)
        }
        ("OPTIONS", _) => write_http_response(&mut stream, "204 No Content", "text/plain", &[]),
        _ => write_json_error(&mut stream, "404 Not Found", "endpoint not found"),
    }
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, (&'static str, String)> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];

    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() >= MAX_HEADER_BYTES {
            return Err((
                "431 Request Header Fields Too Large",
                "headers too large".into(),
            ));
        }
        let n = stream
            .read(&mut chunk)
            .map_err(|e| ("400 Bad Request", format!("read error: {e}")))?;
        if n == 0 {
            return Err(("400 Bad Request", "unexpected EOF before headers".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let header_bytes = &buf[..header_end];
    let header_str = std::str::from_utf8(header_bytes)
        .map_err(|_| ("400 Bad Request", "non-UTF-8 HTTP headers".into()))?;
    let mut lines = header_str.split("\r\n");
    let request_line = lines
        .next()
        .ok_or(("400 Bad Request", "missing request line".into()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or(("400 Bad Request", "missing method".into()))?
        .to_ascii_uppercase();
    let raw_target = parts
        .next()
        .ok_or(("400 Bad Request", "missing request path".into()))?;
    let path = raw_target
        .split('?')
        .next()
        .unwrap_or(raw_target)
        .to_string();

    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = v
                .trim()
                .parse::<usize>()
                .map_err(|_| ("400 Bad Request", "invalid Content-Length".into()))?;
        }
    }
    if content_length > MAX_HTTP_BYTES {
        return Err(("413 Payload Too Large", "request body exceeds 2 MiB".into()));
    }

    let body_start = header_end + 4;
    let mut body = buf[body_start..].to_vec();
    while body.len() < content_length {
        let needed = content_length - body.len();
        let to_read = needed.min(chunk.len());
        let n = stream
            .read(&mut chunk[..to_read])
            .map_err(|e| ("400 Bad Request", format!("body read error: {e}")))?;
        if n == 0 {
            return Err(("400 Bad Request", "truncated request body".into()));
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Ok(HttpRequest { method, path, body })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn write_json<T: Serialize>(stream: &mut TcpStream, status: &str, value: &T) -> io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    write_http_response(stream, status, "application/json; charset=utf-8", &body)
}

fn write_json_error(stream: &mut TcpStream, status: &str, msg: &str) -> io::Result<()> {
    #[derive(Serialize)]
    struct ErrBody<'a> {
        ok: bool,
        error: &'a str,
    }
    write_json(
        stream,
        status,
        &ErrBody {
            ok: false,
            error: msg,
        },
    )
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn bot_meta_for(id: &str) -> (&'static str, &'static str) {
    match id.to_ascii_lowercase().as_str() {
        "kiter" => ("Predictive Lead-Aimer & Dash Kiter", "#22d3ee"),
        "berserker" => ("Aggressive Heavy-Cannon Brawler", "#fb7185"),
        "sentry" => ("Center-Control LOS Cone Turret", "#34d399"),
        "vanguard" => ("4-State Tactical FSM", "#fbbf24"),
        "ghost" => ("Memory-Array Flanker & Dodger", "#a78bfa"),
        "rogue_loop" => ("Gas-Limit Containment Demo (while true)", "#94a3b8"),
        _ => ("Custom .phx Autonomous Bot", "#38bdf8"),
    }
}

const PALETTE: [&str; 8] = [
    "#22d3ee", "#fb7185", "#34d399", "#fbbf24", "#a78bfa", "#60a5fa", "#f472b6", "#f8fafc",
];

#[derive(Serialize)]
struct BotTemplateDto {
    id: &'static str,
    name: &'static str,
    role: &'static str,
    color: &'static str,
    source: &'static str,
    builtin_fighter: bool,
}

fn builtin_bot_dtos() -> Vec<BotTemplateDto> {
    let mut out: Vec<BotTemplateDto> = BUILTIN_BOTS
        .iter()
        .map(|&(name, src)| {
            let (role, color) = bot_meta_for(name);
            BotTemplateDto {
                id: name,
                name,
                role,
                color,
                source: src,
                builtin_fighter: true,
            }
        })
        .collect();
    let (role, color) = bot_meta_for("rogue_loop");
    out.push(BotTemplateDto {
        id: "rogue_loop",
        name: "rogue_loop",
        role,
        color,
        source: ROGUE_LOOP_SRC,
        builtin_fighter: false,
    });
    out
}

#[derive(Deserialize)]
struct CompileReq {
    name: String,
    source: String,
    opt: Option<bool>,
}

#[derive(Serialize)]
struct DiagnosticDto {
    line: u32,
    col: u32,
    message: String,
    help: Option<String>,
    rendered: String,
}

#[derive(Serialize)]
struct VerifyReportDto {
    program_name: String,
    fingerprint: String,
    functions: usize,
    instructions: usize,
    basic_blocks: usize,
    back_edges: usize,
    max_call_depth: usize,
    state_slots: usize,
    state_arrays: usize,
}

#[derive(Serialize)]
struct OptStatsDto {
    consts_folded: usize,
    algebraic_simplifications: usize,
    copies_propagated: usize,
    dead_insts_removed: usize,
    dead_blocks_removed: usize,
}

#[derive(Serialize)]
struct CompileResp {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<VerifyReportDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opt_stats: Option<OptStatsDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ir_dump: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disasm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytecode_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<Vec<DiagnosticDto>>,
}

fn compile_source_or_diags(
    name: &str,
    source: &str,
    opt: OptLevel,
) -> Result<phalanx_lang::CompileOutput, Vec<DiagnosticDto>> {
    compile(name, source, opt).map_err(|diags| {
        let sm = SourceMap::new(source);
        let file_lbl = format!("{name}.phx");
        diags
            .into_iter()
            .map(|d| {
                let (line, col) = sm.line_col(d.span.start);
                let rendered = sm.render_diagnostic(&file_lbl, &d);
                DiagnosticDto {
                    line,
                    col,
                    message: d.message,
                    help: d.help,
                    rendered,
                }
            })
            .collect()
    })
}

fn handle_compile(req: CompileReq) -> CompileResp {
    let opt = if req.opt.unwrap_or(true) {
        OptLevel::Standard
    } else {
        OptLevel::None
    };
    let clean_name = if req.name.trim().is_empty() {
        "custom_bot".to_string()
    } else {
        req.name.trim().to_string()
    };

    match compile_source_or_diags(&clean_name, &req.source, opt) {
        Ok(out) => {
            let disasm = disassemble(out.verified.program());
            let bytecode_bytes = encode_program(out.verified.program()).map_or(0, |b| b.len());
            CompileResp {
                ok: true,
                report: Some(VerifyReportDto {
                    program_name: out.report.program_name,
                    fingerprint: format!("0x{:016x}", out.report.fingerprint),
                    functions: out.report.functions,
                    instructions: out.report.instructions,
                    basic_blocks: out.report.basic_blocks,
                    back_edges: out.report.back_edges,
                    max_call_depth: out.report.max_call_depth,
                    state_slots: out.report.state_slots,
                    state_arrays: out.report.state_arrays,
                }),
                opt_stats: Some(OptStatsDto {
                    consts_folded: out.opt_stats.consts_folded,
                    algebraic_simplifications: out.opt_stats.algebraic_simplifications,
                    copies_propagated: out.opt_stats.copies_propagated,
                    dead_insts_removed: out.opt_stats.dead_insts_removed,
                    dead_blocks_removed: out.opt_stats.dead_blocks_removed,
                }),
                ir_dump: Some(out.ir_dump),
                disasm: Some(disasm),
                bytecode_bytes: Some(bytecode_bytes),
                diagnostics: None,
            }
        }
        Err(diagnostics) => CompileResp {
            ok: false,
            report: None,
            opt_stats: None,
            ir_dump: None,
            disasm: None,
            bytecode_bytes: None,
            diagnostics: Some(diagnostics),
        },
    }
}

#[derive(Deserialize)]
struct BotSourceInput {
    name: String,
    source: String,
}

type BotCompileResult = Result<Vec<(String, VerifiedProgram)>, (String, Vec<DiagnosticDto>)>;

fn resolve_bot_inputs(input: Vec<BotSourceInput>) -> BotCompileResult {
    let list = if input.is_empty() {
        BUILTIN_BOTS
            .iter()
            .map(|&(name, src)| BotSourceInput {
                name: name.to_string(),
                source: src.to_string(),
            })
            .collect()
    } else {
        input
    };

    let mut compiled = Vec::with_capacity(list.len().min(8));
    for item in list.into_iter().take(8) {
        let name = if item.name.trim().is_empty() {
            "bot".to_string()
        } else {
            item.name.trim().to_string()
        };
        match compile_source_or_diags(&name, &item.source, OptLevel::Standard) {
            Ok(out) => compiled.push((name, out.verified)),
            Err(diags) => return Err((name, diags)),
        }
    }
    Ok(compiled)
}

#[derive(Deserialize)]
struct MatchReq {
    seed: Option<u64>,
    max_ticks: Option<u32>,
    #[serde(default)]
    bots: Vec<BotSourceInput>,
}

#[derive(Serialize)]
struct ArenaDto {
    width: f64,
    height: f64,
    max_ticks: u32,
    gas_budget: u32,
    bot_radius: f64,
    pillars: Vec<RectDto>,
}

#[derive(Serialize)]
struct RectDto {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Serialize)]
struct BotFrameDto {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    heading: f64,
    hp: i32,
    energy: i32,
    alive: bool,
    shield: bool,
    kills: u32,
    dmg: u32,
    gas: u32,
    vm_status: &'static str,
}

#[derive(Serialize)]
struct EventDto {
    kind: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bot: Option<usize>,
}

#[derive(Serialize)]
struct FrameDto {
    t: u32,
    zr: f64,
    hash: String,
    bots: Vec<BotFrameDto>,
    projectiles: Vec<(usize, f64, f64, f64, f64)>,
    events: Vec<EventDto>,
}

#[derive(Serialize)]
struct BotSummaryDto {
    name: String,
    color: String,
    hp: i32,
    alive: bool,
    kills: u32,
    damage_dealt: u32,
    survival_ticks: u32,
    avg_gas_per_tick: f64,
    total_instructions: u64,
    traps: u32,
    out_of_gas: u32,
}

#[derive(Serialize)]
struct MatchResp {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    determinism_verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replay_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    winner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arena: Option<ArenaDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bot_summary: Option<Vec<BotSummaryDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frames: Option<Vec<FrameDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_bot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<Vec<DiagnosticDto>>,
}

fn handle_match(req: MatchReq) -> MatchResp {
    let compiled = match resolve_bot_inputs(req.bots) {
        Ok(c) => c,
        Err((error_bot, diagnostics)) => {
            return MatchResp {
                ok: false,
                seed: None,
                determinism_verified: None,
                replay_bytes: None,
                final_hash: None,
                winner: None,
                arena: None,
                bot_summary: None,
                frames: None,
                error_bot: Some(error_bot),
                diagnostics: Some(diagnostics),
            };
        }
    };

    let seed = req.seed.unwrap_or(42);
    let max_ticks = req.max_ticks.unwrap_or(1200).clamp(50, 1200);
    let config = ArenaConfig {
        max_ticks,
        ..ArenaConfig::default()
    };

    let (summary, replay, raw_frames) = match record_match(compiled, seed, config.clone()) {
        Ok(res) => res,
        Err(err) => {
            return MatchResp {
                ok: false,
                seed: None,
                determinism_verified: None,
                replay_bytes: None,
                final_hash: None,
                winner: None,
                arena: None,
                bot_summary: None,
                frames: None,
                error_bot: Some(format!("simulation error: {err}")),
                diagnostics: None,
            };
        }
    };
    let determinism_verified = verify_replay(&replay).is_ok();
    let replay_bytes = encode_replay(&replay).len();

    let bot_colors: Vec<String> = summary
        .bots
        .iter()
        .enumerate()
        .map(|(idx, b)| {
            let (_, known) = bot_meta_for(&b.name);
            if BUILTIN_BOTS
                .iter()
                .any(|&(n, _)| n.eq_ignore_ascii_case(&b.name))
                || b.name.eq_ignore_ascii_case("rogue_loop")
            {
                known.to_string()
            } else {
                PALETTE[idx % PALETTE.len()].to_string()
            }
        })
        .collect();

    let names: Vec<String> = summary.bots.iter().map(|b| b.name.clone()).collect();
    let frames = raw_frames
        .into_iter()
        .map(|f| FrameDto {
            t: f.tick,
            zr: f.zone_radius.to_f64(),
            hash: format!("0x{:016x}", f.state_hash),
            bots: f
                .bots
                .into_iter()
                .map(|b| BotFrameDto {
                    x: b.x.to_f64(),
                    y: b.y.to_f64(),
                    vx: b.vx.to_f64(),
                    vy: b.vy.to_f64(),
                    heading: b.heading.to_f64(),
                    hp: b.hp,
                    energy: b.energy,
                    alive: b.alive,
                    shield: b.shield_active,
                    kills: b.kills,
                    dmg: b.damage_dealt,
                    gas: b.last_gas_used,
                    vm_status: match b.last_vm_status {
                        ExecStatus::Completed | ExecStatus::Running => "OK",
                        ExecStatus::OutOfGas => "OOG",
                        ExecStatus::Trapped(_) => "TRAP",
                    },
                })
                .collect(),
            projectiles: f
                .projectiles
                .into_iter()
                .map(|(o, x, y, vx, vy)| (o, x.to_f64(), y.to_f64(), vx.to_f64(), vy.to_f64()))
                .collect(),
            events: f
                .events
                .into_iter()
                .filter_map(|ev| convert_event(&ev, &names))
                .collect(),
        })
        .collect();

    let winner = summary
        .winner
        .and_then(|w| summary.bots.get(w))
        .map(|b| b.name.clone());

    let bot_summary = summary
        .bots
        .into_iter()
        .enumerate()
        .map(|(idx, b)| BotSummaryDto {
            name: b.name,
            color: bot_colors[idx].clone(),
            hp: b.hp,
            alive: b.alive,
            kills: b.kills,
            damage_dealt: b.damage_dealt,
            survival_ticks: b.survival_ticks,
            avg_gas_per_tick: b.avg_gas_per_tick,
            total_instructions: b.total_instructions,
            traps: b.traps,
            out_of_gas: b.out_of_gas_count,
        })
        .collect();

    MatchResp {
        ok: true,
        seed: Some(seed),
        determinism_verified: Some(determinism_verified),
        replay_bytes: Some(replay_bytes),
        final_hash: Some(format!("0x{:016x}", summary.final_state_hash)),
        winner,
        arena: Some(ArenaDto {
            width: config.width.to_f64(),
            height: config.height.to_f64(),
            max_ticks: config.max_ticks,
            gas_budget: config.gas_budget,
            bot_radius: config.bot_radius.to_f64(),
            pillars: config
                .pillars
                .iter()
                .map(|p| RectDto {
                    x: p.x.to_f64(),
                    y: p.y.to_f64(),
                    w: p.w.to_f64(),
                    h: p.h.to_f64(),
                })
                .collect(),
        }),
        bot_summary: Some(bot_summary),
        frames: Some(frames),
        error_bot: None,
        diagnostics: None,
    }
}

fn convert_event(ev: &CombatEvent, names: &[String]) -> Option<EventDto> {
    let bname = |i: usize| names.get(i).map_or("?", String::as_str);
    match ev {
        CombatEvent::Hit {
            tick,
            attacker,
            victim,
            damage,
            x,
            y,
        } => Some(EventDto {
            kind: "hit",
            text: format!(
                "T{tick:04} {} hit {} (-{damage} HP)",
                bname(*attacker),
                bname(*victim)
            ),
            x: Some(x.to_f64()),
            y: Some(y.to_f64()),
            bot: Some(*victim),
        }),
        CombatEvent::Kill {
            tick,
            killer: Some(k),
            victim,
        } => Some(EventDto {
            kind: "kill",
            text: format!("T{tick:04} {} eliminated {}!", bname(*k), bname(*victim)),
            x: None,
            y: None,
            bot: Some(*victim),
        }),
        CombatEvent::Kill {
            tick,
            killer: None,
            victim,
        } => Some(EventDto {
            kind: "storm",
            text: format!("T{tick:04} {} claimed by the storm", bname(*victim)),
            x: None,
            y: None,
            bot: Some(*victim),
        }),
        CombatEvent::Dash { tick, bot_id } => Some(EventDto {
            kind: "dash",
            text: format!("T{tick:04} {} dashed", bname(*bot_id)),
            x: None,
            y: None,
            bot: Some(*bot_id),
        }),
        CombatEvent::BotTrapped {
            tick,
            bot_id,
            reason,
        } => Some(EventDto {
            kind: "trap",
            text: format!("T{tick:04} [TRAP] {} faulted: {reason}", bname(*bot_id)),
            x: None,
            y: None,
            bot: Some(*bot_id),
        }),
        CombatEvent::BotOutOfGas { tick, bot_id } => Some(EventDto {
            kind: "oog",
            text: format!("T{tick:04} [OOG] {} hit gas ceiling", bname(*bot_id)),
            x: None,
            y: None,
            bot: Some(*bot_id),
        }),
        CombatEvent::DebugLog {
            tick,
            bot_id,
            tag,
            val,
        } => Some(EventDto {
            kind: "log",
            text: format!("T{tick:04} [LOG] {} tag={tag} val={val}", bname(*bot_id)),
            x: None,
            y: None,
            bot: Some(*bot_id),
        }),
        CombatEvent::Shot { .. } | CombatEvent::StormDamage { .. } => None,
    }
}

#[derive(Deserialize)]
struct TraceTickReq {
    seed: Option<u64>,
    tick: Option<u32>,
    bot_idx: Option<usize>,
    max_steps: Option<usize>,
    #[serde(default)]
    bots: Vec<BotSourceInput>,
}

#[derive(Serialize)]
struct DisasmLineDto {
    pc: usize,
    line: Option<u32>,
    text: String,
}

#[derive(Serialize)]
struct VmStepDto {
    step_idx: usize,
    pc: u32,
    gas_rem: u32,
    gas_used: u32,
    depth: usize,
    status: String,
    regs: [i32; 16],
    state: Vec<i32>,
}

#[derive(Serialize)]
struct TraceTickResp {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    bot_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tick: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disasm_lines: Option<Vec<DisasmLineDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    steps: Option<Vec<VmStepDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn handle_trace_tick(req: TraceTickReq) -> TraceTickResp {
    let compiled = match resolve_bot_inputs(req.bots) {
        Ok(c) => c,
        Err((bot, _)) => {
            return TraceTickResp {
                ok: false,
                bot_name: None,
                tick: None,
                disasm_lines: None,
                state_names: None,
                steps: None,
                error: Some(format!("bot `{bot}` failed compilation")),
            };
        }
    };
    let bot_idx = req.bot_idx.unwrap_or(0);
    if bot_idx >= compiled.len() {
        return TraceTickResp {
            ok: false,
            bot_name: None,
            tick: None,
            disasm_lines: None,
            state_names: None,
            steps: None,
            error: Some("bot index out of range".into()),
        };
    }

    let seed = req.seed.unwrap_or(42);
    let target_tick = req.tick.unwrap_or(0).min(1200);
    let max_steps = req.max_steps.unwrap_or(300).clamp(1, 400);
    let bot_name = compiled[bot_idx].0.clone();

    let mut sim = Simulation::new(compiled, seed, ArenaConfig::default());
    while sim.tick() < target_tick && sim.step_tick() {}

    let raw = sim.bot_program(bot_idx).program().clone();
    let state_len = raw.state_layout.len();
    let state_names = raw.state_names.clone();
    let disasm_lines = raw
        .code
        .iter()
        .enumerate()
        .map(|(pc, &insn)| DisasmLineDto {
            pc,
            line: raw.line_for_pc(pc as u32),
            text: format_instruction(&raw, pc, insn),
        })
        .collect();

    let mut steps = Vec::new();
    for step_idx in 0..max_steps {
        let status = sim.step_bot_instruction(bot_idx);
        let vm = sim.bot_vm(bot_idx);
        let mut regs16 = [0i32; 16];
        if let Some(active) = vm.active_regs() {
            regs16.copy_from_slice(&active[..16]);
        }
        steps.push(VmStepDto {
            step_idx,
            pc: vm.current_pc().unwrap_or(0),
            gas_rem: vm.gas_remaining(),
            gas_used: vm.gas_used(),
            depth: vm.depth(),
            status: format!("{status:?}"),
            regs: regs16,
            state: vm.state_slice(state_len).to_vec(),
        });
        if status != ExecStatus::Running {
            break;
        }
    }

    TraceTickResp {
        ok: true,
        bot_name: Some(bot_name),
        tick: Some(sim.tick()),
        disasm_lines: Some(disasm_lines),
        state_names: Some(state_names),
        steps: Some(steps),
        error: None,
    }
}

#[derive(Deserialize)]
struct TourneyReq {
    format: Option<String>,
    rounds: Option<u32>,
    seed: Option<u64>,
    audit_determinism: Option<bool>,
    #[serde(default)]
    bots: Vec<BotSourceInput>,
}

#[derive(Serialize)]
struct TourneyResp {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<phalanx_tourney::TournamentReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn handle_tourney(req: TourneyReq) -> TourneyResp {
    let compiled = match resolve_bot_inputs(req.bots) {
        Ok(c) => c,
        Err((bot, _)) => {
            return TourneyResp {
                ok: false,
                report: None,
                error: Some(format!("bot `{bot}` failed compilation")),
            };
        }
    };
    let entries: Vec<BotEntry> = compiled
        .into_iter()
        .map(|(name, program)| BotEntry { name, program })
        .collect();
    let format = match req.format.as_deref() {
        Some("royale") => TournamentFormat::Royale,
        _ => TournamentFormat::RoundRobin1v1,
    };
    let rounds = req.rounds.unwrap_or(10).clamp(1, 50);
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let cfg = TournamentConfig {
        format,
        rounds,
        threads,
        base_seed: req.seed.unwrap_or(1337),
        arena: ArenaConfig::default(),
        audit_determinism: req.audit_determinism.unwrap_or(true),
    };
    let report = run_tournament(&entries, &cfg);
    TourneyResp {
        ok: true,
        report: Some(report),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::bind_with_fallback;
    use std::net::TcpListener;

    #[test]
    fn port_fallback_skips_occupied_port() {
        let occupied = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
        let busy_port = occupied.local_addr().unwrap().port();
        let fallback = bind_with_fallback("127.0.0.1", busy_port).expect("fallback succeeds");
        assert_ne!(fallback.local_addr().unwrap().port(), busy_port);
    }
}
