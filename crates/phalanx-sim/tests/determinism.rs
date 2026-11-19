use phalanx_lang::{OptLevel, compile};
use phalanx_sim::{
    ArenaConfig, Simulation, decode_replay, encode_replay, record_match, verify_replay,
};

const BOT_A: &str = r#"
state dir: i32 = 1;
fn tick() {
    let tid = radar_nearest();
    if tid >= 0 {
        let a = target_angle(tid);
        let d = target_dist(tid);
        if d > 18.0 {
            thrust(a, 0.9);
        } else {
            thrust(a + HALF_PI * to_fix(dir), 0.8);
        }
        let _fired = fire(a, 2);
    }
    if tick_num() % 40 == 0 {
        dir = -dir;
    }
}
"#;

const BOT_B: &str = r#"
fn tick() {
    let tid = radar_nearest();
    if tid >= 0 {
        let a = target_angle(tid);
        thrust(a, 0.7);
        if wall_dist(self_heading()) < 5.0 {
            let _d = dash(a + PI);
        }
        let _f = fire(a, 1);
    }
}
"#;

#[test]
fn simulation_is_bit_exact_deterministic_and_replay_verifiable() {
    let p1 = compile("bot_a", BOT_A, OptLevel::Standard)
        .unwrap()
        .verified;
    let p2 = compile("bot_b", BOT_B, OptLevel::Standard)
        .unwrap()
        .verified;
    let cfg = ArenaConfig {
        max_ticks: 300,
        ..ArenaConfig::default()
    };

    let mut sim1 = Simulation::new(
        vec![("A".into(), p1.clone()), ("B".into(), p2.clone())],
        99_123,
        cfg.clone(),
    );
    let mut sim2 = Simulation::new(
        vec![("A".into(), p1.clone()), ("B".into(), p2.clone())],
        99_123,
        cfg.clone(),
    );

    while sim1.step_tick() {
        assert!(sim2.step_tick());
        assert_eq!(sim1.state_hash(), sim2.state_hash());
    }
    assert!(!sim2.step_tick());

    let (summary, replay, frames) =
        record_match(vec![("A".into(), p1), ("B".into(), p2)], 99_123, cfg).unwrap();
    assert_eq!(summary.final_state_hash, sim1.state_hash());
    assert!(!frames.is_empty());

    let bytes = encode_replay(&replay);
    let decoded = decode_replay(&bytes).unwrap();
    let report = verify_replay(&decoded).unwrap();
    assert_eq!(report.final_state_hash, summary.final_state_hash);
    assert_eq!(report.ticks_verified, summary.ticks_elapsed as usize);

    // Tampered hash in replay must fail verification.
    let mut bad = decoded.clone();
    if let Some(last) = bad.tick_hashes.last_mut() {
        *last ^= 0x1;
    }
    assert!(verify_replay(&bad).is_err());
}
