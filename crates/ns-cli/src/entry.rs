//! The CLI's own conventions for starting a program: what argument list to
//! instantiate the root class with, what argument list to size an entry
//! send for, and how to report either one's failure.

use std::process::ExitCode;

/// `--cache-statistics`'s one line, shared by `run` and `bench` -- always to
/// stderr, so it never lands where a fixture or a golden reads stdout.
pub(crate) fn print_cache_statistics(statistics: &ns_runtime::CacheStatistics) {
    eprintln!(
        "cache: {} site entries ({} filled, {} megamorphic), {}/{} polymorphic blocks, {}/{} \
         global entries; entry {} bytes, global entry {} bytes",
        statistics.site_entries,
        statistics.filled_sites,
        statistics.megamorphic_sites,
        statistics.polymorphic_blocks_in_use,
        statistics.polymorphic_blocks,
        statistics.filled_global_entries,
        statistics.global_entries,
        statistics.entry_bytes,
        statistics.global_entry_bytes,
    );
}

/// `--collector-statistics`'s one line, always to stderr so that a golden's
/// recorded stdout is untouched.
pub(crate) fn print_collector_statistics(statistics: &ns_runtime::CollectorStatistics) {
    eprintln!(
        "collector: {} collections, {:.3} ms total pause, {:.3} ms longest; {} live objects, \
         {} live frames",
        statistics.collections,
        statistics.total_pause.as_secs_f64() * 1000.0,
        statistics.longest_pause.as_secs_f64() * 1000.0,
        statistics.live_objects,
        statistics.live_frames,
    );
}

/// Renders the entry send's outcome -- the goldens' recorded `stderr` pins
/// these strings byte for byte, so they live here once and are never
/// duplicated.
pub(crate) fn report_entry_send(result: ns_runtime::HostResult) -> ExitCode {
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(ns_runtime::HostSignal::Error(error)) => {
            eprintln!("Uncaught Newspeak error: {}", error.message);
            ExitCode::FAILURE
        }
        Err(ns_runtime::HostSignal::Return) => {
            eprintln!("internal: non-local return escaped the entry send");
            ExitCode::FAILURE
        }
    }
}

/// Renders a failed instantiation -- the root's or the manifest's -- with
/// the texts the goldens pin.
pub(crate) fn report_instantiation(signal: ns_runtime::HostSignal) -> ExitCode {
    match signal {
        ns_runtime::HostSignal::Error(error) => {
            eprintln!(
                "Uncaught Newspeak error during instantiation: {}",
                error.message
            );
        }
        ns_runtime::HostSignal::Return => {
            eprintln!("internal: non-local return escaped instantiation");
        }
    }
    ExitCode::FAILURE
}

/// `main:args:`'s arguments (§8.4): the platform, and the program's
/// arguments -- the words after `--` -- as an `Array` of `String`s, empty
/// when there are none.
pub(crate) fn main_arguments(
    interpreter: &mut ns_runtime::Interpreter,
    program_arguments: &[String],
) -> Vec<ns_runtime::Root> {
    let platform = interpreter.platform();
    let arguments = interpreter.enter(|mutator| {
        let strings = program_arguments
            .iter()
            .map(|argument| ns_runtime::Value::Object(mutator.new_string(argument.clone())))
            .collect();
        let array = mutator.new_array(strings);
        mutator.stash(ns_runtime::Value::Object(array))
    });
    vec![platform, arguments]
}

/// The one entry send every started program receives: `main: platform
/// args: arguments`, the only way the platform reaches a program.
pub(crate) fn send_main(
    interpreter: &mut ns_runtime::Interpreter,
    application: &ns_runtime::Root,
    program_arguments: &[String],
) -> ns_runtime::HostResult {
    let arguments = main_arguments(interpreter, program_arguments);
    interpreter.send(application, ns_runtime::Symbol::MAIN_ARGUMENTS, &arguments)
}

/// The class object a module's root-class `Root` holds: `load_module` and
/// `image::load` only ever root a class object there.
pub(crate) fn class_reference<'gc>(
    mutator: &ns_runtime::Mutator<'gc>,
    class: &ns_runtime::Root,
) -> ns_runtime::ClassReference<'gc> {
    match mutator.fetch(class) {
        ns_runtime::Value::Object(class) => class,
        _ => unreachable!("internal: a module's root class is a class object"),
    }
}

/// The argument list `instantiate_root` (used by `snapshot` and
/// `load`) and `bench` pass to `interpreter.instantiate`,
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
pub(crate) fn factory_arguments_for<'gc>(
    heap: &ns_runtime::Heap<'gc>,
    class: ns_runtime::ClassReference<'gc>,
    platform: &ns_runtime::Root,
) -> Result<Vec<ns_runtime::Root>, String> {
    let factory = heap
        .class_application(class)
        .and_then(|application| application.mixin.factory);
    match factory {
        // No declared factory (a builtin root, in principle) or a unary
        // one: a unary selector takes no arguments by definition, so there
        // is nothing to guess regardless of what the selector is spelled.
        None => Ok(vec![]),
        Some(factory) if factory.argument_count == 0 => Ok(vec![]),
        // `usingPlatform:`'s one parameter is the only keyword shape the CLI
        // knows what to fill in. The selector is the canonical Newspeak
        // library's convention for a module factory, not a specification
        // rule -- it appears nowhere in `newspeak-spec.pdf`, which settles
        // only that an application's `main:args:` receives a platform
        // object (§8.3, §8.4).
        Some(factory)
            if factory.argument_count == 1
                && factory.selector == ns_runtime::Symbol::USING_PLATFORM =>
        {
            Ok(vec![platform.clone()])
        }
        Some(factory) => Err(format!(
            "cannot instantiate `{}`: its primary factory `{}` takes {} argument(s), and \
             this CLI only knows how to send a zero-argument factory or a one-argument \
             `usingPlatform:`",
            class_name(heap, class),
            factory.selector,
            factory.argument_count
        )),
    }
}

/// The mixin name behind a class object, for `factory_arguments_for`'s error
/// message. `class` is always `ObjectData::Class` at every call site -- it
/// comes either from `bootstrap` or, when `load` starts a
/// stateless binary, from `image::load`'s rebuilt root class -- so the
/// fallback is unreachable in practice and exists only to keep this a total
/// function.
fn class_name<'gc>(heap: &ns_runtime::Heap<'gc>, class: ns_runtime::ClassReference<'gc>) -> String {
    heap.class_application(class)
        .map(|application| application.mixin.name.to_string())
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
        Some(first) if first.is_alphabetic() || first == '_' => 0,
        // A binary selector, e.g. `--send +`. Empty selectors reach the
        // runtime and fail there as an ordinary unknown message.
        Some(_) => 1,
        None => 0,
    }
}

/// The argument list an entry send (`snapshot`/`load`'s `--send`)
/// supplies, sized to what the selector actually takes.
///
/// A method activation addresses its arguments and temporaries as one run
/// of registers, so a surplus argument would silently shift every
/// temporary rather than fail. `Mutator::push_step` refuses the mismatch
/// outright; sizing the list to the selector here is what keeps a
/// legitimate `--send` from tripping that refusal.
///
/// The template is §8.4's `main: platform args: arguments` truncated to the
/// selector's arity: a unary entry point takes nothing, a one-parameter one
/// takes the platform, a two-parameter one takes the platform and "a list of
/// any environment-specific (e.g., command line) arguments" -- the program's
/// arguments, the words after `--`, as an `Array` of `String`s, the list a
/// tuple literal makes, and empty when there are none. Beyond two the CLI has
/// nothing to offer, and says so rather than padding.
pub(crate) fn entry_arguments(
    interpreter: &mut ns_runtime::Interpreter,
    selector: &str,
    platform: &ns_runtime::Root,
    program_arguments: &[String],
) -> Result<Vec<ns_runtime::Root>, String> {
    match selector_arity(selector) {
        0 => Ok(vec![]),
        1 => Ok(vec![platform.clone()]),
        2 => {
            let arguments = interpreter.enter(|mutator| {
                let strings = program_arguments
                    .iter()
                    .map(|argument| ns_runtime::Value::Object(mutator.new_string(argument.clone())))
                    .collect();
                let array = mutator.new_array(strings);
                mutator.stash(ns_runtime::Value::Object(array))
            });
            Ok(vec![platform.clone(), arguments])
        }
        arity => Err(format!(
            "cannot send `{selector}`: it takes {arity} argument(s), and this CLI only knows how \
             to supply the platform and the program's arguments"
        )),
    }
}

/// Instantiates the root `class` with the arguments `factory_arguments_for`
/// builds for it, reporting a failure the way every command that starts a
/// program does. `Err` carries the exit code to return.
pub(crate) fn instantiate_root(
    interpreter: &mut ns_runtime::Interpreter,
    class: &ns_runtime::Root,
) -> Result<ns_runtime::Root, ExitCode> {
    let platform = interpreter.platform();
    let factory_arguments = interpreter
        .enter(|mutator| {
            factory_arguments_for(mutator.heap(), class_reference(mutator, class), &platform)
        })
        .map_err(|message| {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        })?;
    interpreter
        .instantiate(class, &factory_arguments)
        .map_err(report_instantiation)
}
