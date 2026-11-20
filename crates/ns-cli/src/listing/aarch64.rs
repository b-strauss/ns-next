//! AArch64 machine code decoded by `yaxpeax-arm` and written as its
//! `Display` writes it, the decoder `ns-assembler`'s encodings are checked
//! against (`docs/design/developer-tool.md`, decision 4).

use yaxpeax_arch::{Decoder as _, U8Reader};
use yaxpeax_arm::armv8::a64::{InstDecoder, Instruction, Opcode, Operand};

use super::{Decoded, body_offset, hex_bytes};

/// Every AArch64 instruction is one 32-bit word.
const INSTRUCTION_BYTES: usize = 4;

/// The instructions of `code`, the first `code.len()` bytes of a body of
/// `body_length` bytes, each target in the body named by its offset; a
/// word that decodes to nothing shown in hex.
pub(super) fn decoded(code: &[u8], body_length: usize) -> Vec<Decoded> {
    let decoder = InstDecoder::default();
    code.chunks(INSTRUCTION_BYTES)
        .zip((0u32..).step_by(INSTRUCTION_BYTES))
        .map(|(word, offset)| {
            let mut instruction = Instruction::default();
            let decodes = word.len() == INSTRUCTION_BYTES
                && decoder
                    .decode_into(&mut instruction, &mut U8Reader::new(word))
                    .is_ok();
            let text = if decodes {
                with_body_offsets(&instruction, offset, body_length)
            } else {
                hex_bytes(word)
            };
            Decoded {
                offset,
                length: u32::try_from(word.len()).unwrap_or(u32::MAX),
                text,
            }
        })
        .collect()
}

/// `instruction`'s text, at `offset` in a body of `body_length` bytes,
/// its target relative to itself (`$+0x…`) rewritten to the offset in
/// the body it reaches, as an x64 listing names it. An `adrp`'s target is
/// a page of the run's own addresses, and stays as it is.
fn with_body_offsets(instruction: &Instruction, offset: u32, body_length: usize) -> String {
    let text = instruction.to_string();
    if instruction.opcode == Opcode::ADRP {
        return text;
    }
    // No instruction has more than one operand relative to itself.
    let Some((operand, relative)) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            Operand::PCOffset(relative) => Some((operand, *relative)),
            _ => None,
        })
    else {
        return text;
    };
    let target = i64::from(offset).saturating_add(relative);
    let in_the_body = u64::try_from(target)
        .is_ok_and(|target| target < u64::try_from(body_length).unwrap_or(u64::MAX));
    if in_the_body {
        text.replacen(&operand.to_string(), &body_offset(target), 1)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A branch forward and a literal load into the body name the offsets
    /// they reach.
    #[test]
    fn a_target_in_the_body_is_named_by_its_offset() {
        let code: Vec<u8> = [
            0x1400_0002_u32, // b to 0x8
            0xD65F_03C0,     // ret
            0x5800_0040,     // ldr x0, the literal at 0x10
            0xD65F_03C0,     // ret
        ]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
        let lines: Vec<String> = decoded(&code, 24)
            .into_iter()
            .map(|instruction| instruction.text)
            .collect();
        assert_eq!(lines[0], "b +0x0008", "{lines:?}");
        assert_eq!(lines[1], "ret", "{lines:?}");
        assert!(lines[2].ends_with("+0x0010"), "{lines:?}");
    }
}
