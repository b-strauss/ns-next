//! `ns-next-analyzer disasm`.

use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;

use ns_tooling::source;

/// Resolves a dot-separated mixin path like `Outer.Inner` or `Outer.class`
/// against `root`'s own mixin tree. `None` (no `--class` given) means the
/// root itself, preserving `disasm`'s pre-nesting behaviour exactly. When a
/// path is given, its first segment must name the root -- every path
/// `disasm` accepts or reports is then rooted at the same place the
/// source's own class declaration is -- and each further segment either
/// names a nested class declaration of the previous level, as `nested`
/// answers them, or is the literal `class`, meaning that level's own class
/// side.
fn resolve_mixin_path(
    root: &Rc<ns_runtime::Mixin>,
    path: Option<&str>,
    mut nested: impl FnMut(&Rc<ns_runtime::Mixin>) -> Vec<Rc<ns_runtime::Mixin>>,
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
            nested(&current)
                .into_iter()
                .find(|nested| nested.name.as_str() == segment)
                .ok_or_else(|| format!("`{}` has no nested class `{segment}`", current.name))?
        };
    }
    Ok(current)
}

/// Whether `bytes` begins with the image format's magic and the version
/// after it.
///
/// Sniffing the content rather than the file extension is deliberate: an
/// artifact has no required extension, and `build` writes whatever path it
/// is given. The magic alone does not decide, because a module may open with
/// the language id `NSNX`; the version's first byte, a control character no
/// source text has there, does.
fn looks_like_an_artifact(bytes: &[u8]) -> bool {
    // The version is written little-endian; a version whose low byte were
    // printable or whitespace would make the test below miss every artifact.
    const VERSION_LOW_BYTE: u8 = ns_runtime::image::encode::FORMAT_VERSION.to_le_bytes()[0];
    const _: () =
        assert!(VERSION_LOW_BYTE.is_ascii_control() && !VERSION_LOW_BYTE.is_ascii_whitespace());
    bytes.len() >= 5
        && &bytes[0..4] == ns_runtime::image::encode::MAGIC
        && bytes[4].is_ascii_control()
        && !bytes[4].is_ascii_whitespace()
}

/// `ns-next-analyzer disasm`: for a `.ns` file, compile it, which compiles the
/// *whole* mixin tree onto its declarations; for a serialized artifact,
/// decode it directly, which attaches its own code table's bodies instead of
/// recompiling anything. Either way, the result is the root mixin, whose tree
/// carries the code, from which a `--class` path can address any mixin, then
/// take exactly one method's body or the target mixin's initializer and print
/// its bytecode listing. Neither path instantiates or runs anything of the
/// program: the source path compiles on a bootstrapped interpreter, and the
/// artifact path's `image::load` only decodes the world into one; the only
/// Newspeak either runs is the shipped core library's instantiation in that
/// bootstrap. `lowering` chooses how a source is compiled; it is refused for an
/// artifact.
pub(crate) fn execute(
    file: &Path,
    selector: Option<&str>,
    initializer: bool,
    class_path: Option<&str>,
    lowering: Option<ns_runtime::Lowering>,
) -> ExitCode {
    let raw = match std::fs::read(file) {
        Ok(raw) => raw,
        Err(error) => {
            eprintln!("error: reading {}: {error}", file.display());
            return ExitCode::FAILURE;
        }
    };

    // Two ways to reach the root mixin, whose tree carries the code.
    // Compiling from source needs the resolver's bindings; an artifact
    // already carries compiled bodies, so loading it is a decode and nothing
    // is recompiled. Neither reads the mixin off a class object: the code is
    // all `disasm` prints, so it holds no handle that would tie it to the
    // heap that minted it. A loaded image's heap registered its mixins,
    // and names their nested declarations by id from then on, so the
    // artifact path keeps the interpreter for the `--class` path to read
    // them through.
    let mut loaded_interpreter: Option<ns_runtime::Interpreter> = None;
    let root_mixin = if looks_like_an_artifact(&raw) {
        if lowering.is_some() {
            eprintln!(
                "error: --lowering applies to a .ns source; {} is an artifact, which carries the code it was built with",
                file.display()
            );
            return ExitCode::FAILURE;
        }
        match ns_runtime::image::load(&raw) {
            Ok(loaded) => {
                let Some(application) = loaded.modules.into_iter().next() else {
                    eprintln!("error: {} carries no module", file.display());
                    return ExitCode::FAILURE;
                };
                loaded_interpreter = Some(loaded.interpreter);
                application.declaration
            }
            Err(error) => {
                eprintln!("error: {}: {error}", file.display());
                return ExitCode::FAILURE;
            }
        }
    } else {
        let loaded = match source::load_source(file) {
            Ok(loaded) => loaded,
            Err(code) => return code,
        };
        ns_runtime::Interpreter::bootstrap_with(lowering.unwrap_or(ns_runtime::Lowering::Optimized))
            .compile_module(&loaded.resolved)
    };
    let nested = |mixin: &Rc<ns_runtime::Mixin>| match &mut loaded_interpreter {
        Some(interpreter) => interpreter.enter(|mutator| mutator.heap().nested_mixins(mixin)),
        None => mixin.declared_nested().to_vec(),
    };
    let mixin = match resolve_mixin_path(&root_mixin, class_path, nested) {
        Ok(mixin) => mixin,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let (code, name) = match (selector, initializer) {
        (None, true) => {
            // A loaded artifact's mixin always carries its full body
            // (`verify_mixins`), but `disasm` holds nothing to that: an
            // artifact is untrusted input, so this is a refusal rather than
            // a panic.
            let Some(code) = mixin.initializer_code.clone() else {
                eprintln!("error: `{}` has no instance initializer", mixin.name);
                return ExitCode::FAILURE;
            };
            print!("{}", initializer_listing(&mixin, &code));
            return ExitCode::SUCCESS;
        }
        (Some(selector), false) => {
            // A slot accessor runs no code, so there is none to disassemble.
            // A text nothing interned names no method.
            let method = ns_runtime::Symbol::lookup(selector)
                .and_then(|selector| mixin.compiled_method(selector))
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

/// `--initializer`'s listing: `mixin`'s full instance initializer `full`,
/// its own part, its standalone initializer when it has one, and the body
/// of its factory, one after another, a blank line between two. The own
/// part is left out when a decoded mixin carries none, which
/// `verify_mixins` accepts of a class without slots and `initExpr`s; the
/// factory when the mixin has none, as a class side has not.
fn initializer_listing(
    mixin: &ns_runtime::Mixin,
    full: &ns_runtime::bytecode::CompiledCode,
) -> String {
    let name = mixin.name.to_string();
    let mut listings = vec![ns_runtime::bytecode::disassemble(full, &name)];
    if let Some(own) = &mixin.own_initializer_code {
        listings.push(ns_runtime::bytecode::disassemble(
            own,
            &format!("{name} own part"),
        ));
    }
    if let Some(standalone) = &mixin.standalone_initializer_code {
        listings.push(ns_runtime::bytecode::disassemble(
            standalone,
            &format!("{name} standalone"),
        ));
    }
    if let Some(factory) = mixin.factory {
        // The entry the class side carries, which is the one a send runs;
        // a class side a declaration does not write is synthesized when its
        // metaclass is, from the same function.
        let entry = mixin
            .class_side
            .as_ref()
            .and_then(|class_side| class_side.methods.get(&factory.selector).cloned())
            .unwrap_or_else(|| ns_runtime::mixin::factory_entry(Some(factory)).1);
        if let ns_runtime::mixin::MethodEntry::Factory(entry) = entry {
            listings.push(ns_runtime::bytecode::disassemble(
                &entry.code,
                &format!("{name} factory {}", entry.selector),
            ));
        }
    }
    listings.join("\n")
}
