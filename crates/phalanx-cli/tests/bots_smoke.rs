use phalanx_lang::{OptLevel, compile};
use phalanx_sim::{ArenaConfig, Simulation};
use phalanx_vm::{ExecStatus, HostApi, SyscallId, VmInstance};

const BUILTINS: &[(&str, &str)] = &[
    ("kiter", include_str!("../../../bots/kiter.phx")),
    ("berserker", include_str!("../../../bots/berserker.phx")),
    ("sentry", include_str!("../../../bots/sentry.phx")),
    ("vanguard", include_str!("../../../bots/vanguard.phx")),
    ("ghost", include_str!("../../../bots/ghost.phx")),
];

const ROGUE_LOOP: &str = include_str!("../../../bots/examples/rogue_loop.phx");

struct DummyHost;

impl HostApi for DummyHost {
    fn invoke(&mut self, id: SyscallId, _args: &[i32]) -> i32 {
        match id {
            SyscallId::RadarNearest => 1,
            _ => 0,
        }
    }
}

#[test]
fn all_champion_bots_compile_verify_and_fight_without_faults() {
    let mut entries = Vec::new();
    for &(name, src) in BUILTINS {
        let out = compile(name, src, OptLevel::Standard)
            .unwrap_or_else(|e| panic!("failed to compile {name}: {e:?}"));
        entries.push((name.to_string(), out.verified));
    }

    let mut sim = Simulation::new(entries, 42, ArenaConfig::default());
    let summary = sim.run_to_completion();
    assert!(summary.ticks_elapsed > 10);
    for bot in &summary.bots {
        assert_eq!(bot.traps, 0, "{} triggered runtime trap", bot.name);
        assert_eq!(bot.out_of_gas_count, 0, "{} ran out of gas", bot.name);
    }
}

#[test]
fn rogue_loop_bot_verifies_and_halts_deterministically_at_gas_limit() {
    let out = compile("rogue_loop", ROGUE_LOOP, OptLevel::Standard).expect("compiles");
    let mut vm = VmInstance::new(&out.verified);
    let mut host = DummyHost;
    let res = vm.run_tick(&out.verified, &mut host, 2500);
    assert_eq!(res.status, ExecStatus::OutOfGas);
    assert!(res.gas_used <= 2500);
}
