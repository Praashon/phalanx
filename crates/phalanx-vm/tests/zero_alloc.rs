//! Proves that `VmInstance::begin_tick` and `VmInstance::run_tick` perform zero heap allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAlloc;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOC_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOC_COUNT.with(|c| c.set(c.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOC_COUNT.with(|c| c.set(c.get() + 1));
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

use phalanx_vm::{
    ExecStatus, FunctionMeta, HostApi, Instruction, Opcode, Program, SyscallId, ValType,
    VmInstance, verify,
};

struct DummyHost;

impl HostApi for DummyHost {
    fn invoke(&mut self, id: SyscallId, args: &[i32]) -> i32 {
        match id {
            SyscallId::SelfHp => 100,
            SyscallId::AbsInt => args[0].abs(),
            _ => 0,
        }
    }
}

#[test]
fn vm_run_tick_performs_zero_heap_allocations() {
    let prog = Program {
        name: "zero_alloc_bot".into(),
        entry_func: 0,
        state_layout: vec![ValType::Int],
        state_init: vec![0],
        state_names: vec!["counter".into()],
        state_arrays: vec![],
        constants: vec![],
        functions: vec![
            FunctionMeta {
                name: "tick".into(),
                entry_pc: 0,
                len: 9,
                params: vec![],
                ret: None,
                num_regs: 6,
            },
            FunctionMeta {
                name: "helper".into(),
                entry_pc: 9,
                len: 3,
                params: vec![ValType::Int],
                ret: Some(ValType::Int),
                num_regs: 3,
            },
        ],
        code: vec![
            Instruction::ab_imm(Opcode::LoadState, 0, 0),
            Instruction::ab_imm(Opcode::LoadImmI, 1, 1),
            Instruction::abc(Opcode::AddI, 0, 0, 1),
            Instruction::abc(Opcode::Call, 2, 1, 0),
            Instruction::abc(Opcode::Syscall, 3, SyscallId::SelfHp as u8, 0),
            Instruction::abc(Opcode::AddI, 0, 2, 3),
            Instruction::ab_imm(Opcode::StoreState, 0, 0),
            Instruction::ab_imm(Opcode::LoadImmI, 4, 0),
            Instruction::abc(Opcode::Ret, 0, 0, 0),
            // helper(r0) -> r0 + 5
            Instruction::ab_imm(Opcode::LoadImmI, 1, 5),
            Instruction::abc(Opcode::AddI, 2, 0, 1),
            Instruction::abc(Opcode::Ret, 2, 1, 0),
        ],
        debug_lines: vec![],
    };

    let (vp, _) = verify(prog).expect("verified");
    let mut vm = VmInstance::new(&vp);
    let mut host = DummyHost;

    // Warm up TLS slots before enabling allocation counting.
    COUNTING.with(|c| c.set(false));
    ALLOC_COUNT.with(|c| c.set(0));
    let _ = vm.run_tick(&vp, &mut host, 1000);

    ALLOC_COUNT.with(|c| c.set(0));
    COUNTING.with(|c| c.set(true));

    for _ in 0..1_000 {
        let res = vm.run_tick(&vp, &mut host, 1000);
        assert_eq!(res.status, ExecStatus::Completed);
    }

    COUNTING.with(|c| c.set(false));
    let allocs = ALLOC_COUNT.with(Cell::get);
    assert_eq!(allocs, 0, "VM tick loop must allocate 0 bytes on heap");
}
