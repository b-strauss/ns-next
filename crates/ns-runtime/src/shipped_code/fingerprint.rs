//! The shipped code's two fingerprints, computed from the files they cover
//! (`docs/design/core-library.md`, "The fingerprint"). `build.rs` compiles
//! them into the binary, which takes the embedded artifact only when its
//! header carries the code's; `cargo xtask gates` computes the same before
//! it builds anything, to leave a current artifact alone without building
//! the generator. One implementation, so that the two can never judge an
//! artifact differently. `build.rs` includes this file by its path, so it
//! uses nothing but `std`.

use std::path::{Path, PathBuf};

/// The two fingerprints of a workspace's sources.
pub struct Fingerprints {
    /// Over the shipped modules' sources alone -- `Kernel.ns`,
    /// `PlatformFactory.ns` and every `.ns` under `platform/` -- which goes
    /// into an image's fingerprint and so must not change with every Rust
    /// edit.
    pub library: u64,
    /// Over everything that can change what the shipped modules compile
    /// to: their sources, the Rust sources of `ns-syntax` and `ns-runtime`,
    /// `ns-runtime`'s `build.rs` and both manifests. It invalidates more
    /// often than needed, and a stale artifact is never decoded.
    pub code: u64,
}

/// The fingerprints of the sources under `workspace`, the repository's
/// root. Neither covers `Manifest.ns`, which a program compiles as a module
/// of its own, so an edit to it changes neither the shipped code nor the
/// shipped world.
pub fn compute(workspace: &Path) -> std::io::Result<Fingerprints> {
    let runtime = workspace.join("crates/ns-runtime");
    let mut library = vec![
        runtime.join("core-library/Kernel.ns"),
        runtime.join("core-library/PlatformFactory.ns"),
    ];
    library.extend(files_under(&runtime.join("core-library/platform"), "ns")?);
    let mut code = library.clone();
    code.extend(files_under(&runtime.join("src"), "rs")?);
    code.extend(files_under(&workspace.join("crates/ns-syntax/src"), "rs")?);
    code.push(runtime.join("build.rs"));
    code.push(runtime.join("Cargo.toml"));
    code.push(workspace.join("crates/ns-syntax/Cargo.toml"));
    Ok(Fingerprints {
        library: fingerprint(workspace, &library)?,
        code: fingerprint(workspace, &code)?,
    })
}

/// Every file under `directory` with `extension`, sorted, so that the
/// fingerprint does not depend on the file system's order.
fn files_under(directory: &Path, extension: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current)? {
            let path = entry?.path();
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
    Ok(found)
}

/// FNV-1a over each file's path relative to the workspace, with `/`
/// separators, and its bytes, each preceded by its length.
fn fingerprint(workspace: &Path, files: &[PathBuf]) -> std::io::Result<u64> {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    let mut part = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let workspace = workspace.canonicalize()?;
    for file in files {
        let file = file.canonicalize()?;
        let relative = file.strip_prefix(&workspace).map_err(|_| {
            std::io::Error::other(format!("{} lies outside the workspace", file.display()))
        })?;
        let name = relative.to_string_lossy().replace('\\', "/");
        part(name.as_bytes());
        part(&std::fs::read(&file)?);
    }
    Ok(state)
}
