//! `ns-next bench`.

use std::path::Path;
use std::process::ExitCode;

use crate::entry;
use crate::source::{self, LoadedSource};

/// The timing summary.
struct BenchStats {
    iterations: u32,
    median: std::time::Duration,
    min: std::time::Duration,
    max: std::time::Duration,
}

/// Renders an `instantiate`/`send` failure for `bench`'s purposes. Neither
/// call site treats it as fatal to the whole process the way `run` does --
/// `execute` folds it into its `Result` instead -- so this
/// produces a `String`, not an `ExitCode`.
fn signal_message(signal: ns_runtime::HostSignal) -> String {
    match signal {
        ns_runtime::HostSignal::Error(e) => format!("Uncaught Newspeak error: {}", e.message),
        ns_runtime::HostSignal::Return => {
            "internal: non-local return escaped the entry send".to_string()
        }
    }
}

/// One instantiate-and-send sample. Instantiation happens but is
/// deliberately *not* part of the timed span: it is fixed, once-per-run
/// setup (an empty slot vector filled in and a fresh identity hash), not
/// the repeated dispatch this benchmark measures. It cannot be hoisted
/// outside the loop either, though -- a `main:args:`-shaped fixture mutates
/// its own instance variables (loop counters, accumulators), so sending the
/// same instance a second time would time a cheaper run against
/// already-mutated state instead of the same workload every iteration.
fn timed_send(
    interp: &mut ns_runtime::Interpreter,
    class: &ns_runtime::Root,
    factory_args: &[ns_runtime::Root],
    selector: ns_runtime::Symbol,
    entry_args: &[ns_runtime::Root],
) -> Result<std::time::Duration, String> {
    let instance = interp
        .instantiate(class, factory_args)
        .map_err(signal_message)?;
    let start = std::time::Instant::now();
    let outcome = interp.send(&instance, selector, entry_args);
    let elapsed = start.elapsed();
    outcome.map(|_| elapsed).map_err(signal_message)
}

/// Bootstraps and loads the module (which compiles it), discards one
/// warm-up `timed_send`, times `iterations` further ones, and reduces the
/// samples to `BenchStats`. Bootstrapping, loading and compiling all happen
/// before the loop -- once-per-process costs, excluded from the per-send
/// timing for the same reason instantiation is.
fn bench_entry(loaded: &LoadedSource, send: &str, iterations: u32) -> Result<BenchStats, String> {
    let (mut interp, class) = source::bootstrap(loaded, ns_runtime::Lowering::Optimized);
    let platform = interp.platform();
    let factory_args = entry::factory_args_for(
        interp.heap_ref(),
        entry::class_ref(&interp, &class),
        &platform,
    )?;
    let entry_args_list = entry::entry_args(&interp, send, &platform)?;
    let selector = ns_runtime::Symbol::intern(send);

    // Warm-up: discarded. It exercises the same allocation and lookup
    // paths the timed runs will, so the first timed sample does not carry
    // a one-time cost (e.g. a lazily-populated cache) the rest do not.
    timed_send(
        &mut interp,
        &class,
        &factory_args,
        selector,
        &entry_args_list,
    )?;

    let mut timings = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        timings.push(timed_send(
            &mut interp,
            &class,
            &factory_args,
            selector,
            &entry_args_list,
        )?);
    }
    timings.sort();
    let min = timings[0];
    let max = timings[timings.len() - 1];
    // Median of N, not mean: a mean lets one
    // GC-pause-sized outlier (there is no GC yet, but a scheduler
    // preemption is the same shape of problem) drag the whole number with
    // it, while the median shrugs off any one such sample.
    let mid = timings.len() / 2;
    let median = if timings.len() % 2 == 1 {
        timings[mid]
    } else {
        (timings[mid - 1] + timings[mid]) / 2
    };
    Ok(BenchStats {
        iterations,
        median,
        min,
        max,
    })
}

/// `ns-next bench`: see the `Bench` variant's doc comment for what is
/// timed and why, and `bench_entry`/`timed_send` for how.
pub(crate) fn execute(file: &Path, send: &str, iterations: u32) -> ExitCode {
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: this is a debug build; the timings below measure debug-build overhead, \
             not the runtime. Rebuild and re-run with `--release`."
        );
    }
    if iterations == 0 {
        eprintln!("error: --iterations must be at least 1");
        return ExitCode::FAILURE;
    }
    let loaded = match source::load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    println!(
        "timing: the entry send `{send}` only, after instantiation -- parsing, resolving, \
         bootstrapping, and compiling all happened once beforehand and are excluded. \
         {iterations} timed run(s), after one discarded warm-up run."
    );
    match bench_entry(&loaded, send, iterations) {
        Ok(stats) => println!(
            "median {:?} (min {:?}, max {:?}, n={})",
            stats.median, stats.min, stats.max, stats.iterations
        ),
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
