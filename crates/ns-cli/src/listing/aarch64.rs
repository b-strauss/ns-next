//! AArch64 machine code decoded by `yaxpeax-arm` and written as its
//! `Display` writes it, the decoder `ns-assembler`'s encodings are checked
//! against (`docs/design/developer-tool.md`, decision 4).

use yaxpeax_arch::{Decoder as _, U8Reader};
use yaxpeax_arm::armv8::a64::{InstDecoder, Instruction, Opcode, Operand};

use super::{Decoded, Extent, hex_bytes};

/// Every AArch64 instruction is one 32-bit word.
const INSTRUCTION_BYTES: usize = 4;

/// The instructions of `code`, the first `code.len()` bytes of a body of
/// `extent`, each address in the body named as `extent` names it; a word
/// that decodes to nothing shown in hex.
pub(super) fn decoded(code: &[u8], extent: Extent) -> Vec<Decoded> {
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
                with_body_offsets(&instruction, offset, extent)
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

/// `instruction`'s text, at `offset` in a body of `extent`, its target
/// relative to itself (`$+0x…`) rewritten to the name of the address in
/// the body it reaches, as an x64 listing names it. An `adrp`'s target is
/// a page of the run's own addresses, and stays as it is.
fn with_body_offsets(instruction: &Instruction, offset: u32, extent: Extent) -> String {
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
    let name = u64::try_from(target)
        .ok()
        .and_then(|target| extent.name(target));
    match name {
        Some(name) => text.replacen(&operand.to_string(), &name, 1),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A branch forward names the offset it reaches, and a literal load
    /// of a data word the word's offset from the first.
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
        let extent = Extent {
            length: 24,
            data_start: 16,
        };
        let lines: Vec<String> = decoded(&code, extent)
            .into_iter()
            .map(|instruction| instruction.text)
            .collect();
        assert_eq!(lines[0], "b +0x0008", "{lines:?}");
        assert_eq!(lines[1], "ret", "{lines:?}");
        assert!(lines[2].ends_with(" data+0x0000"), "{lines:?}");
    }
}
