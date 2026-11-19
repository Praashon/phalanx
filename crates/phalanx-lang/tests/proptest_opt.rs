use proptest::prelude::*;

use phalanx_lang::{OptLevel, compile};
use phalanx_vm::{ExecStatus, HostApi, SyscallId, VmInstance};

struct CaptureHost(Vec<i32>);

impl HostApi for CaptureHost {
    fn invoke(&mut self, id: SyscallId, args: &[i32]) -> i32 {
        if id == SyscallId::DebugLog {
            self.0.push(args[0]);
        }
        0
    }
}

fn gen_expr(depth: usize) -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        (-50i32..50).prop_map(|n| n.to_string()),
        Just("a".to_string()),
        Just("b".to_string()),
        Just("c".to_string()),
    ];
    leaf.prop_recursive(depth as u32, 32, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(l, r)| format!("({l} + {r})")),
            (inner.clone(), inner.clone()).prop_map(|(l, r)| format!("({l} - {r})")),
            (inner.clone(), inner.clone()).prop_map(|(l, r)| format!("({l} * {r})")),
            (inner.clone(), inner.clone()).prop_map(|(l, r)| format!("({l} ^ {r})")),
        ]
    })
}

proptest! {
    #[test]
    fn optimized_and_unoptimized_code_produce_identical_results(
        a in -100i32..100,
        b in -100i32..100,
        c in -100i32..100,
        e1 in gen_expr(3),
        e2 in gen_expr(3),
    ) {
        let src = format!(
            "fn compute(a: i32, b: i32, c: i32) -> i32 {{\n\
                 let mut acc = {e1};\n\
                 if a < b || (c & 1) == 0 {{\n\
                     acc += {e2};\n\
                 }} else {{\n\
                     acc -= {e2};\n\
                 }}\n\
                 return acc;\n\
             }}\n\
             fn tick() {{\n\
                 debug_log(compute({a}, {b}, {c}), 0.0);\n\
             }}"
        );

        let out_none = compile("prop_none", &src, OptLevel::None).expect("compile O0");
        let out_std = compile("prop_std", &src, OptLevel::Standard).expect("compile O1");

        let mut vm0 = VmInstance::new(&out_none.verified);
        let mut h0 = CaptureHost(Vec::new());
        let r0 = vm0.run_tick(&out_none.verified, &mut h0, 10_000);
        prop_assert_eq!(r0.status, ExecStatus::Completed);

        let mut vm1 = VmInstance::new(&out_std.verified);
        let mut h1 = CaptureHost(Vec::new());
        let r1 = vm1.run_tick(&out_std.verified, &mut h1, 10_000);
        prop_assert_eq!(r1.status, ExecStatus::Completed);

        prop_assert_eq!(h0.0, h1.0);
    }
}
