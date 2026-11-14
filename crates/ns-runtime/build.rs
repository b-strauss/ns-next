//! The shipped code's fingerprints and the embedded artifact
//! (`docs/design/core-library.md`, "The embedded shipped code").
//!
//! The code's fingerprint covers everything that can change what the
//! shipped modules compile to -- their sources, the Rust sources of
//! `ns-syntax` and `ns-runtime`, this script and both manifests -- so an
//! artifact a compiler change made stale is never decoded: it invalidates
//! more often than needed, and the boot then compiles from source until the
//! artifact is regenerated. The second, over the shipped modules' sources
//! alone, goes into an image's fingerprint, which must not change with
//! every Rust edit. Both take the shipped modules' sources -- `Kernel.ns`,
//! `PlatformFactory.ns` and `platform/` -- and not `Manifest.ns`, which a
//! program compiles as a module of its own, so an edit to it changes
//! neither the shipped code nor the shipped world.

use std::fs::{read, write};
use std::path::PathBuf;

// The one computation of the fingerprints, which `cargo xtask gates` runs
// too, to judge the artifact before it builds anything.
#[path = "src/shipped_code/fingerprint.rs"]
mod fingerprint;

const ARTIFACT: &str = "core-library/shipped-code.bin";

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let workspace = manifest.join("../..");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by Cargo"));

    let fingerprints =
        fingerprint::compute(&workspace).expect("the fingerprinted sources are readable");
    write(
        out.join("fingerprints.rs"),
        format!(
            "pub(crate) const CORE_LIBRARY_FINGERPRINT: u64 = {:#x};\n\
             pub(crate) const SHIPPED_CODE_FINGERPRINT: u64 = {:#x};\n",
            fingerprints.library, fingerprints.code,
        ),
    )
    .expect("OUT_DIR is writable");

    // A tree without the artifact -- a fresh clone -- embeds an empty one
    // and boots from source.
    let artifact = manifest.join(ARTIFACT);
    let bytes = read(&artifact).unwrap_or_default();
    write(out.join("shipped-code.bin"), bytes).expect("OUT_DIR is writable");

    // `ns_jit`: this target has a JIT back end, so the JIT's machinery
    // and the heap's accessors for machine code are compiled; elsewhere
    // every body stays interpreted (`jit::available`). x64 has one on
    // every system, AArch64 on Linux and Apple's platforms; Windows on
    // ARM stays interpreted, its calling convention and unwinding being
    // ones no stub crosses. It says nothing about which encoder: the
    // encoders are named only in the macro assembler, whose ABI decides
    // the roles and calling conventions, and by the tests that write an
    // architecture's registers by name, which keep a `target_arch` gate
    // of their own, since they are that architecture's code on any
    // target the JIT runs on.
    println!("cargo::rustc-check-cfg=cfg(ns_jit)");
    let architecture = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let system = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let has_a_back_end = architecture == "x86_64"
        || (architecture == "aarch64" && (system == "linux" || system == "macos"));
    if has_a_back_end {
        println!("cargo::rustc-cfg=ns_jit");
    }

    // Directories, not the artifact itself: Cargo reruns a script whose
    // named file is missing on every build, and a directory's scan sees the
    // artifact appear.
    println!("cargo:rerun-if-changed=core-library");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../ns-syntax/src");
    println!("cargo:rerun-if-changed=../ns-syntax/Cargo.toml");
}
