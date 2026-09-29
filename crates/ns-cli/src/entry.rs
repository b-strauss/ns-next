//! The CLI's own conventions for starting a program: the one entry send and
//! its arguments, and how to report a failed instantiation or send.

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
