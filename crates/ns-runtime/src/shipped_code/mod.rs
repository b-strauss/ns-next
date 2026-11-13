//! The shipped core-library modules' compiled mixins, as bytes the binary
//! embeds (`docs/design/core-library.md`, "The embedded shipped code").
//! A codec of its own rather than the image's: the image decoder reads
//! untrusted input and refuses a mirror send (the VM mirror's lock (6):
//! `SendMirror` has no wire tag), while these bytes are the binary's own
//! and carry the kernel's mirror sends. The two share nothing.
//!
//! **The format.** `MAGIC`, `FORMAT_VERSION` (`u16`), the fingerprint
//! (`u64`), the module count (`u32`), then per module its name (`str`), its
//! section's byte length (`u32`) and the section, `Kernel` first and then
//! the `SHIPPED_MODULES` in order. A section is the symbol count (`u32`)
//! and each symbol (`str`), in order of first use, then the root mixin with
//! every nested one. A symbol in a mixin or a body is its `u32` index in its
//! section's table. Numbers are little-endian and fixed-width, a `str` its
//! `u32` byte length and its UTF-8, an `Option` a `0`/`1` byte and the
//! value. Its length lets a section be skipped and decoded on its own.
//!
//! **No verification.** Bytes that do not decode are a defect of the
//! binary, as a shipped module that does not parse is, and panic; the
//! decoder has no error type.

use std::rc::Rc;

use crate::compiler::{primitive_method, Lowering};
use crate::interpreter::core_library::{KERNEL_MODULE, SHIPPED_MODULES};
use crate::interpreter::Interpreter;
use crate::mixin::Mixin;

mod bytes;
mod decode;
mod encode;

pub(crate) use encode::encode;

use bytes::ByteReader;

pub(crate) const MAGIC: [u8; 4] = *b"NSSC";
pub(crate) const FORMAT_VERSION: u16 = 7;

// `SHIPPED_CODE_FINGERPRINT` is the fingerprint of the sources and the
// compiler the artifact was generated from, which the boot compares against
// the binary's own; `CORE_LIBRARY_FINGERPRINT` covers the shipped modules'
// sources alone.
include!(concat!(env!("OUT_DIR"), "/fingerprints.rs"));

/// The artifact `cargo xtask shipped-code` wrote, embedded when the crate
/// was built; empty when the tree had none.
pub(crate) fn embedded() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/shipped-code.bin"))
}

/// Where a boot takes the shipped modules' compiled mixins from: the
/// artifact the binary embeds, or their source, compiled and checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShippedCodePath {
    Embedded,
    Source,
}

/// Whether `bytes` is an artifact this binary may decode: one written for
/// exactly the sources it was built from.
pub(crate) fn usable(bytes: &[u8]) -> bool {
    // The magic, the format version and the fingerprint.
    let header_length = MAGIC.len() + size_of::<u16>() + size_of::<u64>();
    if bytes.len() < header_length {
        return false;
    }
    let mut reader = ByteReader::new(bytes);
    reader.raw(MAGIC.len()) == MAGIC
        && reader.u16() == FORMAT_VERSION
        && reader.u64() == SHIPPED_CODE_FINGERPRINT
}

/// Where a boot takes the shipped modules' mixins from. Only the optimized
/// lowering's code is embedded, and primitive methods are decided when a
/// body is compiled, so the reference lowering and a boot without them
/// compile from source -- the reference lowering stays an oracle that
/// compiles everything afresh. A missing or stale artifact falls back
/// silently: a warning would change every golden's `stderr`, and the gates
/// regenerate the artifact before they test.
pub(crate) fn path_for(
    lowering: Lowering,
    primitive_methods_disabled: bool,
    bytes: &[u8],
) -> ShippedCodePath {
    if lowering == Lowering::Optimized && !primitive_methods_disabled && usable(bytes) {
        ShippedCodePath::Embedded
    } else {
        ShippedCodePath::Source
    }
}

/// The shipped modules' names, in the artifact's order: `Kernel`, then
/// every `SHIPPED_MODULES` name.
pub(crate) fn module_names() -> impl Iterator<Item = &'static str> {
    std::iter::once(KERNEL_MODULE).chain(SHIPPED_MODULES.iter().map(|shipped| shipped.name))
}

/// The artifact: every shipped module compiled through the source path,
/// checks included, with the optimized lowering, then encoded. For
/// `cargo xtask shipped-code`. It needs the root mixins, not the world they
/// build, so it compiles each module and instantiates none.
///
/// Refuses, answering why, under `NS_NEXT_NO_PRIMITIVE_METHODS=1`: the
/// fingerprint covers the sources, not the process's environment, so an
/// artifact compiled without primitive methods would carry the binary's
/// fingerprint and every later optimized boot would decode a kernel without
/// them, silently.
#[doc(hidden)]
pub fn generate() -> Result<Vec<u8>, &'static str> {
    refuse_without_primitive_methods(primitive_method::disabled())?;
    let mut interpreter = Interpreter::bootstrap_on(Lowering::Optimized, ShippedCodePath::Source);
    let modules: Vec<(&'static str, Rc<Mixin>)> = interpreter.enter(|mutator| {
        module_names()
            .map(|name| (name, mutator.shipped_root_mixin_from_source(name)))
            .collect()
    });
    Ok(encode(&modules, SHIPPED_CODE_FINGERPRINT))
}

/// `generate`'s refusal, given whether primitive methods are disabled; a
/// function of its own because the process reads the variable once, so no
/// test of the suite could reach the refusal through `generate`.
fn refuse_without_primitive_methods(primitive_methods_disabled: bool) -> Result<(), &'static str> {
    if primitive_methods_disabled {
        Err(GENERATING_WITHOUT_PRIMITIVE_METHODS)
    } else {
        Ok(())
    }
}

/// Why `generate` refuses under `NS_NEXT_NO_PRIMITIVE_METHODS=1`.
const GENERATING_WITHOUT_PRIMITIVE_METHODS: &str =
    "NS_NEXT_NO_PRIMITIVE_METHODS=1 is set: the shipped code is compiled with primitive \
     methods, so it is not generated under it; unset the variable and run again";

/// The root mixin of the module `name` in the artifact `bytes`, every
/// mixin of it sharing `module_text`, the module's text.
pub(crate) fn decode_module(bytes: &[u8], name: &str, module_text: Rc<str>) -> Rc<Mixin> {
    let mut reader = ByteReader::new(bytes);
    assert!(
        reader.raw(MAGIC.len()) == MAGIC,
        "internal: the embedded shipped code has no magic"
    );
    let version = reader.u16();
    assert!(
        version == FORMAT_VERSION,
        "internal: the embedded shipped code is format {version}, not {FORMAT_VERSION}"
    );
    // The fingerprint, which the boot compares before it decodes a module.
    reader.u64();
    for _ in 0..reader.length() {
        let module = reader.str();
        let length = reader.length();
        let section = reader.raw(length);
        if module == name {
            return decode::decode_section(section, module_text);
        }
    }
    panic!("internal: the embedded shipped code holds no module {name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{CompiledCode, Instruction};

    fn decoded_modules(bytes: &[u8]) -> Vec<(&'static str, Rc<Mixin>)> {
        module_names()
            .map(|name| {
                let module_text = crate::interpreter::core_library::shipped_module_text(name);
                (name, decode_module(bytes, name, module_text))
            })
            .collect()
    }

    fn header(fingerprint: u64) -> Vec<u8> {
        let mut writer = bytes::ByteWriter::new();
        writer.raw(&MAGIC);
        writer.u16(FORMAT_VERSION);
        writer.u64(fingerprint);
        writer.u32(0);
        writer.into_bytes()
    }

    #[test]
    fn an_empty_artifact_is_not_used() {
        assert_eq!(
            path_for(Lowering::Optimized, false, &[]),
            ShippedCodePath::Source
        );
    }

    #[test]
    fn an_artifact_with_another_fingerprint_is_not_used() {
        let stale = header(SHIPPED_CODE_FINGERPRINT ^ 1);
        assert_eq!(
            path_for(Lowering::Optimized, false, &stale),
            ShippedCodePath::Source
        );
    }

    #[test]
    fn only_the_optimized_lowering_with_primitive_methods_uses_the_artifact() {
        let current = header(SHIPPED_CODE_FINGERPRINT);
        assert_eq!(
            path_for(Lowering::Optimized, false, &current),
            ShippedCodePath::Embedded
        );
        assert_eq!(
            path_for(Lowering::Reference, false, &current),
            ShippedCodePath::Source
        );
        assert_eq!(
            path_for(Lowering::Optimized, true, &current),
            ShippedCodePath::Source
        );
    }

    /// The artifact `generate` writes; a test that needs one cannot run
    /// under `NS_NEXT_NO_PRIMITIVE_METHODS=1`, and says so.
    fn generated() -> Vec<u8> {
        generate().unwrap_or_else(|refusal| panic!("{refusal}"))
    }

    #[test]
    fn the_optimized_boot_decodes_the_embedded_code() {
        assert!(
            !primitive_method::disabled(),
            "NS_NEXT_NO_PRIMITIVE_METHODS=1 is set: an optimized boot compiles the shipped code \
             from source under it"
        );
        assert!(
            usable(embedded()),
            "the embedded shipped code is missing or stale: run `cargo xtask shipped-code`"
        );
        let mut interpreter = Interpreter::bootstrap();
        let path = interpreter.enter(|mutator| mutator.shipped_code_path);
        assert_eq!(path, ShippedCodePath::Embedded);
    }

    #[test]
    fn the_reference_boot_compiles_from_source() {
        let mut interpreter = Interpreter::bootstrap_with(Lowering::Reference);
        let path = interpreter.enter(|mutator| mutator.shipped_code_path);
        assert_eq!(path, ShippedCodePath::Source);
    }

    #[test]
    fn the_embedded_shipped_code_round_trips() {
        let bytes = embedded();
        assert!(usable(bytes), "run `cargo xtask shipped-code`");
        let decoded = decoded_modules(bytes);
        assert_eq!(encode(&decoded, SHIPPED_CODE_FINGERPRINT), bytes);
    }

    /// Asserts, for `code` and every block it creates, that the flag the
    /// builder derived off the encoding is what `addresses_own_slots_of`
    /// answers over the decoded view; returns how many bodies were true.
    fn count_addressing_own_slots(code: &CompiledCode) -> usize {
        let instructions = code.instructions();
        assert_eq!(
            code.addresses_own_slots,
            crate::bytecode::addresses_own_slots_of(&instructions)
        );
        let blocks: usize = instructions
            .iter()
            .map(|instruction| match instruction {
                Instruction::MakeClosure { code, .. } => count_addressing_own_slots(code),
                _ => 0,
            })
            .sum();
        usize::from(code.addresses_own_slots) + blocks
    }

    fn count_in_mixin(mixin: &Mixin) -> usize {
        let own = mixin
            .initializer_code
            .iter()
            .chain(&mixin.own_initializer_code)
            .chain(&mixin.standalone_initializer_code)
            .chain(&mixin.superclass_clause_code)
            .map(|code| count_addressing_own_slots(code))
            .chain(
                mixin
                    .compiled_methods()
                    .map(|method| count_addressing_own_slots(&method.code)),
            )
            .sum::<usize>();
        own + mixin
            .declared_nested()
            .iter()
            .chain(&mixin.class_side)
            .map(|nested| count_in_mixin(nested))
            .sum::<usize>()
    }

    /// The flag is exact for every body of the shipped world as the
    /// embedded artifact decodes it.
    #[test]
    fn the_flag_is_exact_for_every_body_of_the_shipped_modules() {
        let mut total = 0;
        for (_, root) in decoded_modules(&generated()) {
            total += count_in_mixin(&root);
        }
        assert!(total > 0, "no shipped body addresses its own slots");
    }

    #[test]
    fn the_two_fingerprints_differ() {
        // The code's covers the Rust sources too; equal, it would cover the
        // core library alone and a compiler change would leave a stale
        // artifact in use.
        assert_ne!(SHIPPED_CODE_FINGERPRINT, CORE_LIBRARY_FINGERPRINT);
    }

    #[test]
    fn generated_shipped_code_round_trips() {
        let generated = generated();
        let decoded = decoded_modules(&generated);
        assert_eq!(encode(&decoded, SHIPPED_CODE_FINGERPRINT), generated);
    }

    #[test]
    fn generating_without_primitive_methods_is_refused() {
        assert_eq!(
            refuse_without_primitive_methods(true),
            Err(GENERATING_WITHOUT_PRIMITIVE_METHODS)
        );
        assert_eq!(refuse_without_primitive_methods(false), Ok(()));
    }

    /// Asserts that the kernel's `Object` in the artifact `bytes` keeps its
    /// mirror sends and its primitive methods' fast paths, and that the
    /// kernel root is marked shipped and carries no declaration.
    fn assert_keeps_the_mirror_sends_and_fast_paths(bytes: &[u8]) {
        let kernel = decode_module(
            bytes,
            "Kernel",
            crate::interpreter::core_library::shipped_module_text("Kernel"),
        );
        let object = kernel
            .declared_nested()
            .iter()
            .find(|nested| nested.name.as_str() == "Object")
            .cloned()
            .expect("Kernel.ns declares Object");
        let mut mirror_sends = 0;
        let mut fast_paths = 0;
        for method in object.compiled_methods() {
            fast_paths += usize::from(method.recognized_fast_path().is_some());
            mirror_sends += method
                .code
                .instructions()
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::SendMirror { .. }))
                .count();
        }
        assert!(mirror_sends > 0, "the kernel's Object has no mirror send");
        assert!(
            fast_paths > 0,
            "the kernel's Object has no primitive method: was the artifact generated under \
             NS_NEXT_NO_PRIMITIVE_METHODS=1?"
        );
        assert!(kernel.shipped && kernel.declaration.is_none());
    }

    #[test]
    fn decoded_shipped_code_keeps_the_mirror_sends_and_fast_paths() {
        assert_keeps_the_mirror_sends_and_fast_paths(&generated());
    }

    #[test]
    fn the_embedded_shipped_code_keeps_the_mirror_sends_and_fast_paths() {
        let bytes = embedded();
        assert!(usable(bytes), "run `cargo xtask shipped-code`");
        assert_keeps_the_mirror_sends_and_fast_paths(bytes);
    }

    #[test]
    fn generated_shipped_code_is_reproducible() {
        assert_eq!(generated(), generated());
    }
}
