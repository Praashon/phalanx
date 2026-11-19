use proptest::prelude::*;

use phalanx_vm::{
    ExecStatus, FunctionMeta, HostApi, Instruction, MAX_REGS, Program, SyscallId, ValType,
    VmInstance, decode_program, encode_program, verify,
};

struct FuzzHost;

impl HostApi for FuzzHost {
    fn invoke(&mut self, _id: SyscallId, _args: &[i32]) -> i32 {
        1
    }
}

proptest! {
    #[test]
    fn random_bytecode_never_panics_verifier_or_vm(words in prop::collection::vec(any::<u32>(), 1..64)) {
        let code: Vec<Instruction> = words.into_iter().map(Instruction).collect();
        let len = code.len() as u32;
        let prog = Program {
            name: "fuzz".into(),
            entry_func: 0,
            state_layout: vec![ValType::Int; 4],
            state_init: vec![0; 4],
            state_names: vec![String::new(); 4],
            state_arrays: vec![],
            constants: vec![],
            functions: vec![FunctionMeta {
                name: "tick".into(),
                entry_pc: 0,
                len,
                params: vec![],
                ret: None,
                num_regs: MAX_REGS as u8,
            }],
            code,
            debug_lines: vec![],
        };

        // Codec must round-trip any structurally valid program.
        let encoded = encode_program(&prog).expect("encode");
        let decoded = decode_program(&encoded).expect("decode");
        prop_assert_eq!(&prog, &decoded);

        // Verifier must never panic; if accepted, bounded VM execution must terminate cleanly.
        if let Ok((vp, _)) = verify(prog) {
            let mut vm = VmInstance::new(&vp);
            let mut host = FuzzHost;
            let res = vm.run_tick(&vp, &mut host, 500);
            prop_assert!(matches!(
                res.status,
                ExecStatus::Completed | ExecStatus::OutOfGas | ExecStatus::Trapped(_)
            ));
        }
    }

    #[test]
    fn malformed_phxb_bytes_never_panic_decoder(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = decode_program(&bytes);
    }
}
