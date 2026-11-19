//! Binary `.phxb` bytecode container serializer and bounds-checked decoder.

use core::fmt;

use crate::crc32c::crc32c;
use crate::isa::{
    ConstEntry, DebugLine, FunctionMeta, Instruction, MAX_CONSTANTS, MAX_FUNCTIONS,
    MAX_INSTRUCTIONS, MAX_REGS, MAX_STATE_ARRAYS, MAX_STATE_SLOTS, Program, StateArrayDesc,
    ValType,
};

/// 4-byte magic header identifying Phalanx compiled bytecode containers.
pub const MAGIC: &[u8; 4] = b"PHXB";
/// Binary container format version.
pub const FORMAT_VERSION: u16 = 1;
const MAX_NAME_LEN: usize = 256;

/// Errors produced when encoding or decoding `.phxb` containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Input buffer is shorter than the minimum header + CRC footer.
    Truncated,
    /// Magic header does not match `b"PHXB"`.
    InvalidMagic,
    /// Unsupported container version.
    UnsupportedVersion(u16),
    /// Castagnoli CRC32C mismatch indicating corruption or tampering.
    ChecksumMismatch {
        /// Checksum recorded in the container footer.
        expected: u32,
        /// Recomputed checksum over header + payload.
        actual: u32,
    },
    /// Invalid UTF-8 string in metadata.
    InvalidUtf8,
    /// Invalid [`ValType`] discriminant.
    InvalidValType(u8),
    /// Declared section count exceeds safety limits.
    LimitExceeded(&'static str),
    /// Trailing unparsed bytes before the CRC footer.
    TrailingBytes,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("unexpected end of .phxb stream"),
            Self::InvalidMagic => f.write_str("invalid .phxb magic bytes"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported .phxb version {v}"),
            Self::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "CRC32C mismatch: expected 0x{expected:08x}, got 0x{actual:08x}"
                )
            }
            Self::InvalidUtf8 => f.write_str("invalid UTF-8 string in .phxb metadata"),
            Self::InvalidValType(v) => write!(f, "invalid value type tag {v}"),
            Self::LimitExceeded(what) => write!(f, "container limit exceeded for {what}"),
            Self::TrailingBytes => f.write_str("unexpected trailing bytes in .phxb payload"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Encodes a [`Program`] into a CRC32C-protected `.phxb` binary container.
pub fn encode_program(prog: &Program) -> Result<Vec<u8>, CodecError> {
    if prog.state_layout.len() > MAX_STATE_SLOTS || prog.state_init.len() != prog.state_layout.len()
    {
        return Err(CodecError::LimitExceeded("state_slots"));
    }
    if prog.state_arrays.len() > MAX_STATE_ARRAYS {
        return Err(CodecError::LimitExceeded("state_arrays"));
    }
    if prog.constants.len() > MAX_CONSTANTS {
        return Err(CodecError::LimitExceeded("constants"));
    }
    if prog.functions.len() > MAX_FUNCTIONS {
        return Err(CodecError::LimitExceeded("functions"));
    }
    if prog.code.len() > MAX_INSTRUCTIONS {
        return Err(CodecError::LimitExceeded("instructions"));
    }

    let mut out = Vec::with_capacity(256 + prog.code.len() * 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    encode_payload_into(prog, &mut out);
    let checksum = crc32c(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

pub(crate) fn encode_payload_into(prog: &Program, out: &mut Vec<u8>) {
    write_str(out, &prog.name);
    out.extend_from_slice(&prog.entry_func.to_le_bytes());

    let state_len = prog.state_layout.len().min(u16::MAX as usize) as u16;
    out.extend_from_slice(&state_len.to_le_bytes());
    for i in 0..usize::from(state_len) {
        out.push(prog.state_layout[i] as u8);
        let init = prog.state_init.get(i).copied().unwrap_or(0);
        out.extend_from_slice(&init.to_le_bytes());
        let name = prog.state_names.get(i).map(String::as_str).unwrap_or("");
        write_str(out, name);
    }

    let arr_len = prog.state_arrays.len().min(u16::MAX as usize) as u16;
    out.extend_from_slice(&arr_len.to_le_bytes());
    for arr in prog.state_arrays.iter().take(usize::from(arr_len)) {
        write_str(out, &arr.name);
        out.extend_from_slice(&arr.base.to_le_bytes());
        out.extend_from_slice(&arr.len.to_le_bytes());
        out.push(arr.elem_ty as u8);
    }

    let const_len = prog.constants.len().min(u16::MAX as usize) as u16;
    out.extend_from_slice(&const_len.to_le_bytes());
    for c in prog.constants.iter().take(usize::from(const_len)) {
        out.extend_from_slice(&c.value.to_le_bytes());
        out.push(c.ty as u8);
    }

    let fn_len = prog.functions.len().min(u16::MAX as usize) as u16;
    out.extend_from_slice(&fn_len.to_le_bytes());
    for func in prog.functions.iter().take(usize::from(fn_len)) {
        write_str(out, &func.name);
        out.extend_from_slice(&func.entry_pc.to_le_bytes());
        out.extend_from_slice(&func.len.to_le_bytes());
        let param_len = func.params.len().min(MAX_REGS) as u8;
        out.push(param_len);
        for &p in func.params.iter().take(usize::from(param_len)) {
            out.push(p as u8);
        }
        match func.ret {
            None => out.push(0),
            Some(ty) => {
                out.push(1);
                out.push(ty as u8);
            }
        }
        out.push(func.num_regs);
    }

    let code_len = prog.code.len().min(u32::MAX as usize) as u32;
    out.extend_from_slice(&code_len.to_le_bytes());
    for insn in prog.code.iter().take(code_len as usize) {
        out.extend_from_slice(&insn.0.to_le_bytes());
    }

    let dbg_len = prog.debug_lines.len().min(u32::MAX as usize) as u32;
    out.extend_from_slice(&dbg_len.to_le_bytes());
    for dbg in prog.debug_lines.iter().take(dbg_len as usize) {
        out.extend_from_slice(&dbg.pc.to_le_bytes());
        out.extend_from_slice(&dbg.line.to_le_bytes());
    }
}

/// Decodes and integrity-checks a `.phxb` bytecode container.
pub fn decode_program(bytes: &[u8]) -> Result<Program, CodecError> {
    if bytes.len() < 10 {
        return Err(CodecError::Truncated);
    }
    if &bytes[0..4] != MAGIC {
        return Err(CodecError::InvalidMagic);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != FORMAT_VERSION {
        return Err(CodecError::UnsupportedVersion(version));
    }
    let split = bytes.len() - 4;
    let expected = u32::from_le_bytes([
        bytes[split],
        bytes[split + 1],
        bytes[split + 2],
        bytes[split + 3],
    ]);
    let actual = crc32c(&bytes[..split]);
    if expected != actual {
        return Err(CodecError::ChecksumMismatch { expected, actual });
    }

    let mut cur = Reader::new(&bytes[6..split]);
    let name = cur.read_str()?;
    let entry_func = cur.read_u16()?;

    let state_len = usize::from(cur.read_u16()?);
    if state_len > MAX_STATE_SLOTS {
        return Err(CodecError::LimitExceeded("state_slots"));
    }
    let mut state_layout = Vec::with_capacity(state_len);
    let mut state_init = Vec::with_capacity(state_len);
    let mut state_names = Vec::with_capacity(state_len);
    for _ in 0..state_len {
        state_layout.push(cur.read_val_type()?);
        state_init.push(cur.read_i32()?);
        state_names.push(cur.read_str()?);
    }

    let arr_len = usize::from(cur.read_u16()?);
    if arr_len > MAX_STATE_ARRAYS {
        return Err(CodecError::LimitExceeded("state_arrays"));
    }
    let mut state_arrays = Vec::with_capacity(arr_len);
    for _ in 0..arr_len {
        state_arrays.push(StateArrayDesc {
            name: cur.read_str()?,
            base: cur.read_u16()?,
            len: cur.read_u16()?,
            elem_ty: cur.read_val_type()?,
        });
    }

    let const_len = usize::from(cur.read_u16()?);
    if const_len > MAX_CONSTANTS {
        return Err(CodecError::LimitExceeded("constants"));
    }
    let mut constants = Vec::with_capacity(const_len);
    for _ in 0..const_len {
        constants.push(ConstEntry {
            value: cur.read_i32()?,
            ty: cur.read_val_type()?,
        });
    }

    let fn_len = usize::from(cur.read_u16()?);
    if fn_len > MAX_FUNCTIONS {
        return Err(CodecError::LimitExceeded("functions"));
    }
    let mut functions = Vec::with_capacity(fn_len);
    for _ in 0..fn_len {
        let fn_name = cur.read_str()?;
        let entry_pc = cur.read_u32()?;
        let len = cur.read_u32()?;
        let param_len = usize::from(cur.read_u8()?);
        if param_len > MAX_REGS {
            return Err(CodecError::LimitExceeded("function_params"));
        }
        let mut params = Vec::with_capacity(param_len);
        for _ in 0..param_len {
            params.push(cur.read_val_type()?);
        }
        let has_ret = cur.read_u8()?;
        let ret = if has_ret == 0 {
            None
        } else {
            Some(cur.read_val_type()?)
        };
        let num_regs = cur.read_u8()?;
        functions.push(FunctionMeta {
            name: fn_name,
            entry_pc,
            len,
            params,
            ret,
            num_regs,
        });
    }

    let code_len = cur.read_u32()? as usize;
    if code_len > MAX_INSTRUCTIONS {
        return Err(CodecError::LimitExceeded("instructions"));
    }
    let mut code = Vec::with_capacity(code_len);
    for _ in 0..code_len {
        code.push(Instruction(cur.read_u32()?));
    }

    let dbg_len = cur.read_u32()? as usize;
    if dbg_len > MAX_INSTRUCTIONS {
        return Err(CodecError::LimitExceeded("debug_lines"));
    }
    let mut debug_lines = Vec::with_capacity(dbg_len);
    for _ in 0..dbg_len {
        debug_lines.push(DebugLine {
            pc: cur.read_u32()?,
            line: cur.read_u32()?,
        });
    }

    if cur.remaining() != 0 {
        return Err(CodecError::TrailingBytes);
    }

    Ok(Program {
        name,
        entry_func,
        state_layout,
        state_init,
        state_names,
        state_arrays,
        constants,
        functions,
        code,
        debug_lines,
    })
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len().min(MAX_NAME_LEN) as u16;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&bytes[..usize::from(len)]);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        let end = self.pos.checked_add(n).ok_or(CodecError::Truncated)?;
        if end > self.buf.len() {
            return Err(CodecError::Truncated);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, CodecError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, CodecError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i32(&mut self) -> Result<i32, CodecError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_val_type(&mut self) -> Result<ValType, CodecError> {
        let raw = self.read_u8()?;
        ValType::from_u8(raw).ok_or(CodecError::InvalidValType(raw))
    }

    fn read_str(&mut self) -> Result<String, CodecError> {
        let len = usize::from(self.read_u16()?);
        if len > MAX_NAME_LEN {
            return Err(CodecError::LimitExceeded("string_length"));
        }
        let raw = self.take(len)?;
        std::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| CodecError::InvalidUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isa::{Opcode, Program};

    fn sample_program() -> Program {
        Program {
            name: "sample".into(),
            entry_func: 0,
            state_layout: vec![ValType::Int],
            state_init: vec![7],
            state_names: vec!["counter".into()],
            state_arrays: vec![],
            constants: vec![ConstEntry {
                value: 65536,
                ty: ValType::Fix,
            }],
            functions: vec![FunctionMeta {
                name: "tick".into(),
                entry_pc: 0,
                len: 2,
                params: vec![],
                ret: None,
                num_regs: 1,
            }],
            code: vec![
                Instruction::ab_imm(Opcode::LoadImmI, 0, 1),
                Instruction::abc(Opcode::Ret, 0, 0, 0),
            ],
            debug_lines: vec![DebugLine { pc: 0, line: 3 }],
        }
    }

    #[test]
    fn roundtrip_and_tamper_detection() {
        let prog = sample_program();
        let mut bytes = encode_program(&prog).expect("encode");
        let decoded = decode_program(&bytes).expect("decode");
        assert_eq!(prog, decoded);

        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xFF;
        assert!(matches!(
            decode_program(&bytes),
            Err(CodecError::ChecksumMismatch { .. })
        ));
    }
}
