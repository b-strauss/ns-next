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

/// One line to stderr when the JIT was wanted and is off
/// (`Interpreter::jit_unavailable`): the program runs interpreted, several
/// times slower, and says so rather than unawares. Nothing when the JIT
/// runs, or when nothing asked for it (`NS_NEXT_NO_JIT=1`).
pub(crate) fn report_jit_unavailable(interpreter: &ns_runtime::Interpreter) {
    if let Some(reason) = interpreter.jit_unavailable() {
        eprintln!("{JIT_OFF_WARNING}{reason}");
    }
}

/// The start of `report_jit_unavailable`'s line, the reason after it,
/// which `run-corpus` takes off its children's stderr and prints once
/// itself.
pub(crate) const JIT_OFF_WARNING: &str =
    "warning: the JIT is off, so the program runs interpreted: ";

/// `--collector-statistics`'s one line, always to stderr so that a golden's
/// recorded stdout is untouched.
pub(crate) fn print_collector_statistics(statistics: &ns_runtime::CollectorStatistics) {
    eprintln!(
        "collector: {} scavenges, {:.3} ms total pause, {:.3} ms longest; \
         {} collections, {:.3} ms total pause, {:.3} ms longest; {} live objects; \
         {} table collections",
        statistics.scavenges,
        statistics.scavenge_pause.as_secs_f64() * 1000.0,
        statistics.longest_scavenge_pause.as_secs_f64() * 1000.0,
        statistics.collections,
        statistics.total_pause.as_secs_f64() * 1000.0,
        statistics.longest_pause.as_secs_f64() * 1000.0,
        statistics.live_objects,
        statistics.table_collections,
    );
}

/// `--jit-statistics`'s one line, always to stderr: the baseline's counts,
/// then the optimizing tier's. Both compile times are what a compiler
/// thread of its own would take off the mutator.
pub(crate) fn print_jit_statistics(statistics: &ns_runtime::JitStatistics) {
    eprintln!(
        "jit: {} bodies, {} twins compiled in {:.3} ms; {} back-edge switches, \
         {} resumptions after a safepoint, {} exits; optimizer: {} requests, \
         {} bodies optimized ({} customized copies) in {:.3} ms, \
         {} optimized exits ({} forced, {} traps), {} abandoned, \
         {} made not entrant, {} refused for good, {} invalidated copies, \
         {} dispatched calls",
        statistics.compiled_bodies,
        statistics.compiled_twins,
        statistics.compile_nanoseconds as f64 / 1_000_000.0,
        statistics.back_edge_switches,
        statistics.safepoint_resumptions,
        statistics.exits,
        statistics.optimization_requests,
        statistics.optimized_bodies,
        statistics.customized_copies,
        statistics.optimize_nanoseconds as f64 / 1_000_000.0,
        statistics.optimized_exits,
        statistics.forced_exits,
        statistics.traps,
        statistics.abandoned_optimized_bodies,
        statistics.made_not_entrant,
        statistics.refused_for_good,
        statistics.invalidated_copies,
        statistics.dispatched_calls,
    );
}

/// Makes machine code count what it leaves uncounted unless asked -- the
/// calls through a dispatching entry among it
/// (`JitStatistics::dispatched_calls`) -- when `--jit-statistics` asked
/// for the counts. Counting changes the code compiled from here on, so a
/// run that times passes no `--jit-statistics`.
pub(crate) fn count_jit_if(interpreter: &mut ns_runtime::Interpreter, jit_statistics: bool) {
    if jit_statistics {
        interpreter.set_jit_counting(true);
    }
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
            .map(|argument| ns_runtime::Value::object(mutator.new_string(argument.clone())))
            .collect();
        let array = mutator.new_array(strings);
        mutator.stash(ns_runtime::Value::object(array))
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
    match mutator.fetch(class).as_object() {
        Some(class) => class,
        None => unreachable!("internal: a module's root class is a class object"),
    }
}
