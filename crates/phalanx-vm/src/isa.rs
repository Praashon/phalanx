//! 32-bit fixed-width RISC register ISA, types, and program metadata.

use core::fmt;

/// Maximum number of 32-bit general-purpose registers per call frame.
pub const MAX_REGS: usize = 32;
/// Maximum static/dynamic call stack depth.
pub const MAX_CALL_DEPTH: usize = 16;
/// Maximum persistent scalar state slots across a bot's lifetime.
pub const MAX_STATE_SLOTS: usize = 64;
/// Maximum number of persistent state arrays declared by a bot.
pub const MAX_STATE_ARRAYS: usize = 16;
/// Maximum number of functions in a compiled program.
pub const MAX_FUNCTIONS: usize = 64;
/// Maximum number of constant pool entries.
pub const MAX_CONSTANTS: usize = 512;
/// Maximum number of 32-bit instructions in a program.
pub const MAX_INSTRUCTIONS: usize = 8192;

/// Static primitive value types tracked by the compiler and bytecode verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ValType {
    /// Signed 32-bit integer (`i32`).
    Int = 0,
    /// Signed Q16.16 fixed-point number (`fix`).
    Fix = 1,
    /// Boolean (`false` = 0, `true` = 1).
    Bool = 2,
}

impl ValType {
    /// Converts a raw byte tag into a [`ValType`].
    #[must_use]
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Int),
            1 => Some(Self::Fix),
            2 => Some(Self::Bool),
            _ => None,
        }
    }
}

impl fmt::Display for ValType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int => f.write_str("i32"),
            Self::Fix => f.write_str("fix"),
            Self::Bool => f.write_str("bool"),
        }
    }
}

/// A typed 32-bit constant stored in the program constant pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstEntry {
    /// Raw 32-bit constant bits.
    pub value: i32,
    /// Static type enforced by the verifier.
    pub ty: ValType,
}

/// Descriptor for a persistent fixed-length state array backed by state slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateArrayDesc {
    /// Source-level array identifier.
    pub name: String,
    /// Starting index inside the persistent state slot table.
    pub base: u16,
    /// Number of elements in the array.
    pub len: u16,
    /// Element value type.
    pub elem_ty: ValType,
}

/// Metadata describing a compiled function within the bytecode stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMeta {
    /// Function identifier.
    pub name: String,
    /// Starting instruction index (PC) inside `Program::code`.
    pub entry_pc: u32,
    /// Number of instructions belonging to this function.
    pub len: u32,
    /// Parameter types (passed in `r0..rN` on entry).
    pub params: Vec<ValType>,
    /// Optional return type (`None` for void).
    pub ret: Option<ValType>,
    /// Peak register count allocated by the compiler (informational).
    pub num_regs: u8,
}

/// Mapping from a bytecode PC to a 1-based source line number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugLine {
    /// Instruction index in `Program::code`.
    pub pc: u32,
    /// 1-based source line number.
    pub line: u32,
}

/// VM instruction opcodes.
///
/// Format A: `[op:8][a:8][b:8][c:8]`
/// Format B: `[op:8][a:8][imm16:16]`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Opcode {
    /// No operation.
    Nop = 0,
    /// Immediate runtime trap (`TrapReason::ExplicitTrap`).
    Trap = 1,
    /// Format B: `r[a] = (imm16 as i16) as i32` (`Int`).
    LoadImmI = 2,
    /// Format B: `r[a] = if imm16 != 0 { 1 } else { 0 }` (`Bool`).
    LoadImmBool = 3,
    /// Format B: `r[a] = constants[imm16]`.
    LoadConst = 4,
    /// Format A: `r[a] = r[b]` (preserves type of `r[b]`).
    Move = 5,

    /// Format A: `r[a] = r[b].wrapping_add(r[c])` (`Int`).
    AddI = 6,
    /// Format A: `r[a] = r[b].wrapping_sub(r[c])` (`Int`).
    SubI = 7,
    /// Format A: `r[a] = r[b].wrapping_mul(r[c])` (`Int`).
    MulI = 8,
    /// Format A: `r[a] = r[b] / r[c]` (`Int`, traps on zero).
    DivI = 9,
    /// Format A: `r[a] = r[b] % r[c]` (`Int`, traps on zero).
    ModI = 10,
    /// Format A: `r[a] = r[b].wrapping_neg()` (`Int`).
    NegI = 11,

    /// Format A: `r[a] = r[b].saturating_add(r[c])` (`Fix`).
    AddFix = 12,
    /// Format A: `r[a] = r[b].saturating_sub(r[c])` (`Fix`).
    SubFix = 13,
    /// Format A: Q16.16 saturating multiply `r[a] = (r[b] * r[c]) >> 16` (`Fix`).
    MulFix = 14,
    /// Format A: Q16.16 saturating divide `r[a] = (r[b] << 16) / r[c]` (`Fix`, traps on zero).
    DivFix = 15,
    /// Format A: `r[a] = r[b].saturating_neg()` (`Fix`).
    NegFix = 16,

    /// Format A: `r[a] = r[b] & r[c]` (`Int`).
    BitAnd = 17,
    /// Format A: `r[a] = r[b] | r[c]` (`Int`).
    BitOr = 18,
    /// Format A: `r[a] = r[b] ^ r[c]` (`Int`).
    BitXor = 19,
    /// Format A: `r[a] = !r[b]` (`Int` bitwise complement).
    BitNot = 20,
    /// Format A: `r[a] = r[b].wrapping_shl((r[c] as u32) & 31)` (`Int`).
    Shl = 21,
    /// Format A: `r[a] = r[b].wrapping_shr((r[c] as u32) & 31)` (`Int`).
    Shr = 22,

    /// Format A: `r[a] = (r[b] == r[c])` (`Int -> Bool`).
    EqI = 23,
    /// Format A: `r[a] = (r[b] != r[c])` (`Int -> Bool`).
    NeI = 24,
    /// Format A: `r[a] = (r[b] < r[c])` (`Int -> Bool`).
    LtI = 25,
    /// Format A: `r[a] = (r[b] <= r[c])` (`Int -> Bool`).
    LeI = 26,
    /// Format A: `r[a] = (r[b] > r[c])` (`Int -> Bool`).
    GtI = 27,
    /// Format A: `r[a] = (r[b] >= r[c])` (`Int -> Bool`).
    GeI = 28,

    /// Format A: `r[a] = (r[b] == r[c])` (`Fix -> Bool`).
    EqFix = 29,
    /// Format A: `r[a] = (r[b] != r[c])` (`Fix -> Bool`).
    NeFix = 30,
    /// Format A: `r[a] = (r[b] < r[c])` (`Fix -> Bool`).
    LtFix = 31,
    /// Format A: `r[a] = (r[b] <= r[c])` (`Fix -> Bool`).
    LeFix = 32,
    /// Format A: `r[a] = (r[b] > r[c])` (`Fix -> Bool`).
    GtFix = 33,
    /// Format A: `r[a] = (r[b] >= r[c])` (`Fix -> Bool`).
    GeFix = 34,

    /// Format A: `r[a] = (r[b] == r[c])` (`Bool -> Bool`).
    EqBool = 35,
    /// Format A: `r[a] = (r[b] != r[c])` (`Bool -> Bool`).
    NeBool = 36,
    /// Format A: `r[a] = (r[b] == 0) as i32` (`Bool -> Bool`).
    NotBool = 37,

    /// Format A: `r[a] = to_fix(r[b])` (`Int -> Fix`).
    ToFix = 38,
    /// Format A: `r[a] = to_int(r[b])` (`Fix -> Int`).
    ToInt = 39,

    /// Format B: `r[a] = state[imm16]`.
    LoadState = 40,
    /// Format B: `state[imm16] = r[a]`.
    StoreState = 41,
    /// Format A: `r[a] = state[arr[b].base + r[c]]` (bounds checked, traps on OOB).
    LoadStateArr = 42,
    /// Format A: `state[arr[b].base + r[c]] = r[a]` (bounds checked, traps on OOB).
    StoreStateArr = 43,

    /// Format B: `pc = pc + imm16_i16`.
    Jump = 44,
    /// Format B: if `r[a] == 0` then `pc = pc + imm16_i16`.
    JumpIfFalse = 45,
    /// Format B: if `r[a] != 0` then `pc = pc + imm16_i16`.
    JumpIfTrue = 46,

    /// Format A: `r[a] = call func[b](r[c .. c + arity])`.
    Call = 47,
    /// Format A: return from function (`b != 0` returns `r[a]`, `b == 0` is void).
    Ret = 48,
    /// Format A: `r[a] = syscall[b](r[c .. c + arity])`.
    Syscall = 49,
}

impl Opcode {
    /// Converts a raw opcode byte to [`Opcode`].
    #[must_use]
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Nop),
            1 => Some(Self::Trap),
            2 => Some(Self::LoadImmI),
            3 => Some(Self::LoadImmBool),
            4 => Some(Self::LoadConst),
            5 => Some(Self::Move),
            6 => Some(Self::AddI),
            7 => Some(Self::SubI),
            8 => Some(Self::MulI),
            9 => Some(Self::DivI),
            10 => Some(Self::ModI),
            11 => Some(Self::NegI),
            12 => Some(Self::AddFix),
            13 => Some(Self::SubFix),
            14 => Some(Self::MulFix),
            15 => Some(Self::DivFix),
            16 => Some(Self::NegFix),
            17 => Some(Self::BitAnd),
            18 => Some(Self::BitOr),
            19 => Some(Self::BitXor),
            20 => Some(Self::BitNot),
            21 => Some(Self::Shl),
            22 => Some(Self::Shr),
            23 => Some(Self::EqI),
            24 => Some(Self::NeI),
            25 => Some(Self::LtI),
            26 => Some(Self::LeI),
            27 => Some(Self::GtI),
            28 => Some(Self::GeI),
            29 => Some(Self::EqFix),
            30 => Some(Self::NeFix),
            31 => Some(Self::LtFix),
            32 => Some(Self::LeFix),
            33 => Some(Self::GtFix),
            34 => Some(Self::GeFix),
            35 => Some(Self::EqBool),
            36 => Some(Self::NeBool),
            37 => Some(Self::NotBool),
            38 => Some(Self::ToFix),
            39 => Some(Self::ToInt),
            40 => Some(Self::LoadState),
            41 => Some(Self::StoreState),
            42 => Some(Self::LoadStateArr),
            43 => Some(Self::StoreStateArr),
            44 => Some(Self::Jump),
            45 => Some(Self::JumpIfFalse),
            46 => Some(Self::JumpIfTrue),
            47 => Some(Self::Call),
            48 => Some(Self::Ret),
            49 => Some(Self::Syscall),
            _ => None,
        }
    }

    /// Returns the base gas cost for executing this opcode (syscalls add their spec gas).
    #[must_use]
    pub const fn gas_cost(self) -> u32 {
        match self {
            Self::DivI | Self::ModI | Self::MulFix | Self::DivFix => 2,
            Self::LoadStateArr | Self::StoreStateArr | Self::Call | Self::Ret => 3,
            Self::Syscall => 1,
            _ => 1,
        }
    }

    /// Lowercase mnemonic for disassembly.
    #[must_use]
    pub const fn mnemonic(self) -> &'static str {
        match self {
            Self::Nop => "nop",
            Self::Trap => "trap",
            Self::LoadImmI => "ldi",
            Self::LoadImmBool => "ldb",
            Self::LoadConst => "ldc",
            Self::Move => "mov",
            Self::AddI => "addi",
            Self::SubI => "subi",
            Self::MulI => "muli",
            Self::DivI => "divi",
            Self::ModI => "modi",
            Self::NegI => "negi",
            Self::AddFix => "addf",
            Self::SubFix => "subf",
            Self::MulFix => "mulf",
            Self::DivFix => "divf",
            Self::NegFix => "negf",
            Self::BitAnd => "and",
            Self::BitOr => "or",
            Self::BitXor => "xor",
            Self::BitNot => "bnot",
            Self::Shl => "shl",
            Self::Shr => "shr",
            Self::EqI => "eqi",
            Self::NeI => "nei",
            Self::LtI => "lti",
            Self::LeI => "lei",
            Self::GtI => "gti",
            Self::GeI => "gei",
            Self::EqFix => "eqf",
            Self::NeFix => "nef",
            Self::LtFix => "ltf",
            Self::LeFix => "lef",
            Self::GtFix => "gtf",
            Self::GeFix => "gef",
            Self::EqBool => "eqb",
            Self::NeBool => "neb",
            Self::NotBool => "not",
            Self::ToFix => "tofix",
            Self::ToInt => "toint",
            Self::LoadState => "lds",
            Self::StoreState => "sts",
            Self::LoadStateArr => "ldsa",
            Self::StoreStateArr => "stsa",
            Self::Jump => "jmp",
            Self::JumpIfFalse => "jfalse",
            Self::JumpIfTrue => "jtrue",
            Self::Call => "call",
            Self::Ret => "ret",
            Self::Syscall => "sys",
        }
    }
}

/// Fixed-width 32-bit VM instruction word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Instruction(pub u32);

impl Instruction {
    /// Encodes a Format A instruction (`op`, `a`, `b`, `c`).
    #[must_use]
    pub const fn abc(op: Opcode, a: u8, b: u8, c: u8) -> Self {
        Self(((op as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8) | (c as u32))
    }

    /// Encodes a Format B instruction with unsigned 16-bit immediate.
    #[must_use]
    pub const fn ab_imm(op: Opcode, a: u8, imm16: u16) -> Self {
        Self(((op as u32) << 24) | ((a as u32) << 16) | (imm16 as u32))
    }

    /// Encodes a Format B instruction with signed 16-bit immediate.
    #[must_use]
    pub const fn ab_simm(op: Opcode, a: u8, imm16: i16) -> Self {
        Self::ab_imm(op, a, imm16 as u16)
    }

    /// Raw opcode byte.
    #[must_use]
    pub const fn raw_opcode(self) -> u8 {
        ((self.0 >> 24) & 0xFF) as u8
    }

    /// Decoded opcode if valid.
    #[must_use]
    pub const fn opcode(self) -> Option<Opcode> {
        Opcode::from_u8(self.raw_opcode())
    }

    /// Field `a` (bits 16..24).
    #[must_use]
    pub const fn a(self) -> u8 {
        ((self.0 >> 16) & 0xFF) as u8
    }

    /// Field `b` (bits 8..16).
    #[must_use]
    pub const fn b(self) -> u8 {
        ((self.0 >> 8) & 0xFF) as u8
    }

    /// Field `c` (bits 0..8).
    #[must_use]
    pub const fn c(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// Unsigned 16-bit immediate (bits 0..16).
    #[must_use]
    pub const fn imm16(self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }

    /// Signed 16-bit immediate (bits 0..16).
    #[must_use]
    pub const fn imm16_i16(self) -> i16 {
        self.imm16() as i16
    }
}

/// An unverified bytecode program ready for serialization or verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// Program / bot name.
    pub name: String,
    /// Index into `functions` for the per-tick entrypoint (`tick`).
    pub entry_func: u16,
    /// Types of all persistent state slots.
    pub state_layout: Vec<ValType>,
    /// Initial values of all persistent state slots.
    pub state_init: Vec<i32>,
    /// Human-readable names for persistent state slots (empty if stripped).
    pub state_names: Vec<String>,
    /// Descriptors for persistent state arrays backed by `state_layout`.
    pub state_arrays: Vec<StateArrayDesc>,
    /// Constant pool entries.
    pub constants: Vec<ConstEntry>,
    /// Function table.
    pub functions: Vec<FunctionMeta>,
    /// Flat instruction array.
    pub code: Vec<Instruction>,
    /// Source line map (sorted by `pc`).
    pub debug_lines: Vec<DebugLine>,
}

impl Program {
    /// Deterministic 64-bit FNV-1a fingerprint over canonical program bytes.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        let mut buf = Vec::with_capacity(256);
        crate::format::encode_payload_into(self, &mut buf);
        fnv1a_64(&buf)
    }

    /// Looks up the 1-based source line for a bytecode PC, if debug info exists.
    #[must_use]
    pub fn line_for_pc(&self, pc: u32) -> Option<u32> {
        if self.debug_lines.is_empty() {
            return None;
        }
        match self.debug_lines.binary_search_by_key(&pc, |d| d.pc) {
            Ok(idx) => Some(self.debug_lines[idx].line),
            Err(0) => None,
            Err(idx) => Some(self.debug_lines[idx - 1].line),
        }
    }
}

pub(crate) fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
