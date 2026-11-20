//! x64 machine code decoded by `iced-x86` and written in Intel syntax,
//! the decoder `ns-assembler`'s encodings are checked against
//! (`docs/design/developer-tool.md`, decision 4).

use iced_x86::{
    Decoder, DecoderOptions, Formatter as _, Instruction, IntelFormatter, OpKind, SymbolResolver,
    SymbolResult,
};

use super::{Decoded, body_offset};

/// Decoding with the instruction pointer at 0 makes every address the
/// decoder computes -- a branch's target, a `rip`-relative operand's -- an
/// offset in the body.
const BODY_START: u64 = 0;

/// The decoder's mode: 64-bit code.
const BITNESS: u32 = 64;

/// The instructions of `code`, the first `code.len()` bytes of a body of
/// `body_length` bytes, each target in the body named by its offset.
pub(super) fn decoded(code: &[u8], body_length: usize) -> Vec<Decoded> {
    let resolver = BodyOffsets {
        length: u64::try_from(body_length).unwrap_or(u64::MAX),
    };
    let mut formatter = IntelFormatter::with_options(Some(Box::new(resolver)), None);
    let mut decoder = Decoder::with_ip(BITNESS, code, BODY_START, DecoderOptions::NONE);
    let mut instructions = Vec::new();
    while decoder.can_decode() {
        let instruction = decoder.decode();
        let mut text = String::new();
        formatter.format(&instruction, &mut text);
        instructions.push(Decoded {
            offset: u32::try_from(instruction.ip()).unwrap_or(u32::MAX),
            length: u32::try_from(instruction.len()).unwrap_or(u32::MAX),
            text,
        });
    }
    instructions
}

/// Names an address in the body by its offset, where a branch reaches it
/// or a `rip`-relative operand reads it: the address the decoder computes
/// from 0 is the offset, and the run's own address would differ in every
/// run. Every other number -- an immediate, a displacement from a
/// register -- stays the decoder's.
struct BodyOffsets {
    length: u64,
}

impl SymbolResolver for BodyOffsets {
    fn symbol(
        &mut self,
        instruction: &Instruction,
        _operand: u32,
        instruction_operand: Option<u32>,
        address: u64,
        _address_size: u32,
    ) -> Option<SymbolResult<'_>> {
        let operand = instruction_operand?;
        let in_the_body = match instruction.op_kind(operand) {
            OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64 => true,
            OpKind::Memory => instruction.is_ip_rel_memory_operand(),
            _ => false,
        };
        (in_the_body && address < self.length)
            .then(|| SymbolResult::with_string(address, body_offset(address)))
    }
}
