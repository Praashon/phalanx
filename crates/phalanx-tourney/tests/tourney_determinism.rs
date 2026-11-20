use phalanx_lang::{OptLevel, compile};
use phalanx_sim::ArenaConfig;
use phalanx_tourney::{BotEntry, TournamentConfig, TournamentFormat, run_tournament};

const B1: &str = r#"
fn tick() {
    let tid = radar_nearest();
    if tid >= 0 {
        let a = target_angle(tid);
        thrust(a, 0.8);
        let _f = fire(a, 2);
    }
}
"#;

const B2: &str = r#"
fn tick() {
    let tid = radar_nearest();
    if tid >= 0 {
        let a = target_angle(tid);
        thrust(a + HALF_PI, 0.9);
        let _f = fire(a, 1);
    }
}
"#;

const B3: &str = r#"
fn tick() {
    let tid = radar_nearest();
    if tid >= 0 {
        let a = target_angle(tid);
        thrust(a - HALF_PI, 0.7);
        let _f = fire(a, 3);
    }
}
"#;

#[test]
fn single_and_multi_thread_tournaments_are_identical_and_pass_audit() {
    let entries = vec![
        BotEntry {
            name: "alpha".into(),
            program: compile("alpha", B1, OptLevel::Standard).unwrap().verified,
        },
        BotEntry {
            name: "beta".into(),
            program: compile("beta", B2, OptLevel::Standard).unwrap().verified,
        },
        BotEntry {
            name: "gamma".into(),
            program: compile("gamma", B3, OptLevel::Standard).unwrap().verified,
        },
    ];

    let cfg1 = TournamentConfig {
        format: TournamentFormat::RoundRobin1v1,
        rounds: 3,
        threads: 1,
        base_seed: 2026,
        arena: ArenaConfig {
            max_ticks: 250,
            ..ArenaConfig::default()
        },
        audit_determinism: true,
    };
    let mut cfg4 = cfg1.clone();
    cfg4.threads = 4;

    let r1 = run_tournament(&entries, &cfg1);
    let r4 = run_tournament(&entries, &cfg4);

    assert!(r1.audit.as_ref().unwrap().passed);
    assert!(r4.audit.as_ref().unwrap().passed);
    assert_eq!(r1.standings.len(), r4.standings.len());
    for (s1, s4) in r1.standings.iter().zip(r4.standings.iter()) {
        assert_eq!(s1.name, s4.name);
        assert!((s1.elo - s4.elo).abs() < 1e-12);
        assert_eq!(s1.wins, s4.wins);
        assert_eq!(s1.kills, s4.kills);
    }
}
