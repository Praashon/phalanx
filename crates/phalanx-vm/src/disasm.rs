//! Human-readable disassembler for Phalanx bytecode programs.

use core::fmt::Write;

use crate::isa::{Instruction, Opcode, Program, ValType};
use crate::syscall::SyscallSpec;

/// Formats a single instruction at `pc` with resolved constant, state, function, and syscall names.
#[must_use]
pub fn format_instruction(prog: &Program, pc: usize, insn: Instruction) -> String {
    let Some(op) = insn.opcode() else {
        return format!("0x{:08x}  <invalid op 0x{:02x}>", insn.0, insn.raw_opcode());
    };
    let m = op.mnemonic();
    let a = insn.a();
    let b = insn.b();
    let c = insn.c();
    let imm_u = insn.imm16();
    let imm_s = insn.imm16_i16();

    match op {
        Opcode::Nop | Opcode::Trap => m.to_string(),
        Opcode::LoadImmI => format!("{m:<7} r{a}, {imm_s}"),
        Opcode::LoadImmBool => {
            let val = if imm_u != 0 { "true" } else { "false" };
            format!("{m:<7} r{a}, {val}")
        }
        Opcode::LoadConst => {
            let hint = prog
                .constants
                .get(usize::from(imm_u))
                .map(|entry| match entry.ty {
                    ValType::Fix => format!("; {:.4}", f64::from(entry.value) / 65536.0),
                    ValType::Int => format!("; {}", entry.value),
                    ValType::Bool => format!("; {}", entry.value != 0),
                })
                .unwrap_or_default();
            format!("{m:<7} r{a}, const[{imm_u}] {hint}")
                .trim_end()
                .to_string()
        }
        Opcode::Move
        | Opcode::NegI
        | Opcode::NegFix
        | Opcode::BitNot
        | Opcode::NotBool
        | Opcode::ToFix
        | Opcode::ToInt => format!("{m:<7} r{a}, r{b}"),
        Opcode::AddI
        | Opcode::SubI
        | Opcode::MulI
        | Opcode::DivI
        | Opcode::ModI
        | Opcode::AddFix
        | Opcode::SubFix
        | Opcode::MulFix
        | Opcode::DivFix
        | Opcode::BitAnd
        | Opcode::BitOr
        | Opcode::BitXor
        | Opcode::Shl
        | Opcode::Shr
        | Opcode::EqI
        | Opcode::NeI
        | Opcode::LtI
        | Opcode::LeI
        | Opcode::GtI
        | Opcode::GeI
        | Opcode::EqFix
        | Opcode::NeFix
        | Opcode::LtFix
        | Opcode::LeFix
        | Opcode::GtFix
        | Opcode::GeFix
        | Opcode::EqBool
        | Opcode::NeBool => format!("{m:<7} r{a}, r{b}, r{c}"),
        Opcode::LoadState | Opcode::StoreState => {
            let sname = prog
                .state_names
                .get(usize::from(imm_u))
                .filter(|s| !s.is_empty())
                .map(|s| format!(" ; {s}"))
                .unwrap_or_default();
            if op == Opcode::LoadState {
                format!("{m:<7} r{a}, state[{imm_u}]{sname}")
            } else {
                format!("{m:<7} state[{imm_u}], r{a}{sname}")
            }
        }
        Opcode::LoadStateArr | Opcode::StoreStateArr => {
            let aname = prog
                .state_arrays
                .get(usize::from(b))
                .map(|d| d.name.as_str())
                .unwrap_or("?");
            if op == Opcode::LoadStateArr {
                format!("{m:<7} r{a}, {aname}[r{c}]")
            } else {
                format!("{m:<7} {aname}[r{c}], r{a}")
            }
        }
        Opcode::Jump => {
            let target = (pc as i32) + i32::from(imm_s);
            format!("{m:<7} -> pc {target} ({imm_s:+})")
        }
        Opcode::JumpIfFalse | Opcode::JumpIfTrue => {
            let target = (pc as i32) + i32::from(imm_s);
            format!("{m:<7} r{a}, -> pc {target} ({imm_s:+})")
        }
        Opcode::Call => {
            let fname = prog
                .functions
                .get(usize::from(b))
                .map(|f| f.name.as_str())
                .unwrap_or("?");
            format!("{m:<7} r{a}, {fname}(r{c}..)")
        }
        Opcode::Ret => {
            if b != 0 {
                format!("{m:<7} r{a}")
            } else {
                "ret".to_string()
            }
        }
        Opcode::Syscall => {
            let sname = SyscallSpec::from_id(b).map(|s| s.name).unwrap_or("?");
            format!("{m:<7} r{a}, {sname}(r{c}..)")
        }
    }
}

/// Disassembles an entire [`Program`] into annotated assembly text.
#[must_use]
pub fn disassemble(prog: &Program) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "; program \"{}\"  fingerprint=0x{:016x}  state_slots={}  consts={}",
        prog.name,
        prog.fingerprint(),
        prog.state_layout.len(),
        prog.constants.len()
    );
    for arr in &prog.state_arrays {
        let _ = writeln!(
            out,
            "; state array {}: [{}; {}] @ slot {}",
            arr.name, arr.elem_ty, arr.len, arr.base
        );
    }

    for (idx, func) in prog.functions.iter().enumerate() {
        let params = func
            .params
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let ret = func
            .ret
            .map_or_else(|| "void".to_string(), |t| t.to_string());
        let entry_tag = if usize::from(prog.entry_func) == idx {
            " [entry]"
        } else {
            ""
        };
        let _ = writeln!(out, "\nfn {}({params}) -> {ret}{entry_tag}:", func.name);

        let start = func.entry_pc as usize;
        let end = start.saturating_add(func.len as usize).min(prog.code.len());
        let mut last_line = None;
        for (pc, &insn) in prog.code.iter().enumerate().take(end).skip(start) {
            let line = prog.line_for_pc(pc as u32);
            let line_col = match line {
                Some(l) if last_line != Some(l) => {
                    last_line = Some(l);
                    format!("L{l:<4}")
                }
                _ => "     ".to_string(),
            };
            let _ = writeln!(
                out,
                "  {pc:04}:  {line_col} {}",
                format_instruction(prog, pc, insn)
            );
        }
    }
    out
}
