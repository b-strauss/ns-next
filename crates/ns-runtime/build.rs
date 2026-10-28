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

use std::fs::{read, read_dir, write};
use std::path::{Path, PathBuf};

const ARTIFACT: &str = "core-library/shipped-code.bin";

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let workspace = manifest.join("../..");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by Cargo"));

    let mut library = vec![
        manifest.join("core-library/Kernel.ns"),
        manifest.join("core-library/PlatformFactory.ns"),
    ];
    library.extend(files_under(&manifest.join("core-library/platform"), "ns"));
    let mut code = library.clone();
    code.extend(files_under(&manifest.join("src"), "rs"));
    code.extend(files_under(&workspace.join("crates/ns-syntax/src"), "rs"));
    code.push(manifest.join("build.rs"));
    code.push(manifest.join("Cargo.toml"));
    code.push(workspace.join("crates/ns-syntax/Cargo.toml"));

    write(
        out.join("fingerprints.rs"),
        format!(
            "pub(crate) const CORE_LIBRARY_FINGERPRINT: u64 = {:#x};\n\
             pub(crate) const SHIPPED_CODE_FINGERPRINT: u64 = {:#x};\n",
            fingerprint(&workspace, &library),
            fingerprint(&workspace, &code),
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
    // every body stays interpreted (`jit::available`). The one switch the
    // AArch64 port turns on. It says nothing about which encoder: the x64
    // encoder is named only inside the JIT's own modules (the macro
    // assembler's roles and calling conventions, the stubs), which a port
    // gives its own counterparts, and by the tests that write x64
    // registers by name, which keep a `target_arch = "x86_64"` gate of
    // their own, since they are x64 code on any target the JIT runs on.
    println!("cargo::rustc-check-cfg=cfg(ns_jit)");
    if std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|architecture| architecture == "x86_64") {
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

/// Every file under `directory` with `extension`, sorted, so that the
/// fingerprint does not depend on the file system's order.
fn files_under(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in read_dir(&current).expect("a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|candidate| candidate == extension)
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// FNV-1a over each file's path relative to the workspace, with `/`
/// separators, and its bytes, each preceded by its length.
fn fingerprint(workspace: &Path, files: &[PathBuf]) -> u64 {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    let mut part = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let workspace = workspace.canonicalize().expect("the workspace exists");
    for file in files {
        let file = file.canonicalize().expect("a listed file exists");
        let relative = file.strip_prefix(&workspace).expect("inside the workspace");
        let name = relative.to_string_lossy().replace('\\', "/");
        part(name.as_bytes());
        part(&read(&file).expect("a listed file is readable"));
    }
    state
}
