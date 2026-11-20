use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use phalanx_cli::server::spawn_server_on;
use serde_json::Value;

fn http_request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect to embedded server");
    let payload = body.unwrap_or("");
    let req = if body.is_some() {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
    } else {
        format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    let (head, resp_body) = raw.split_once("\r\n\r\n").expect("valid HTTP separator");
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, resp_body.to_string())
}

#[test]
fn web_studio_endpoints_work_end_to_end() {
    let server = spawn_server_on("127.0.0.1:0").expect("bind ephemeral server");
    let addr = server.addr();

    // 1. GET /, /app.css, /app.js
    let (status, html) = http_request(addr, "GET", "/", None);
    assert_eq!(status, 200);
    assert!(html.contains("arenaCanvas"));
    assert!(html.contains("/app.css"));
    assert!(html.contains("/app.js"));

    let (css_status, css) = http_request(addr, "GET", "/app.css", None);
    assert_eq!(css_status, 200);
    assert!(css.contains("--bg-0"));

    let (js_status, js) = http_request(addr, "GET", "/app.js", None);
    assert_eq!(js_status, 200);
    assert!(js.contains("arenaCanvas"));

    // 2. GET /api/bots
    let (status, bots_json) = http_request(addr, "GET", "/api/bots", None);
    assert_eq!(status, 200);
    let bots: Vec<Value> = serde_json::from_str(&bots_json).unwrap();
    assert_eq!(bots.len(), 6);

    // 3. POST /api/compile (valid + invalid)
    let (status, comp_ok) = http_request(
        addr,
        "POST",
        "/api/compile",
        Some(r#"{"name":"sample","source":"fn tick() { let x: fix = 1.5; debug_log(1, x); }"}"#),
    );
    assert_eq!(status, 200);
    let v_ok: Value = serde_json::from_str(&comp_ok).unwrap();
    assert_eq!(v_ok["ok"], true);
    assert!(v_ok["report"]["instructions"].as_u64().unwrap() > 0);

    let (status, comp_err) = http_request(
        addr,
        "POST",
        "/api/compile",
        Some(r#"{"name":"bad","source":"fn tick() { let x: i32 = 1.5; }"}"#),
    );
    assert_eq!(status, 200);
    let v_err: Value = serde_json::from_str(&comp_err).unwrap();
    assert_eq!(v_err["ok"], false);
    assert!(!v_err["diagnostics"].as_array().unwrap().is_empty());

    // 4. POST /api/match
    let (status, match_json) = http_request(
        addr,
        "POST",
        "/api/match",
        Some(r#"{"seed":42,"max_ticks":200,"bots":[]}"#),
    );
    assert_eq!(status, 200);
    let m: Value = serde_json::from_str(&match_json).unwrap();
    assert_eq!(m["ok"], true);
    assert_eq!(m["determinism_verified"], true);
    assert!(!m["frames"].as_array().unwrap().is_empty());

    // 5. POST /api/trace-tick
    let (status, trace_json) = http_request(
        addr,
        "POST",
        "/api/trace-tick",
        Some(r#"{"seed":42,"tick":10,"bot_idx":0,"max_steps":50,"bots":[]}"#),
    );
    assert_eq!(status, 200);
    let tr: Value = serde_json::from_str(&trace_json).unwrap();
    assert_eq!(tr["ok"], true);
    assert!(!tr["steps"].as_array().unwrap().is_empty());

    // 6. POST /api/tourney
    let (status, tourney_json) = http_request(
        addr,
        "POST",
        "/api/tourney",
        Some(r#"{"format":"round-robin","rounds":2,"seed":99,"audit_determinism":true,"bots":[]}"#),
    );
    assert_eq!(status, 200);
    let t: Value = serde_json::from_str(&tourney_json).unwrap();
    assert_eq!(t["ok"], true);
    assert_eq!(t["report"]["audit"]["passed"], true);

    server.shutdown();
}
