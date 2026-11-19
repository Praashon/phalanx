use criterion::{Criterion, black_box, criterion_group, criterion_main};
use phalanx_lang::{OptLevel, compile};
use phalanx_sim::{ArenaConfig, Simulation};

const BENCH_BOT: &str = r#"
fn tick() {
    let tid = radar_nearest();
    if tid >= 0 {
        let a = target_angle(tid);
        let d = target_dist(tid);
        if d > 16.0 {
            thrust(a, 0.85);
        } else {
            thrust(a + HALF_PI, 0.75);
        }
        let _f = fire(a, 2);
    }
}
"#;

fn bench_five_bot_arena_tick(c: &mut Criterion) {
    let prog = compile("bench_bot", BENCH_BOT, OptLevel::Standard)
        .expect("compiles")
        .verified;
    let entries: Vec<_> = (0..5).map(|i| (format!("bot_{i}"), prog.clone())).collect();
    let cfg = ArenaConfig {
        max_ticks: 1_000_000,
        ..ArenaConfig::default()
    };
    let mut sim = Simulation::new(entries, 42, cfg);

    c.bench_function("sim_5bot_arena_step_tick", |b| {
        b.iter(|| {
            let alive = sim.step_tick();
            black_box(sim.state_hash());
            if !alive {
                let entries2: Vec<_> = (0..5).map(|i| (format!("bot_{i}"), prog.clone())).collect();
                sim = Simulation::new(
                    entries2,
                    42,
                    ArenaConfig {
                        max_ticks: 1_000_000,
                        ..ArenaConfig::default()
                    },
                );
            }
        });
    });
}

criterion_group!(benches, bench_five_bot_arena_tick);
criterion_main!(benches);
