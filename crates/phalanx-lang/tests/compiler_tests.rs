use phalanx_lang::{OptLevel, SourceMap, compile};
use phalanx_vm::{ExecStatus, HostApi, SyscallId, VmInstance};

struct MockHost {
    logs: Vec<(i32, i32)>,
}

impl HostApi for MockHost {
    fn invoke(&mut self, id: SyscallId, args: &[i32]) -> i32 {
        match id {
            SyscallId::DebugLog => {
                self.logs.push((args[0], args[1]));
                0
            }
            SyscallId::AbsInt => args[0].abs(),
            SyscallId::MinFix => args[0].min(args[1]),
            SyscallId::MaxFix => args[0].max(args[1]),
            _ => 0,
        }
    }
}

fn compile_and_run(src: &str, ticks: usize, opt: OptLevel) -> Vec<(i32, i32)> {
    let out = match compile("test_bot", src, opt) {
        Ok(o) => o,
        Err(diags) => {
            let sm = SourceMap::new(src);
            let rendered: Vec<String> = diags
                .iter()
                .map(|d| sm.render_diagnostic("test.phx", d))
                .collect();
            panic!("compile failed:\n{}", rendered.join("\n"));
        }
    };
    let mut vm = VmInstance::new(&out.verified);
    let mut host = MockHost { logs: Vec::new() };
    for _ in 0..ticks {
        let res = vm.run_tick(&out.verified, &mut host, 50_000);
        assert_eq!(res.status, ExecStatus::Completed);
    }
    host.logs
}

#[test]
fn compiles_and_runs_loops_recursion_free_calls_and_state() {
    let src = r#"
const LIMIT: i32 = 5 + 1;
state ticks: i32 = 0;
state history: [fix; 4];

fn fib_iter(n: i32) -> i32 {
    if n <= 1 {
        return n;
    }
    let mut a = 0;
    let mut b = 1;
    for _i in 2..n + 1 {
        let next = a + b;
        a = b;
        b = next;
    }
    return b;
}

fn tick() {
    ticks += 1;
    let f = fib_iter(LIMIT);
    let idx = (ticks - 1) % 4;
    history[idx] = to_fix(f + ticks);
    debug_log(f, history[idx]);
}
"#;

    let logs_none = compile_and_run(src, 3, OptLevel::None);
    let logs_opt = compile_and_run(src, 3, OptLevel::Standard);
    assert_eq!(logs_none, logs_opt);
    assert_eq!(
        logs_opt,
        vec![(8, 9 * 65536), (8, 10 * 65536), (8, 11 * 65536),]
    );
}

#[test]
fn short_circuit_and_for_continue_break_work_correctly() {
    let src = r#"
fn side_effect(flag: i32) -> bool {
    debug_log(flag, 1.0);
    return true;
}

fn tick() {
    // Second call must NOT execute due to short-circuiting.
    let a = false && side_effect(99);
    let b = true || side_effect(88);
    if !a && b {
        let mut sum = 0;
        for i in 0..10 {
            if i == 2 {
                continue;
            }
            if i == 6 {
                break;
            }
            sum += i; // 0 + 1 + 3 + 4 + 5 = 13
        }
        debug_log(sum, 2.5);
    }
}
"#;
    let logs = compile_and_run(src, 1, OptLevel::Standard);
    assert_eq!(logs, vec![(13, (5 * 65536) / 2)]);
}

#[test]
fn reports_rich_type_diagnostics() {
    let bad = "fn tick() {\n    let x: i32 = 1.5;\n}\n";
    let err = compile("bad", bad, OptLevel::Standard).expect_err("must fail");
    let sm = SourceMap::new(bad);
    let rendered = sm.render_diagnostic("bad.phx", &err[0]);
    assert!(rendered.contains("mismatched types"));
    assert!(rendered.contains("to_int"));
    assert!(rendered.contains("bad.phx:2:"));
}

#[test]
fn optimizer_eliminates_dead_constants_and_unreachable_branches() {
    let src = r#"
fn tick() {
    let dead = (2 + 3) * 10;
    let x = 4 * 1 + 0;
    if false {
        debug_log(dead, 0.0);
    }
    debug_log(x, 1.0);
}
"#;
    let out = compile("opt_test", src, OptLevel::Standard).unwrap();
    assert!(out.opt_stats.consts_folded > 0);
    assert!(out.opt_stats.dead_blocks_removed > 0);
}
