//! `ns-next build`.

use std::path::Path;
use std::process::ExitCode;

use ns_runtime::mixin::MethodEntry;

use crate::source;

/// Whether the root class can answer `selector`: a method anywhere up its
/// application chain, or the accessor of one of its nested declarations.
///
/// Checked at build time so a binary that could never start is refused where
/// the mistake was made. It is a convenience, not a guarantee -- a selector
/// that slips through still fails at start-up as an ordinary
/// `doesNotUnderstand:`, through the same path `run` uses.
///
/// Deliberately narrow: only method selectors and nested-class accessor
/// names, because those are exactly what `mixins::encode` interns into the
/// symbol table while walking this same superclass chain over `mixin.methods`
/// and `mixin.nested`. Accepting anything wider -- a slot's implicit *setter*,
/// a builtin's native method -- would accept a selector the code and mixin
/// tables never interned: a slot's *getter* is interned (`mixins::encode`
/// writes every slot name), but the setter selector it is derived from at
/// dispatch time is not a name `mixins::encode` ever writes. Accepting it
/// would make `build` have to grow the symbol table to record it, breaking
/// the byte-identity guarantee with `snapshot` that this check exists to
/// preserve (`docs/design/image-format.md`, "The consistency guarantee,
/// stated precisely").
///
/// The walk stops at the first level whose `class_builtin` is `Some`: since
/// `Object`'s methods moved into `Kernel.ns`
/// (`docs/design/core-library.md`, "The VM mirror is the whole Rust
/// boundary"), a builtin's own level can hold a real `MethodEntry::Compiled`
/// entry, not only `MethodEntry::Primitive` -- `Object`'s `printString`, for
/// one. Every builtin level is the kernel's, which `mixins::encode` never
/// walks (it interns only the module's own declaration tree), so accepting a
/// selector found there would accept one absent from the image's symbol
/// table, breaking the byte-identity guarantee above instead of refusing the
/// build -- `build_refuses_an_entry_only_a_builtin_answers` pins this.
fn root_class_answers<'gc>(
    heap: &ns_runtime::Heap<'gc>,
    class: ns_runtime::ClassReference<'gc>,
    selector: &str,
) -> bool {
    let symbol = ns_runtime::Symbol::intern(selector);
    let mut level = Some(class);
    while let Some(current) = level {
        if heap.class_builtin(current).is_some() {
            return false;
        }
        let Some(application) = heap.class_application(current) else {
            return false;
        };
        let declared = application
            .mixin
            .methods
            .get(&symbol)
            .is_some_and(|entry| !matches!(entry, MethodEntry::Primitive(_)));
        if declared {
            return true;
        }
        if application
            .mixin
            .nested
            .iter()
            .any(|nested| nested.name == symbol)
        {
            return true;
        }
        level = heap.class_superclass(current);
    }
    false
}

/// `ns-next build`: `run`'s pipeline up to loading the module, and then
/// deliberately no instantiation.
pub(crate) fn execute(
    file: &Path,
    out: &Path,
    entry: Option<&str>,
    debug_source: bool,
    lowering: ns_runtime::Lowering,
) -> ExitCode {
    let loaded = match source::load_source(file) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let (mut interpreter, class) = source::bootstrap(&loaded, lowering);

    let entry = match entry {
        Some(selector) => selector.to_string(),
        None if interpreter.enter(|mutator| {
            root_class_answers(
                mutator.heap(),
                crate::entry::class_reference(mutator, &class),
                "main:args:",
            )
        }) =>
        {
            "main:args:".to_string()
        }
        None => {
            eprintln!(
                "error: {} declares no `main:args:`; pass --entry with the selector to send",
                file.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let platform = interpreter.platform();
    if let Err(message) = crate::entry::entry_arguments(&interpreter, &entry, &platform) {
        eprintln!("error: cannot use `{entry}` as an entry point: {message}");
        return ExitCode::FAILURE;
    }
    let answers_entry = interpreter.enter(|mutator| {
        root_class_answers(
            mutator.heap(),
            crate::entry::class_reference(mutator, &class),
            &entry,
        )
    });
    if !answers_entry {
        eprintln!(
            "error: the root class of {} does not answer `{entry}`",
            file.display()
        );
        return ExitCode::FAILURE;
    }

    let bytes = match interpreter.enter(|mutator| {
        ns_runtime::image::build(
            mutator.heap(),
            debug_source.then_some(loaded.source.as_str()),
            &loaded.origin,
            crate::entry::class_reference(mutator, &class),
            ns_runtime::Symbol::intern(&entry),
        )
    }) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: build failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: writing {}: {error}", out.display());
            ExitCode::FAILURE
        }
    }
}
