use std::io::{self, Stdout, stdout};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect as UiRect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Rectangle};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use phalanx_sim::{ArenaConfig, CombatEvent, FrameSnapshot, MAX_HP, Simulation};
use phalanx_vm::disasm::format_instruction;
use phalanx_vm::{ExecStatus, VerifiedProgram};

const BOT_COLORS: [Color; 8] = [
    Color::Cyan,
    Color::LightRed,
    Color::LightGreen,
    Color::Yellow,
    Color::LightMagenta,
    Color::LightBlue,
    Color::White,
    Color::LightCyan,
];

/// RAII guard restoring terminal mode even on early return or panic.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<(Terminal<CrosstermBackend<Stdout>>, Self)> {
        enable_raw_mode()?;
        let guard = Self;
        execute!(stdout(), EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend)?;
        Ok((terminal, guard))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

/// Interactive TUI mode backing either a live Simulation or a recorded Replay.
pub enum TuiSession {
    Live {
        entries: Vec<(String, VerifiedProgram)>,
        config: ArenaConfig,
        seed: u64,
        sim: Box<Simulation>,
    },
    Replay {
        config: ArenaConfig,
        seed: u64,
        programs: Vec<(String, VerifiedProgram)>,
    },
}

struct AppState {
    session: TuiSession,
    history: Vec<FrameSnapshot>,
    event_log: Vec<String>,
    cursor: usize,
    playing: bool,
    speed: usize,
    debug_mode: bool,
    inspected_bot: usize,
    ascii_mode: bool,
}

impl AppState {
    fn new_live(
        entries: Vec<(String, VerifiedProgram)>,
        seed: u64,
        config: ArenaConfig,
        ascii_mode: bool,
    ) -> Self {
        let sim = Box::new(Simulation::new(entries.clone(), seed, config.clone()));
        let first = sim.snapshot();
        Self {
            session: TuiSession::Live {
                entries,
                config,
                seed,
                sim,
            },
            history: vec![first],
            event_log: vec![format!("Match initialized (seed=0x{seed:016x})")],
            cursor: 0,
            playing: true,
            speed: 1,
            debug_mode: false,
            inspected_bot: 0,
            ascii_mode,
        }
    }

    fn new_replay(
        programs: Vec<(String, VerifiedProgram)>,
        seed: u64,
        config: ArenaConfig,
        frames: Vec<FrameSnapshot>,
        ascii_mode: bool,
    ) -> Self {
        let mut event_log = vec![format!(
            "Loaded .phxr replay (seed=0x{seed:016x}, {} ticks)",
            frames.len().saturating_sub(1)
        )];
        if let Some(first) = frames.first() {
            append_events(&first.events, &first.bots, &mut event_log);
        }
        Self {
            session: TuiSession::Replay {
                config,
                seed,
                programs,
            },
            history: frames,
            event_log,
            cursor: 0,
            playing: true,
            speed: 1,
            debug_mode: false,
            inspected_bot: 0,
            ascii_mode,
        }
    }

    fn seed(&self) -> u64 {
        match &self.session {
            TuiSession::Live { seed, .. } | TuiSession::Replay { seed, .. } => *seed,
        }
    }

    fn config(&self) -> &ArenaConfig {
        match &self.session {
            TuiSession::Live { config, .. } | TuiSession::Replay { config, .. } => config,
        }
    }

    fn bot_count(&self) -> usize {
        self.history.first().map_or(0, |f| f.bots.len())
    }

    fn current_frame(&self) -> &FrameSnapshot {
        &self.history[self.cursor]
    }

    fn advance_one_tick(&mut self) {
        if self.cursor + 1 < self.history.len() {
            self.cursor += 1;
            let f = &self.history[self.cursor];
            append_events(&f.events, &f.bots, &mut self.event_log);
            return;
        }
        if let TuiSession::Live { sim, .. } = &mut self.session {
            if sim.step_tick() {
                let snap = sim.snapshot();
                append_events(&snap.events, &snap.bots, &mut self.event_log);
                self.history.push(snap);
                self.cursor = self.history.len() - 1;
            } else {
                self.playing = false;
            }
        } else {
            self.playing = false;
        }
    }

    fn step_vm_instruction(&mut self) {
        self.playing = false;
        self.debug_mode = true;
        self.cursor = self.history.len().saturating_sub(1);
        if let TuiSession::Live { sim, .. } = &mut self.session {
            let status = sim.step_bot_instruction(self.inspected_bot);
            if status != ExecStatus::Running {
                self.event_log.push(format!(
                    "[DBG] Bot #{} tick ended: {status:?}",
                    self.inspected_bot
                ));
            }
        }
    }

    fn restart(&mut self, new_seed: u64) {
        match &mut self.session {
            TuiSession::Live {
                entries,
                config,
                seed,
                sim,
            } => {
                *seed = new_seed;
                **sim = Simulation::new(entries.clone(), new_seed, config.clone());
                self.history.clear();
                self.history.push(sim.snapshot());
                self.cursor = 0;
                self.playing = true;
                self.event_log = vec![format!("Restarted match (seed=0x{new_seed:016x})")];
            }
            TuiSession::Replay { .. } => {
                self.cursor = 0;
                self.playing = true;
            }
        }
    }
}

fn append_events(events: &[CombatEvent], bots: &[phalanx_sim::BotState], log: &mut Vec<String>) {
    let bname = |id: usize| bots.get(id).map_or("?", |b| b.name.as_str());
    for ev in events {
        let msg = match ev {
            CombatEvent::Hit {
                tick,
                attacker,
                victim,
                damage,
                ..
            } => format!(
                "T{tick:04}  {} hit {} (-{damage} HP)",
                bname(*attacker),
                bname(*victim)
            ),
            CombatEvent::Kill {
                tick,
                killer: Some(k),
                victim,
            } => format!("T{tick:04}  {} ELIMINATED {}!", bname(*k), bname(*victim)),
            CombatEvent::Kill {
                tick,
                killer: None,
                victim,
            } => format!("T{tick:04}  {} claimed by the STORM", bname(*victim)),
            CombatEvent::Dash { tick, bot_id } => {
                format!("T{tick:04}  {} dashed", bname(*bot_id))
            }
            CombatEvent::BotTrapped {
                tick,
                bot_id,
                reason,
            } => format!("T{tick:04}  [TRAP] {} faulted: {reason}", bname(*bot_id)),
            CombatEvent::BotOutOfGas { tick, bot_id } => {
                format!("T{tick:04}  [OOG] {} hit gas ceiling", bname(*bot_id))
            }
            CombatEvent::DebugLog {
                tick,
                bot_id,
                tag,
                val,
            } => format!("T{tick:04}  [LOG] {} tag={tag} val={val}", bname(*bot_id)),
            CombatEvent::Shot { .. } | CombatEvent::StormDamage { .. } => continue,
        };
        log.push(msg);
    }
    if log.len() > 250 {
        let excess = log.len() - 250;
        log.drain(..excess);
    }
}

/// Runs the interactive Ratatui loop for a live match.
pub fn run_live_tui(
    entries: Vec<(String, VerifiedProgram)>,
    seed: u64,
    config: ArenaConfig,
    ascii_mode: bool,
) -> io::Result<()> {
    let app = AppState::new_live(entries, seed, config, ascii_mode);
    run_tui_loop(app)
}

/// Runs the interactive Ratatui loop for a recorded replay.
pub fn run_replay_tui(
    programs: Vec<(String, VerifiedProgram)>,
    seed: u64,
    config: ArenaConfig,
    frames: Vec<FrameSnapshot>,
    ascii_mode: bool,
) -> io::Result<()> {
    let app = AppState::new_replay(programs, seed, config, frames, ascii_mode);
    run_tui_loop(app)
}

fn run_tui_loop(mut app: AppState) -> io::Result<()> {
    let (mut terminal, _guard) = TerminalGuard::enter()?;

    let mut last_step = Instant::now();
    let tick_interval = Duration::from_millis(33);

    loop {
        terminal.draw(|f| draw_ui(f, &app))?;

        while event::poll(Duration::from_millis(8))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char(' ') => app.playing = !app.playing,
                    KeyCode::Left => {
                        app.playing = false;
                        app.cursor = app.cursor.saturating_sub(1);
                    }
                    KeyCode::Right => {
                        app.playing = false;
                        app.advance_one_tick();
                    }
                    KeyCode::Char('d') => app.debug_mode = !app.debug_mode,
                    KeyCode::Char('s') => app.step_vm_instruction(),
                    KeyCode::Tab => {
                        let count = app.bot_count().max(1);
                        app.inspected_bot = (app.inspected_bot + 1) % count;
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        app.speed = (app.speed * 2).min(8);
                    }
                    KeyCode::Char('-') => {
                        app.speed = (app.speed / 2).max(1);
                    }
                    KeyCode::Char('r') => {
                        let next_seed = app.seed().wrapping_add(1);
                        app.restart(next_seed);
                    }
                    KeyCode::Char('R') => {
                        let same_seed = app.seed();
                        app.restart(same_seed);
                    }
                    _ => {}
                }
            }
        }

        if app.playing && last_step.elapsed() >= tick_interval {
            for _ in 0..app.speed {
                if !app.playing {
                    break;
                }
                app.advance_one_tick();
            }
            last_step = Instant::now();
        }
    }
}

fn draw_ui(f: &mut Frame<'_>, app: &AppState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(16), Constraint::Length(3)])
        .split(f.area());

    let main_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(root[0]);

    draw_arena(f, app, main_cols[0]);

    let bot_rows = (app.bot_count() as u16) * 2 + 3;
    let sidebar = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(bot_rows.min(18)), Constraint::Min(10)])
        .split(main_cols[1]);

    draw_telemetry(f, app, sidebar[0]);
    if app.debug_mode {
        draw_debugger(f, app, sidebar[1]);
    } else {
        draw_event_log(f, app, sidebar[1]);
    }
    draw_footer(f, app, root[1]);
}

fn draw_arena(f: &mut Frame<'_>, app: &AppState, area: UiRect) {
    let frame = app.current_frame();
    let cfg = app.config();
    let w = cfg.width.to_f64();
    let h = cfg.height.to_f64();
    let marker = if app.ascii_mode {
        Marker::Dot
    } else {
        Marker::Braille
    };

    let title = format!(
        " PHALANX ARENA  [Tick {}/{}]  [Zone R={:.1}] ",
        frame.tick,
        cfg.max_ticks,
        frame.zone_radius.to_f64()
    );

    let canvas = Canvas::default()
        .block(Block::default().borders(Borders::ALL).title(title))
        .marker(marker)
        .x_bounds([0.0, w])
        .y_bounds([0.0, h])
        .paint(|ctx| {
            // Cover pillars
            for p in &cfg.pillars {
                ctx.draw(&Rectangle {
                    x: p.x.to_f64(),
                    y: p.y.to_f64(),
                    width: p.w.to_f64(),
                    height: p.h.to_f64(),
                    color: Color::DarkGray,
                });
            }

            // Shrinking storm boundary
            let cx = w / 2.0;
            let cy = h / 2.0;
            let zr = frame.zone_radius.to_f64();
            if zr < (w * w + h * h).sqrt() * 0.55 {
                ctx.draw(&Circle {
                    x: cx,
                    y: cy,
                    radius: zr,
                    color: Color::Magenta,
                });
            }

            // Projectiles
            for &(owner, px, py, vx, vy) in &frame.projectiles {
                let x = px.to_f64();
                let y = py.to_f64();
                let color = BOT_COLORS[owner % BOT_COLORS.len()];
                ctx.draw(&CanvasLine {
                    x1: x - vx.to_f64() * 0.8,
                    y1: y - vy.to_f64() * 0.8,
                    x2: x,
                    y2: y,
                    color,
                });
            }

            // Impact sparks
            for ev in &frame.events {
                if let CombatEvent::Hit { x, y, .. } = ev {
                    let hx = x.to_f64();
                    let hy = y.to_f64();
                    ctx.print(
                        hx,
                        hy,
                        Span::styled(
                            "*",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                    );
                }
            }

            // Bots
            let br = cfg.bot_radius.to_f64();
            for (idx, bot) in frame.bots.iter().enumerate() {
                if !bot.alive {
                    continue;
                }
                let color = BOT_COLORS[idx % BOT_COLORS.len()];
                let bx = bot.x.to_f64();
                let by = bot.y.to_f64();
                ctx.draw(&Circle {
                    x: bx,
                    y: by,
                    radius: br,
                    color,
                });
                if bot.shield_active {
                    ctx.draw(&Circle {
                        x: bx,
                        y: by,
                        radius: br + 0.8,
                        color: Color::LightCyan,
                    });
                }
                let hx = bx + bot.heading.to_f64().cos() * (br * 1.6);
                let hy = by + bot.heading.to_f64().sin() * (br * 1.6);
                ctx.draw(&CanvasLine {
                    x1: bx,
                    y1: by,
                    x2: hx,
                    y2: hy,
                    color: Color::White,
                });
                let tag = bot.name.chars().next().unwrap_or('?').to_ascii_uppercase();
                ctx.print(
                    bx - 0.6,
                    by + br + 0.8,
                    Span::styled(
                        format!("{tag}:{}", bot.hp),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                );
            }
        });

    f.render_widget(canvas, area);
}

fn draw_telemetry(f: &mut Frame<'_>, app: &AppState, area: UiRect) {
    let frame = app.current_frame();
    let mut lines = Vec::new();
    for (idx, bot) in frame.bots.iter().enumerate() {
        let color = BOT_COLORS[idx % BOT_COLORS.len()];
        let selected = if idx == app.inspected_bot { ">" } else { " " };
        let status_tag = if !bot.alive {
            "DEAD"
        } else {
            match bot.last_vm_status {
                ExecStatus::Completed | ExecStatus::Running => "OK  ",
                ExecStatus::OutOfGas => "OOG ",
                ExecStatus::Trapped(_) => "TRAP",
            }
        };
        let hp_pct = ((bot.hp.max(0) as usize) * 10) / (MAX_HP as usize);
        let hp_bar = format!("{}{}", "#".repeat(hp_pct), "-".repeat(10 - hp_pct));
        lines.push(Line::from(vec![
            Span::styled(format!("{selected} "), Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("{:<10} ", truncate_str(&bot.name, 10)),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("HP {:>3} [{hp_bar}] ", bot.hp)),
            Span::styled(
                format!("EN {:>3} ", bot.energy),
                Style::default().fg(Color::Blue),
            ),
            Span::raw(format!("K:{} ", bot.kills)),
        ]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                format!(
                    "Gas:{:>4}/{}  VM:{status_tag}  Dmg:{:<3}",
                    bot.last_gas_used,
                    app.config().gas_budget,
                    bot.damage_dealt
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" BOT TELEMETRY (Tab: select) ");
    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}

fn draw_event_log(f: &mut Frame<'_>, app: &AppState, area: UiRect) {
    let frame = app.current_frame();
    let max_lines = area.height.saturating_sub(2) as usize;
    let start = app.event_log.len().saturating_sub(max_lines);
    let lines: Vec<Line<'_>> = app.event_log[start..]
        .iter()
        .map(|s| Line::from(Span::raw(s.as_str())))
        .collect();

    let title = format!(" COMBAT LOG  [Hash 0x{:016x}] ", frame.state_hash);
    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    f.render_widget(widget, area);
}

fn draw_debugger(f: &mut Frame<'_>, app: &AppState, area: UiRect) {
    let bot_idx = app.inspected_bot.min(app.bot_count().saturating_sub(1));
    let mut lines = Vec::new();

    match &app.session {
        TuiSession::Live { sim, .. } => {
            let vp = sim.bot_program(bot_idx);
            let raw = vp.program();
            let vm = sim.bot_vm(bot_idx);
            let cur_pc = vm.current_pc().unwrap_or(0) as usize;

            lines.push(Line::from(Span::styled(
                format!(
                    "Bot #{bot_idx} ({})  Status={:?}  PC={cur_pc}  Depth={}  GasRem={}",
                    raw.name,
                    vm.status(),
                    vm.depth(),
                    vm.gas_remaining()
                ),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));

            let win_start = cur_pc.saturating_sub(2);
            let win_end = (win_start + 7).min(raw.code.len());
            for pc in win_start..win_end {
                let marker = if pc == cur_pc && vm.status() == ExecStatus::Running {
                    ">>"
                } else {
                    "  "
                };
                let insn_str = format_instruction(raw, pc, raw.code[pc]);
                let style = if marker == ">>" {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Gray)
                };
                lines.push(Line::from(Span::styled(
                    format!("{marker} {pc:04}: {insn_str}"),
                    style,
                )));
            }

            if let Some(regs) = vm.active_regs() {
                lines.push(Line::from(Span::styled(
                    "Registers (r0..r7):",
                    Style::default().fg(Color::Green),
                )));
                for row in 0..2 {
                    let base = row * 4;
                    let text = (0..4)
                        .map(|j| {
                            let r = base + j;
                            format!("r{r}:{:<8}", regs[r])
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    lines.push(Line::from(Span::raw(text)));
                }
            }

            let state_len = raw.state_layout.len().min(6);
            if state_len > 0 {
                let vals = vm.state_slice(state_len);
                let state_str = vals
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let name = raw.state_names.get(i).map_or("?", String::as_str);
                        format!("{name}={v}")
                    })
                    .collect::<Vec<_>>()
                    .join("  ");
                lines.push(Line::from(Span::styled(
                    format!("State: {state_str}"),
                    Style::default().fg(Color::LightMagenta),
                )));
            }
        }
        TuiSession::Replay { programs, .. } => {
            if let Some((name, vp)) = programs.get(bot_idx) {
                let raw = vp.program();
                lines.push(Line::from(Span::styled(
                    format!(
                        "Replay Disassembly: {name} ({} insns) - live step in `watch`",
                        raw.code.len()
                    ),
                    Style::default().fg(Color::Yellow),
                )));
                for (pc, &insn) in raw.code.iter().take(10).enumerate() {
                    lines.push(Line::from(Span::raw(format!(
                        "   {pc:04}: {}",
                        format_instruction(raw, pc, insn)
                    ))));
                }
            }
        }
    }

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" VM BYTECODE DEBUGGER (`s` step, `d` close) "),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(widget, area);
}

fn draw_footer(f: &mut Frame<'_>, app: &AppState, area: UiRect) {
    let mode_lbl = if app.playing { "PLAY" } else { "PAUSE" };
    let dbg_lbl = if app.debug_mode { "DBG:ON" } else { "DBG:OFF" };
    let text = format!(
        " [{mode_lbl} {speed}x] [{dbg_lbl}]  Space:Play/Pause  <-/->:Scrub  d:Debugger  s:StepVM  Tab:Bot  +/-:Speed  r/R:Restart  q:Quit   Seed:0x{:08x}",
        app.seed(),
        speed = app.speed
    );
    let footer = Paragraph::new(Line::from(Span::styled(
        text,
        Style::default().fg(Color::Cyan),
    )))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, area);
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len { s } else { &s[..max_len] }
}
