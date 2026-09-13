//! The CLI's own conventions for starting a program: what argument list to
//! instantiate the root class with, what argument list to size an entry
//! send for, and how to report either one's failure.

use std::process::ExitCode;

/// Renders the entry send's outcome -- `program_outcomes.rs` pins these
/// strings byte for byte, so they live here once and are never duplicated.
pub(crate) fn report_entry_send(result: Result<ns_runtime::Value, ns_runtime::Signal>) -> ExitCode {
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(ns_runtime::Signal::Error(e)) => {
            eprintln!("Uncaught Newspeak error: {}", e.message);
            ExitCode::FAILURE
        }
        Err(ns_runtime::Signal::Return(..)) => {
            eprintln!("internal: non-local return escaped the entry send");
            ExitCode::FAILURE
        }
    }
}

fn report_signal(signal: ns_runtime::Signal) -> ExitCode {
    match signal {
        ns_runtime::Signal::Error(e) => {
            eprintln!(
                "Uncaught Newspeak error during instantiation: {}",
                e.message
            );
        }
        ns_runtime::Signal::Return(..) => {
            eprintln!("internal: non-local return escaped instantiation");
        }
    }
    ExitCode::FAILURE
}

/// The argument list `instantiate_root` (used by `run`, `snapshot` and
/// `load`) and `bench` pass to `interp.instantiate`,
/// built from the root class's own declared primary factory pattern
/// (`Mixin::factory`) rather than assumed.
///
/// Deciding *that* a command-line entry point sends the platform, rather
/// than some other object, is a CLI concern -- nothing in the runtime
/// needs to know it was invoked from a terminal. Deciding whether a given
/// factory arity is even *satisfiable* is not a CLI opinion, though: that
/// check already lives in `instantiate.rs` (the "wrong number of factory
/// arguments" error), and stays there. This function draws the line at
/// producing the one argument list the CLI is able to construct at all --
/// it has no manifest, no IDE, no test-framework object to offer a
/// multi-parameter factory, so anything that isn't the two shapes below is
/// refused here, before a guessed argument could reach `instantiate` and
/// fail for a reason this message never mentions.
pub(crate) fn factory_args_for(
    heap: &ns_runtime::Heap,
    class: ns_runtime::ClassRef,
    platform: ns_runtime::Value,
) -> Result<Vec<ns_runtime::Value>, String> {
    let factory = heap
        .class_application(class)
        .and_then(|app| app.mixin.factory);
    match factory {
        // No declared factory (a builtin root, in principle) or a unary
        // one: a unary selector takes no arguments by definition, so there
        // is nothing to guess regardless of what the selector is spelled.
        None => Ok(vec![]),
        Some(factory) if factory.num_args == 0 => Ok(vec![]),
        // `usingPlatform:`'s one parameter (`newspeak-spec.pdf` §8.4's Core
        // Library factory convention) is the only keyword shape the CLI
        // knows what to fill in.
        Some(factory)
            if factory.num_args == 1 && factory.selector == ns_runtime::Symbol::USING_PLATFORM =>
        {
            Ok(vec![platform])
        }
        Some(factory) => Err(format!(
            "cannot instantiate `{}`: its primary factory `{}` takes {} argument(s), and \
             this CLI only knows how to send a zero-argument factory or a one-argument \
             `usingPlatform:`",
            class_name(heap, class),
            factory.selector,
            factory.num_args
        )),
    }
}

/// The mixin name behind a class object, for `factory_args_for`'s error
/// message. `class` is always `ObjectData::Class` at every call site -- it
/// comes either from `bootstrap` or, when `load` starts a
/// stateless binary, from `image::load`'s rebuilt root class -- so the
/// fallback is unreachable in practice and exists only to keep this a total
/// function.
fn class_name(heap: &ns_runtime::Heap, class: ns_runtime::ClassRef) -> String {
    heap.class_application(class)
        .map(|app| app.mixin.name.to_string())
        .unwrap_or_else(|| "<non-class>".to_string())
}

/// How many arguments `selector` takes, read off its spelling.
///
/// The one place the CLI decides this. A keyword selector takes one
/// argument per colon, a binary selector exactly one, a unary selector
/// none -- Newspeak identifiers start with a letter or `_`, so anything
/// else beginning a colon-free selector is an operator.
fn selector_arity(selector: &str) -> usize {
    if selector.contains(':') {
        return selector.matches(':').count();
    }
    match selector.chars().next() {
        Some(c) if c.is_alphabetic() || c == '_' => 0,
        // A binary selector, e.g. `--send +`. Empty selectors reach the
        // runtime and fail there as an ordinary unknown message.
        Some(_) => 1,
        None => 0,
    }
}

/// The argument list an entry send (`run`/`snapshot`/`load`'s `--send`)
/// supplies, sized to what the selector actually takes.
///
/// A method activation addresses its arguments and temporaries as one run
/// of registers, so a surplus argument would silently shift every
/// temporary rather than fail. `Interpreter::push_step` refuses the mismatch
/// outright; sizing the list to the selector here is what keeps a
/// legitimate `--send` from tripping that refusal.
///
/// The template is the Core Library's `main: platform args: arguments`
/// convention truncated to the selector's arity: a unary entry point takes
/// nothing, a one-parameter one takes the platform, a two-parameter one
/// takes the platform and `Nil` for the environment-args list -- there is
/// still no command-line-argument passthrough into Newspeak. Beyond two
/// the CLI has nothing to offer, and says so rather than padding.
pub(crate) fn entry_args(
    selector: &str,
    platform: ns_runtime::Value,
) -> Result<Vec<ns_runtime::Value>, String> {
    match selector_arity(selector) {
        0 => Ok(vec![]),
        1 => Ok(vec![platform]),
        2 => Ok(vec![platform, ns_runtime::Value::Nil]),
        n => Err(format!(
            "cannot send `{selector}`: it takes {n} argument(s), and this CLI only knows how \
             to supply the platform and a `nil` argument list"
        )),
    }
}

/// Instantiates the root `class` with the arguments `factory_args_for` builds
/// for it, reporting a failure the way every command that starts a program
/// does. `Err` carries the exit code to return.
pub(crate) fn instantiate_root(
    interp: &mut ns_runtime::Interpreter,
    class: ns_runtime::ClassRef,
) -> Result<ns_runtime::Value, ExitCode> {
    let platform = interp.platform_value();
    let factory_args = factory_args_for(interp.heap_ref(), class, platform).map_err(|msg| {
        eprintln!("error: {msg}");
        ExitCode::FAILURE
    })?;
    interp
        .instantiate_unrooted(class, factory_args)
        .map_err(report_signal)
}
