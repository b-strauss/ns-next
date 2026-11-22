//! `ns-next bench`.

use std::path::Path;
use std::process::ExitCode;

use crate::entry;
use crate::program::{self, Loaded, Program};

/// The timing summary.
struct BenchmarkStatistics {
    iterations: u32,
    median: std::time::Duration,
    min: std::time::Duration,
    max: std::time::Duration,
    /// The inline caches' sizes after the timed runs, when `--cache-statistics`
    /// asked for them.
    cache_statistics: Option<ns_runtime::CacheStatistics>,
    /// The collector's counts and pauses after the timed runs, when
    /// `--collector-statistics` asked for them.
    collector_statistics: Option<ns_runtime::CollectorStatistics>,
    /// The JIT's counts after the timed runs, when `--jit-statistics` asked
    /// for them.
    jit_statistics: Option<ns_runtime::JitStatistics>,
}

/// Reports a failed entry send on stderr, once, and answers the exit code
/// `bench` ends with.
fn report_signal(signal: ns_runtime::HostSignal) -> ExitCode {
    match signal {
        ns_runtime::HostSignal::Error(error) => {
            report_line!("error: Uncaught Newspeak error: {}", error.message);
        }
        ns_runtime::HostSignal::Return => {
            report_line!("error: internal: non-local return escaped the entry send");
        }
    }
    ExitCode::FAILURE
}

/// One assemble-and-send sample. Assembling the application happens but is
/// deliberately *not* part of the timed span: it is fixed, once-per-run
/// setup, not the repeated dispatch this benchmark measures. It cannot be
/// hoisted outside the loop either, though -- a program mutates its own
/// instance variables (loop counters, accumulators), so sending the same
/// application a second time would time a cheaper run against
/// already-mutated state instead of the same workload every iteration.
///
/// A failure is reported on stderr where it happens --
/// `instantiate_application` reports its own -- so the `Err` carries only
/// the exit code.
fn timed_send(
    program: &Program,
    loaded: &mut Loaded,
    arguments: &[ns_runtime::Root],
) -> Result<std::time::Duration, ExitCode> {
    let application = program::instantiate_application(program, loaded, None)?;
    let start = std::time::Instant::now();
    let outcome =
        loaded
            .interpreter
            .send(&application, ns_runtime::Symbol::MAIN_ARGUMENTS, arguments);
    let elapsed = start.elapsed();
    outcome.map(|_| elapsed).map_err(report_signal)
}

/// Loads the program (which compiles every module), discards one warm-up
/// `timed_send`, times `iterations` further ones, and reduces the samples to
/// `BenchmarkStatistics`. Loading and compiling happen before the loop --
/// once-per-process costs, excluded from the per-send timing for the same
/// reason assembling is.
fn benchmark_entry(
    program: &Program,
    iterations: u32,
    cache_statistics: bool,
    collector_statistics: bool,
    jit_statistics: bool,
    program_arguments: &[String],
) -> Result<BenchmarkStatistics, ExitCode> {
    let mut loaded = program::load_program(program, ns_runtime::Lowering::Optimized);
    // A measurement never times the interpreter in the JIT's place: a JIT
    // that was wanted and is off refuses the benchmark, naming why.
    if let Some(reason) = loaded.interpreter.jit_unavailable() {
        report_line!("error: the JIT is off, and a benchmark would time the interpreter: {reason}");
        return Err(ExitCode::FAILURE);
    }
    entry::count_jit_if(&mut loaded.interpreter, jit_statistics);
    let arguments = entry::main_arguments(&mut loaded.interpreter, program_arguments);

    // Warm-up: discarded. It exercises the same allocation and lookup
    // paths the timed runs will, so the first timed sample does not carry
    // a one-time cost (e.g. a lazily-populated cache) the rest do not.
    timed_send(program, &mut loaded, &arguments)?;
    // Every sample assembles the application afresh and asks the manifest
    // what the warm-up asked: its warnings are printed once per process,
    // by the warm-up, not once per sample.
    loaded.warns = false;

    let mut timings = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        timings.push(timed_send(program, &mut loaded, &arguments)?);
    }
    timings.sort();
    let min = timings[0];
    let max = timings[timings.len() - 1];
    // Median of N, not mean: a mean lets one
    // GC-pause-sized outlier (there is no GC yet, but a scheduler
    // preemption is the same shape of problem) drag the whole number with
    // it, while the median shrugs off any one such sample.
    let middle = timings.len() / 2;
    let median = if timings.len() % 2 == 1 {
        timings[middle]
    } else {
        (timings[middle - 1] + timings[middle]) / 2
    };
    // After every timed run, so it reports what the timed sends actually
    // filled, not only the discarded warm-up.
    let cache_statistics = cache_statistics.then(|| loaded.interpreter.cache_statistics());
    let collector_statistics =
        collector_statistics.then(|| loaded.interpreter.collector_statistics());
    let jit_statistics = jit_statistics.then(|| loaded.interpreter.jit_statistics());
    Ok(BenchmarkStatistics {
        iterations,
        median,
        min,
        max,
        cache_statistics,
        collector_statistics,
        jit_statistics,
    })
}

/// `ns-next bench`: see the `Bench` variant's doc comment for what is
/// timed and why, and `benchmark_entry`/`timed_send` for how.
pub(crate) fn execute(
    file: &Path,
    iterations: u32,
    cache_statistics: bool,
    collector_statistics: bool,
    jit_statistics: bool,
    script: Option<&str>,
    program_arguments: &[String],
) -> ExitCode {
    if cfg!(debug_assertions) {
        report_line!(
            "warning: this is a debug build; the timings below measure debug-build overhead, \
             not the runtime. Rebuild and re-run with `--release`."
        );
    }
    if iterations == 0 {
        report_line!("error: --iterations must be at least 1");
        return ExitCode::FAILURE;
    }
    let program = match program::read_program(file) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let arguments = match program::entry_arguments(&program, script, program_arguments) {
        Ok(arguments) => arguments,
        Err(code) => return code,
    };
    println!(
        "timing: `main:args:` only, after assembling the application -- parsing, resolving, \
         bootstrapping, and compiling all happened once beforehand and are excluded. \
         {iterations} timed run(s), after one discarded warm-up run."
    );
    match benchmark_entry(
        &program,
        iterations,
        cache_statistics,
        collector_statistics,
        jit_statistics,
        &arguments,
    ) {
        Ok(statistics) => {
            println!(
                "median {:?} (min {:?}, max {:?}, n={})",
                statistics.median, statistics.min, statistics.max, statistics.iterations
            );
            if let Some(cache_statistics) = &statistics.cache_statistics {
                entry::print_cache_statistics(cache_statistics);
            }
            if let Some(collector_statistics) = &statistics.collector_statistics {
                entry::print_collector_statistics(collector_statistics);
            }
            if let Some(jit_statistics) = &statistics.jit_statistics {
                entry::print_jit_statistics(jit_statistics);
            }
        }
        Err(code) => return code,
    }
    ExitCode::SUCCESS
}
