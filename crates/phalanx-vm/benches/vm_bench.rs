use criterion::{Criterion, black_box, criterion_group, criterion_main};
use phalanx_vm::{
    FunctionMeta, HostApi, Instruction, Opcode, Program, SyscallId, VmInstance, verify,
};

struct BenchHost;

impl HostApi for BenchHost {
    fn invoke(&mut self, id: SyscallId, _args: &[i32]) -> i32 {
        match id {
            SyscallId::SelfHp => 100,
            _ => 0,
        }
    }
}

fn build_loop_program(iters: i16) -> phalanx_vm::VerifiedProgram {
    // r0 = iters; r1 = 0 (acc); r2 = 1;
    // loop: r1 += r0; r0 -= r2; r3 = r0 > 0; if r3 jump -3; ret
    let prog = Program {
        name: "tight_loop".into(),
        entry_func: 0,
        state_layout: vec![],
        state_init: vec![],
        state_names: vec![],
        state_arrays: vec![],
        constants: vec![],
        functions: vec![FunctionMeta {
            name: "tick".into(),
            entry_pc: 0,
            len: 9,
            params: vec![],
            ret: None,
            num_regs: 4,
        }],
        code: vec![
            Instruction::ab_simm(Opcode::LoadImmI, 0, iters),
            Instruction::ab_imm(Opcode::LoadImmI, 1, 0),
            Instruction::ab_imm(Opcode::LoadImmI, 2, 1),
            Instruction::ab_imm(Opcode::LoadImmI, 4, 0),
            Instruction::abc(Opcode::AddI, 1, 1, 0),
            Instruction::abc(Opcode::SubI, 0, 0, 2),
            Instruction::abc(Opcode::GtI, 3, 0, 4),
            Instruction::ab_simm(Opcode::JumpIfTrue, 3, -3),
            Instruction::abc(Opcode::Ret, 0, 0, 0),
        ],
        debug_lines: vec![],
    };
    verify(prog).expect("verified").0
}

fn bench_vm_loop(c: &mut Criterion) {
    let vp = build_loop_program(250);
    let mut vm = VmInstance::new(&vp);
    let mut host = BenchHost;

    c.bench_function("vm_250_iter_loop_1000_insns", |b| {
        b.iter(|| {
            let res = vm.run_tick(black_box(&vp), &mut host, 50_000);
            black_box(res.instructions_executed);
        });
    });
}

criterion_group!(benches, bench_vm_loop);
criterion_main!(benches);
