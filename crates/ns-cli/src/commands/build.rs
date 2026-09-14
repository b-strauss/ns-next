//! `ns-next build`.

use std::path::Path;
use std::process::ExitCode;

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
/// Walking past the root level (`application = a.superclass.clone()`) is safe
/// only because of an invariant this function relies on but does not check:
/// every non-own mixin reachable from a root class's superclass chain today is
/// a builtin's, and a builtin mixin is `Mixin::empty()` -- empty `methods`,
/// empty `nested` -- since native methods dispatch through
/// `builtins::dispatch_native`, never through `Mixin.methods`, and cross-module
/// inheritance does not exist yet. So the ancestor branch can never actually
/// return `true` today; the loop climbs `superclass` only to stay correct if a
/// root class's chain ever grows past one level of `Mixin::empty()`. If ns-next
/// later gains cross-module inheritance, or a core-library superclass written
/// in Newspeak, an ancestor mixin could carry real `methods`/`nested` entries
/// that `mixins::encode` never interns for *this* module's symbol table (it
/// only walks the module's own declaration tree) -- and this check would then
/// accept a selector absent from that table, silently breaking the guarantee
/// above instead of refusing the build. No test catches that today: every
/// fixture here inherits directly from `Object`.
fn root_class_answers<'gc>(
    heap: &ns_runtime::Heap<'gc>,
    class: ns_runtime::ClassReference<'gc>,
    selector: &str,
) -> bool {
    let symbol = ns_runtime::Symbol::intern(selector);
    let mut level = heap.class_application(class);
    while let Some(application) = level {
        if application.mixin.methods.contains_key(&symbol) {
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
        level = application.superclass.clone();
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
