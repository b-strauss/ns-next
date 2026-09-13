//! `ns-next disasm`.

use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;

use crate::source;

/// Resolves a dot-separated mixin path like `Outer.Inner` or `Outer.class`
/// against `root`'s own mixin tree. `None` (no `--class` given) means the
/// root itself, preserving `disasm`'s pre-nesting behaviour exactly. When a
/// path is given, its first segment must name the root -- every path
/// `disasm` accepts or reports is then rooted at the same place the
/// source's own class declaration is -- and each further segment either
/// names a nested class declaration in the previous level's `nested` list
/// or is the literal `class`, meaning that level's own class side.
fn resolve_mixin_path(
    root: &Rc<ns_runtime::Mixin>,
    path: Option<&str>,
) -> Result<Rc<ns_runtime::Mixin>, String> {
    let Some(path) = path else {
        return Ok(root.clone());
    };
    let mut segments = path.split('.');
    let first = segments.next().unwrap_or_default();
    if first != root.name.as_str() {
        return Err(format!(
            "unknown class path `{path}` (expected it to start with the root class `{}`)",
            root.name
        ));
    }
    let mut current = root.clone();
    for segment in segments {
        current = if segment == "class" {
            current
                .class_side
                .clone()
                .ok_or_else(|| format!("`{}` has no class side", current.name))?
        } else {
            current
                .nested
                .iter()
                .find(|m| m.name.as_str() == segment)
                .cloned()
                .ok_or_else(|| format!("`{}` has no nested class `{segment}`", current.name))?
        };
    }
    Ok(current)
}

/// Whether `bytes` begins with the image format's magic.
///
/// Sniffing the content rather than the file extension is deliberate: an
/// artifact has no required extension, `snapshot`/`build` write whatever path
/// they are given, and a `.ns` file cannot begin with these four bytes because
/// every module starts with the `Newspeak3` language identifier.
fn looks_like_an_artifact(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == ns_runtime::image::encode::MAGIC
}

/// The mixin behind a class object, or the message for the one case where
/// there is none. Shared by both of `execute`'s branches so each can
/// read it from its own `class` while that class's interpreter is still
/// alive, rather than through a `Heap` built after the fact.
fn root_mixin_of(
    heap: &ns_runtime::Heap,
    class: ns_runtime::ClassRef,
) -> Result<Rc<ns_runtime::Mixin>, String> {
    heap.class_application(class)
        .map(|app| app.mixin.clone())
        .ok_or_else(|| "internal: loaded module did not produce a class object".to_string())
}

/// `ns-next disasm`: for a `.ns` file, bootstrap and load it, which compiles
/// the *whole* mixin tree onto its declarations; for a serialized artifact,
/// decode it directly, which attaches its own code table's bodies instead of
/// recompiling anything. Either way, the result is the root mixin, whose tree
/// carries the code, from which a `--class` path can address any mixin, then
/// take exactly one method's body or the target mixin's initializer and print
/// its bytecode listing. The source path never instantiates; the artifact
/// path never instantiates either, since `image::load` only decodes the
/// world. `lowering` chooses how a source is compiled; it is refused for an
/// artifact.
pub(crate) fn execute(
    file: &Path,
    selector: Option<&str>,
    initializer: bool,
    class_path: Option<&str>,
    lowering: Option<ns_runtime::Lowering>,
) -> ExitCode {
    let raw = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };

    // Two ways to reach the root mixin, whose tree carries the code.
    // Compiling from source needs the resolver's bindings; an artifact
    // already carries compiled bodies, so loading it is a decode and nothing
    // is recompiled. Each arm reads
    // `root_mixin` out of its own `class` before that arm's interpreter (or
    // `loaded.interp`) goes out of scope, rather than through a fresh
    // `Heap` afterwards. A `ClassRef` is an index into the arena of the
    // `Heap` that allocated it and means nothing against any other one, so
    // a `Heap` built after that interpreter is gone does not answer
    // `class`'s data: in a debug build the origin guard panics at the read,
    // and in a release build the index is in range and names whatever
    // object that heap put there -- a different, perfectly valid object,
    // disassembled without a word of complaint.
    let root_mixin = if looks_like_an_artifact(&raw) {
        if lowering.is_some() {
            eprintln!(
                "error: --lowering applies to a .ns source; {} is an artifact, which carries the code it was built with",
                file.display()
            );
            return ExitCode::FAILURE;
        }
        match ns_runtime::image::load(&raw) {
            Ok(loaded) => match root_mixin_of(loaded.interp.heap_ref(), loaded.root_class) {
                Ok(m) => m,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    return ExitCode::FAILURE;
                }
            },
            Err(e) => {
                eprintln!("error: {}: {e}", file.display());
                return ExitCode::FAILURE;
            }
        }
    } else {
        let loaded = match source::load_source(file) {
            Ok(l) => l,
            Err(code) => return code,
        };
        let (interp, class) =
            source::bootstrap(&loaded, lowering.unwrap_or(ns_runtime::Lowering::Optimized));
        match root_mixin_of(interp.heap_ref(), crate::entry::class_ref(&interp, &class)) {
            Ok(m) => m,
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::FAILURE;
            }
        }
    };
    let mixin = match resolve_mixin_path(&root_mixin, class_path) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let (code, name) = match (selector, initializer) {
        (None, true) => {
            // Not an invariant: a decoded mixin with no slots and no
            // `initExpr`s legitimately carries no initializer (`verify_mixins`
            // accepts that row), and an artifact is untrusted input, so this
            // is a refusal rather than a panic.
            let Some(code) = mixin.initializer_code.clone() else {
                eprintln!("error: `{}` has no instance initializer", mixin.name);
                return ExitCode::FAILURE;
            };
            (code, mixin.name.to_string())
        }
        (Some(selector), false) => {
            let method = mixin
                .methods
                .get(&ns_runtime::Symbol::intern(selector))
                .cloned();
            let Some(method) = method else {
                eprintln!("error: no method `{selector}` found on `{}`", mixin.name);
                return ExitCode::FAILURE;
            };
            (method.code.clone(), selector.to_string())
        }
        _ => {
            eprintln!("error: disasm takes exactly one of <selector> or --initializer");
            return ExitCode::FAILURE;
        }
    };
    print!(
        "{}",
        ns_runtime::bytecode::disassemble(code.as_ref(), &name)
    );
    ExitCode::SUCCESS
}
