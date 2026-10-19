//! Shared helpers for the analyzer's command-line tests.
//!
//! Each test binary that needs these declares `mod common;`, so this file is
//! compiled once per binary rather than linked once -- which is why the
//! allow below is here: a binary that uses only some of the helpers would
//! otherwise warn about the rest. `ns-cli`'s tests keep their own
//! `common/mod.rs`, whose runner starts `ns-next`: a test can start only its
//! own crate's binaries.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

/// The `goldens/` directory: every recorded program outcome lives in a
/// category directory below it.
pub fn goldens_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../goldens")
}

/// One golden's source by its path below `goldens/`, without the extension --
/// `golden("library/number/integer/arithmetic")`. Panics on a miss, so a
/// renamed or moved golden fails loudly here rather than silently skipping
/// the test that wanted it.
pub fn golden(path: &str) -> PathBuf {
    let parts: Vec<&str> = path.split('/').collect();
    assert!(
        parts.len() >= 2 && parts.iter().all(|part| !part.is_empty()),
        "{path} is not `<category>/<name>` -- a golden lives in a category \
         directory below goldens/, so its path has at least one directory component"
    );
    let (name, directories) = parts.split_last().expect("at least two parts");
    let mut source = goldens_root();
    source.extend(directories);
    source.push(format!("{name}.ns"));
    assert!(
        source.is_file(),
        "no golden at {} -- goldens/{path}.ns does not exist",
        source.display()
    );
    source
}

/// A temporary directory that removes itself, under the repository's own
/// `.temp/` (git-ignored) rather than the system temporary folder, so that
/// whatever a killed run leaves behind stays visible and belongs to the
/// project. The name carries the process id and a counter as well as the
/// test's own name because cargo runs a binary's tests as threads of one
/// process.
pub struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    pub fn new(test: &str) -> ScratchDirectory {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            scratch_root().join(format!("{}-{sequence}-analyzer-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        ScratchDirectory { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the directory. The file need not exist yet.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

/// `.temp/` at the repository root, two levels above this crate.
fn scratch_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/ns-analyzer is two levels below the repository root")
        .join(".temp")
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        // Best effort on purpose: a failing test unwinds through here, and a
        // panic during a panic would replace the assertion message.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Runs the `ns-next-analyzer` binary and returns `(success, stdout,
/// stderr)`, with line endings normalized so an assertion reads the same on
/// every platform.
pub fn ns_next_analyzer(arguments: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .args(arguments)
        .output()
        .expect("run ns-next-analyzer");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
    )
}

/// The image `ns-next build <source> <image>` writes for a single `.ns`
/// source whose root is instantiated with `new`: the module loaded, its root
/// instantiated, and the instance packaged as the application, with the
/// source text stored when `with_source` (as `build` stores it unless
/// `--strip-source`). Built through
/// `ns-runtime` because a test cannot start another crate's binary, and the
/// image is all an `inspect` or `disasm` test reads.
pub fn build_image(source: &Path, lowering: ns_runtime::Lowering, with_source: bool) -> Vec<u8> {
    let text = std::fs::read_to_string(source)
        .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
    let Ok(module) = ns_syntax::parse(&text) else {
        panic!("{} does not parse", source.display())
    };
    let origin = ns_runtime::SourceOrigin::File(source.to_path_buf());
    let Ok(resolved) = ns_runtime::resolve(module, Rc::from(text.as_str()), origin.clone()) else {
        panic!("{} does not resolve", source.display())
    };
    let mut interpreter = ns_runtime::Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    let Ok(application) = interpreter.instantiate(&class, &[]) else {
        panic!("`new` of {}'s root raises", source.display())
    };
    let packaged = interpreter.enter(|mutator| {
        let Some(class) = mutator.fetch(&class).as_object() else {
            panic!("load_module answers a class object")
        };
        let modules = [ns_runtime::image::ModuleSource {
            class,
            origin: &origin,
            with_source,
        }];
        ns_runtime::image::package(mutator, &modules, mutator.fetch(&application))
    });
    match packaged {
        Ok(bytes) => bytes,
        Err(error) => panic!("package {}: {error}", source.display()),
    }
}
