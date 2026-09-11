#![warn(clippy::all)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use clap::{Parser, Subcommand};
use ns_syntax::{Diagnostic, Severity};

#[derive(Parser)]
#[command(name = "ns-next", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Parse a single .ns file.
    Parse {
        file: PathBuf,
        #[arg(long)]
        ast: bool,
    },
    /// Parse every .ns file under a directory.
    ParseCorpus {
        dir: PathBuf,
        #[arg(long)]
        blacklist: Option<PathBuf>,
    },
    /// Run every whitelisted .ns file under a directory, from source and as
    /// a built binary, and check each against its recorded expected output.
    RunCorpus {
        dir: PathBuf,
        #[arg(long)]
        whitelist: Option<PathBuf>,
    },
    /// Parse, resolve, bootstrap, load, instantiate, and run a .ns file
    /// end to end.
    Run {
        file: PathBuf,
        /// The entry selector to send after instantiation, e.g.
        /// `main:args:` (the default, per the spec's Core Library
        /// convention) or `--send someSelector` for a fixture that isn't
        /// a full `main:args:`-shaped application.
        #[arg(long, default_value = "main:args:")]
        send: String,
    },
    /// Run a file under the bytecode engine, then serialize the resulting
    /// (quiesced) world to an image file.
    Snapshot {
        file: PathBuf,
        out: PathBuf,
        /// A selector to send (with the arguments `run` supplies for its
        /// arity) before snapshotting; its result becomes a second root.
        #[arg(long)]
        send: Option<String>,
        /// Store the module's source text in the image (the debug
        /// variant). Off by default: a loaded world runs from the image's
        /// code table and mixin table, so the source is never needed to
        /// run it -- only to read it back as `.ns`.
        #[arg(long)]
        debug_source: bool,
    },
    /// Deserialize an artifact and start it. An image already holds its
    /// instance; a stateless binary is instantiated here and sent the entry
    /// selector it recorded. --send overrides that selector, with the
    /// arguments `run` supplies for its arity.
    Load {
        image: PathBuf,
        #[arg(long)]
        send: Option<String>,
        /// Print the source text a `--debug-source` image carries, instead
        /// of loading and reporting it. Fails on a source-free image,
        /// which by design has none.
        #[arg(long)]
        print_source: bool,
    },
    /// Time the entry send: bootstrap, load and compile once, then discard
    /// a warm-up run and time `--iterations` further runs, reporting the
    /// median (with min and max) time for the entry send alone.
    ///
    /// Only the entry send is timed -- parsing, resolving, bootstrapping,
    /// and compiling all happen once beforehand and are excluded, because
    /// what Milestone 4's inline caches change is repeated dispatch, not
    /// one-time startup. ALWAYS measure with `--release`: a debug build's
    /// numbers are not comparable to anything and this subcommand warns on
    /// stderr when it detects one.
    Bench {
        file: PathBuf,
        /// The entry selector to time, sized exactly as `run --send` sizes
        /// it.
        #[arg(long, default_value = "main:args:")]
        send: String,
        /// How many timed runs to take the median of, after one discarded
        /// warm-up run.
        #[arg(long, default_value_t = 10)]
        iterations: u32,
    },
    /// Print one compiled body: a method, or with --initializer the target
    /// class's instance initializer. Takes either a .ns file, which is
    /// compiled first, or a serialized artifact, whose bodies are decoded
    /// rather than compiled. Nothing is instantiated or run either way.
    Disasm {
        file: PathBuf,
        /// The method selector, e.g. `main:args:`.
        selector: Option<String>,
        /// Disassemble the target's compiled instance initializer instead.
        #[arg(long)]
        initializer: bool,
        /// A dot-separated mixin path rooted at the module's own class
        /// name, e.g. `Outer.Inner` for a nested class or `Outer.class`
        /// for a class side. Defaults to the root class itself.
        #[arg(long)]
        class: Option<String>,
    },
    /// Print an artifact's structure: header flags, section offsets, the
    /// symbol table, the code table with every body named, the mixin table,
    /// and -- for an image -- its object graph, read record by record.
    Inspect {
        artifact: PathBuf,
        /// Render everything: the mixin rows expanded, and every compiled
        /// body disassembled. The default is a table of contents.
        #[arg(long)]
        full: bool,
    },
    /// Compile a file into a stateless deployment binary: code and
    /// structure, no instance state. Nothing is instantiated -- the primary
    /// factory and every initializer run when the binary is loaded.
    Build {
        file: PathBuf,
        out: PathBuf,
        /// The selector to record as the entry point. Defaults to
        /// `main:args:` when the root class declares it.
        #[arg(long)]
        entry: Option<String>,
        /// Store the module's source text in the binary (the debug variant).
        #[arg(long)]
        debug_source: bool,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Cmd::Parse { file, ast } => cmd_parse(&file, ast),
        Cmd::ParseCorpus { dir, blacklist } => cmd_parse_corpus(&dir, blacklist.as_deref()),
        Cmd::RunCorpus { dir, whitelist } => cmd_run_corpus(&dir, whitelist.as_deref()),
        Cmd::Run { file, send } => cmd_run(&file, &send),
        Cmd::Snapshot {
            file,
            out,
            send,
            debug_source,
        } => cmd_snapshot(&file, &out, send.as_deref(), debug_source),
        Cmd::Load {
            image,
            send,
            print_source,
        } => cmd_load(&image, send.as_deref(), print_source),
        Cmd::Bench {
            file,
            send,
            iterations,
        } => cmd_bench(&file, &send, iterations),
        Cmd::Disasm {
            file,
            selector,
            initializer,
            class,
        } => cmd_disasm(&file, selector.as_deref(), initializer, class.as_deref()),
        Cmd::Inspect { artifact, full } => cmd_inspect(&artifact, full),
        Cmd::Build {
            file,
            out,
            entry,
            debug_source,
        } => cmd_build(&file, &out, entry.as_deref(), debug_source),
    }
}

fn cmd_parse(file: &Path, ast: bool) -> ExitCode {
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: error: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    match ns_syntax::parse(&src) {
        Ok(module) => {
            if ast {
                println!("{module:#?}");
            }
            println!("OK {}", file.display());
            ExitCode::SUCCESS
        }
        Err(diags) => {
            print_diagnostics(file, &src, &diags);
            ExitCode::FAILURE
        }
    }
}

/// A module read, parsed, and resolved -- everything `run`, `disasm`, and
/// `snapshot` share before an interpreter exists.
struct LoadedSource {
    source: String,
    origin: ns_runtime::SourceOrigin,
    resolved: ns_runtime::ResolvedModule,
}

/// Reads, parses, and resolves `file`, reporting diagnostics exactly as
/// the Milestone 1 `run` did. `Err` carries the exit code to return.
fn load_source(file: &Path) -> Result<LoadedSource, ExitCode> {
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: reading {}: {e}", file.display());
            return Err(ExitCode::FAILURE);
        }
    };
    let module = match ns_syntax::parse(&source) {
        Ok(m) => m,
        Err(diags) => {
            print_diagnostics(file, &source, &diags);
            return Err(ExitCode::FAILURE);
        }
    };
    let origin = ns_runtime::SourceOrigin::File(file.to_path_buf());
    let resolved = match ns_runtime::resolve(module, source.as_str().into(), origin.clone()) {
        Ok(r) => r,
        Err(errors) => {
            for e in errors {
                eprintln!("resolve error: {}", e.message);
            }
            return Err(ExitCode::FAILURE);
        }
    };
    Ok(LoadedSource {
        source,
        origin,
        resolved,
    })
}

/// Bootstraps an interpreter and loads the module's root class, which
/// compiles every body onto the declaration it belongs to -- before anything
/// is instantiated, so the root class's instance initializer runs compiled
/// too.
fn bootstrap(loaded: &LoadedSource) -> (ns_runtime::Interpreter, ns_runtime::ClassRef) {
    let mut interp = ns_runtime::Interpreter::bootstrap();
    let class = interp.load_module(&loaded.resolved);
    (interp, class)
}

/// Renders the entry send's outcome exactly as Milestone 1's `run` did --
/// `program_outcomes.rs` pins these strings byte for byte, so they live here
/// once and are never duplicated.
fn report_entry_send(result: Result<ns_runtime::Value, ns_runtime::Signal>) -> ExitCode {
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

/// The argument list `cmd_run`, `cmd_snapshot` and `cmd_load` pass to
/// `interp.instantiate`,
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
fn factory_args_for(
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
/// It used to be `[platform, nil]` unconditionally, which is right only
/// for a two-parameter `main:args:`. Anything else got a frame whose
/// argument count did not match its declaration -- and a method activation
/// addresses its arguments and temporaries as one run of registers, so the
/// surplus silently shifted every temporary rather than failing.
/// `invoke_method` now refuses a mismatch outright; sizing the list here
/// is what keeps a legitimate `--send` from tripping that refusal.
///
/// The template is the Core Library's `main: platform args: arguments`
/// convention truncated to the selector's arity: a unary entry point takes
/// nothing, a one-parameter one takes the platform, a two-parameter one
/// takes the platform and `Nil` for the environment-args list -- there is
/// still no command-line-argument passthrough into Newspeak. Beyond two
/// the CLI has nothing to offer, and says so rather than padding.
fn entry_args(
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

/// `ns-next run`: parse, resolve, bootstrap, load,
/// instantiate the root class with whatever its declared primary factory
/// takes, send the entry selector with whatever `entry_args` sizes for it,
/// report.
fn cmd_run(file: &Path, send: &str) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let (mut interp, class) = bootstrap(&loaded);
    let platform = interp.platform();
    let factory_args = match factory_args_for(interp.heap_ref(), class, platform) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let instance = match interp.instantiate(class, factory_args) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    let args = match entry_args(send, platform) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let selector = ns_runtime::Symbol::intern(send);
    report_entry_send(interp.send(instance, selector, args))
}

/// `ns-next snapshot`: `run`'s pipeline up to (and optionally including)
/// the entry send, then a serialized image of the resulting world. The
/// image's own code table is what a later `load` runs.
fn cmd_snapshot(file: &Path, out: &Path, send: Option<&str>, debug_source: bool) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let (mut interp, class) = bootstrap(&loaded);
    let platform = interp.platform();
    let factory_args = match factory_args_for(interp.heap_ref(), class, platform) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let instance = match interp.instantiate(class, factory_args) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    let mut roots = vec![instance];
    if let Some(selector) = send {
        let args = match entry_args(selector, platform) {
            Ok(args) => args,
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::FAILURE;
            }
        };
        match interp.send(instance, ns_runtime::Symbol::intern(selector), args) {
            Ok(result) => roots.push(result),
            Err(signal) => return report_entry_send(Err(signal)),
        }
    }
    let bytes = match ns_runtime::image::snapshot(
        interp.heap_ref(),
        debug_source.then_some(loaded.source.as_str()),
        &loaded.origin,
        class,
        &platform,
        &roots,
    ) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: snapshot failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: writing {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}

/// `ns-next load`: deserialize an artifact and start it. What "start" means
/// depends on which of the two shapes it is, and the header says which.
///
/// A **stateful image** already carries its instance, so a `--send` is sent
/// to the root it decoded, and without one the command reports what it loaded
/// and stops.
///
/// A **stateless binary** carries no instance at all. Its root class is
/// instantiated here, against this interpreter's own freshly bootstrapped
/// platform, and then the selector it recorded is sent -- `--send` overrides
/// that selector but not the instantiation. Running the factory and every
/// initializer at load is what makes the artifact stateless in the first
/// place: it has never held a capability.
///
/// Either way the entry send goes through the same `entry_args` convention
/// and the same `report_entry_send` as `run`, so `load` of an artifact prints
/// exactly what `run` of its source prints and nothing else. A malformed
/// artifact is a message, never a stack trace.
fn cmd_load(image: &Path, send: Option<&str>, print_source: bool) -> ExitCode {
    let bytes = match std::fs::read(image) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading {}: {e}", image.display());
            return ExitCode::FAILURE;
        }
    };
    let ns_runtime::image::LoadedImage {
        mut interp,
        root_class,
        roots,
        origin: _,
        source,
        entry,
        stateless,
    } = match ns_runtime::image::load(&bytes) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: loading {}: {e}", image.display());
            return ExitCode::FAILURE;
        }
    };
    if print_source {
        let Some(source) = source else {
            eprintln!(
                "error: {} carries no source (it was not written with --debug-source)",
                image.display()
            );
            return ExitCode::FAILURE;
        };
        print!("{source}");
        return ExitCode::SUCCESS;
    }
    // `stateless` (the decoded header flag), not `roots.is_empty()`, decides
    // whether to instantiate: it says what the artifact *is*, where an empty
    // `roots` is only what a binary happens to look like. `image::snapshot`
    // is a library API that a caller can hand an empty `roots` slice to,
    // producing a valid *stateful* image that carries zero root values --
    // and that image must still be treated as stateful here, since
    // instantiating it would re-run a primary factory and every instance
    // initializer in a world where they already ran, minting a second,
    // unrelated instance in place of the (absent) one the artifact
    // deliberately carried. A decoded `stateless: true` with a non-empty
    // `roots` would mean `image::load` built an inconsistent `LoadedImage`
    // -- a decoder bug, not a case this branch tolerates.
    let instance = if stateless {
        // A binary carries no instance: the primary factory and every
        // initializer run here, against this interpreter's freshly
        // bootstrapped platform, so capabilities enter at start time rather
        // than having been baked in at build time. Instantiating in the CLI
        // rather than in `image::load` keeps an existing boundary: deciding
        // *that* a command-line entry point supplies the platform is a CLI
        // concern -- nothing in the runtime needs to know it was invoked
        // from a terminal.
        let platform = interp.platform();
        let factory_args = match factory_args_for(interp.heap_ref(), root_class, platform) {
            Ok(args) => args,
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::FAILURE;
            }
        };
        match interp.instantiate(root_class, factory_args) {
            Ok(v) => v,
            Err(signal) => return report_signal(signal),
        }
    } else {
        // Pre-existing behaviour for a stateful image, unconditional on
        // `roots.len()`: no `--send` means report the root count and stop,
        // sending nothing -- including a stateful image with zero roots,
        // which reports "0 root value(s)" rather than being mistaken for a
        // binary.
        let Some(_) = send else {
            println!("loaded {}: {} root value(s)", image.display(), roots.len());
            return ExitCode::SUCCESS;
        };
        let Some(instance) = roots.into_iter().next() else {
            eprintln!("error: image has no root instance to send to");
            return ExitCode::FAILURE;
        };
        instance
    };
    // `--send` overrides whatever the artifact recorded. In practice the
    // `None` arm below is unreachable today: a binary always records an
    // entry (`image::load` refuses a stateless artifact without one), and a
    // stateful image with neither already returned above, in the `else`
    // (stateful) branch. It stays as a real check rather than an
    // `unreachable!()` because that pairing is an invariant of the image
    // format, not of this function -- a future image that records an entry
    // too should fall through to sending it, not panic.
    let selector = match send.or_else(|| entry.map(ns_runtime::Symbol::as_str)) {
        Some(s) => s,
        None => {
            eprintln!("error: this artifact records no entry point; pass --send");
            return ExitCode::FAILURE;
        }
    };
    let platform = interp.platform();
    let args = match entry_args(selector, platform) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    report_entry_send(interp.send(instance, ns_runtime::Symbol::intern(selector), args))
}

/// The timing summary.
struct BenchStats {
    iterations: u32,
    median: std::time::Duration,
    min: std::time::Duration,
    max: std::time::Duration,
}

/// Renders an `instantiate`/`send` failure for `bench`'s purposes. Neither
/// call site treats it as fatal to the whole process the way `run` does --
/// `cmd_bench` folds it into its `Result` instead -- so this
/// produces a `String`, not an `ExitCode`.
fn signal_message(signal: ns_runtime::Signal) -> String {
    match signal {
        ns_runtime::Signal::Error(e) => format!("Uncaught Newspeak error: {}", e.message),
        ns_runtime::Signal::Return(..) => {
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
    class: ns_runtime::ClassRef,
    factory_args: &[ns_runtime::Value],
    selector: ns_runtime::Symbol,
    entry_args: &[ns_runtime::Value],
) -> Result<std::time::Duration, String> {
    let instance = interp
        .instantiate(class, factory_args.to_vec())
        .map_err(signal_message)?;
    let start = std::time::Instant::now();
    let outcome = interp.send(instance, selector, entry_args.to_vec());
    let elapsed = start.elapsed();
    outcome.map(|_| elapsed).map_err(signal_message)
}

/// Bootstraps and loads the module (which compiles it), discards one
/// warm-up `timed_send`, times `iterations` further ones, and reduces the
/// samples to `BenchStats`. Bootstrapping, loading and compiling all happen
/// before the loop -- once-per-process costs, excluded from the per-send
/// timing for the same reason instantiation is.
fn bench_entry(loaded: &LoadedSource, send: &str, iterations: u32) -> Result<BenchStats, String> {
    let (mut interp, class) = bootstrap(loaded);
    let platform = interp.platform();
    let factory_args = factory_args_for(interp.heap_ref(), class, platform)?;
    let entry_args_list = entry_args(send, platform)?;
    let selector = ns_runtime::Symbol::intern(send);

    // Warm-up: discarded. It exercises the same allocation and lookup
    // paths the timed runs will, so the first timed sample does not carry
    // a one-time cost (e.g. a lazily-populated cache) the rest do not.
    timed_send(
        &mut interp,
        class,
        &factory_args,
        selector,
        &entry_args_list,
    )?;

    let mut timings = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        timings.push(timed_send(
            &mut interp,
            class,
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
fn cmd_bench(file: &Path, send: &str, iterations: u32) -> ExitCode {
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
    let loaded = match load_source(file) {
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
/// there is none. Shared by both of `cmd_disasm`'s branches so each can
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
/// world.
fn cmd_disasm(
    file: &Path,
    selector: Option<&str>,
    initializer: bool,
    class_path: Option<&str>,
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
        let loaded = match load_source(file) {
            Ok(l) => l,
            Err(code) => return code,
        };
        let (interp, class) = bootstrap(&loaded);
        match root_mixin_of(interp.heap_ref(), class) {
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

/// `ns-next inspect`: what is in this artifact. Read-only -- nothing is
/// bootstrapped, instantiated or run, so inspecting a hostile file exercises
/// only the decoders.
fn cmd_inspect(artifact: &Path, full: bool) -> ExitCode {
    let bytes = match std::fs::read(artifact) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading {}: {e}", artifact.display());
            return ExitCode::FAILURE;
        }
    };
    match ns_runtime::image::inspect::inspect(&bytes) {
        Ok(report) => {
            let rendered = if full {
                ns_runtime::image::inspect::render_full(&report)
            } else {
                ns_runtime::image::inspect::render(&report)
            };
            print!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {}: {e}", artifact.display());
            ExitCode::FAILURE
        }
    }
}

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
/// Walking past the root level (`app = a.superclass.clone()`) is safe only
/// because of an invariant this function relies on but does not check:
/// every non-own mixin reachable from a root class's superclass chain today
/// is a builtin's, and a builtin mixin is `Mixin::empty()` -- empty
/// `methods`, empty `nested` -- since native methods dispatch through
/// `builtins::dispatch_native`, never through `Mixin.methods`, and
/// cross-module inheritance does not exist yet. So the ancestor branch can
/// never actually return `true` today; the loop climbs `superclass` only to
/// stay correct if a root class's chain ever grows past one level of
/// `Mixin::empty()`. If ns-next later gains cross-module inheritance, or a
/// core-library superclass written in Newspeak, an ancestor mixin could
/// carry real `methods`/`nested` entries that `mixins::encode` never
/// interns for *this* module's symbol table (it only walks the module's own
/// declaration tree) -- and this check would then accept a selector absent
/// from that table, silently breaking the guarantee above instead of
/// refusing the build. No test catches that today: every fixture here
/// inherits directly from `Object`.
fn root_class_answers(
    heap: &ns_runtime::Heap,
    class: ns_runtime::ClassRef,
    selector: &str,
) -> bool {
    let sym = ns_runtime::Symbol::intern(selector);
    let mut app = heap.class_application(class);
    while let Some(a) = app {
        if a.mixin.methods.contains_key(&sym) {
            return true;
        }
        if a.mixin.nested.iter().any(|n| n.name == sym) {
            return true;
        }
        app = a.superclass.clone();
    }
    false
}

/// `ns-next build`: `run`'s pipeline up to engine installation, and then
/// deliberately no instantiation.
fn cmd_build(file: &Path, out: &Path, entry: Option<&str>, debug_source: bool) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let (mut interp, class) = bootstrap(&loaded);

    let entry = match entry {
        Some(e) => e.to_string(),
        None if root_class_answers(interp.heap_ref(), class, "main:args:") => {
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

    if let Err(msg) = entry_args(&entry, interp.platform()) {
        eprintln!("error: cannot use `{entry}` as an entry point: {msg}");
        return ExitCode::FAILURE;
    }
    if !root_class_answers(interp.heap_ref(), class, &entry) {
        eprintln!(
            "error: the root class of {} does not answer `{entry}`",
            file.display()
        );
        return ExitCode::FAILURE;
    }

    let bytes = match ns_runtime::image::build(
        interp.heap_ref(),
        debug_source.then_some(loaded.source.as_str()),
        &loaded.origin,
        class,
        ns_runtime::Symbol::intern(&entry),
    ) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: build failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: writing {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}

fn cmd_parse_corpus(dir: &Path, blacklist: Option<&Path>) -> ExitCode {
    let bl_path = blacklist
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.parent().unwrap_or(dir).join("blacklist.toml"));
    let blacklist = match load_blacklist(&bl_path) {
        Ok(bl) => bl,
        Err(e) => {
            eprintln!("error: blacklist {}: {e}", bl_path.display());
            return ExitCode::FAILURE;
        }
    };

    let mut files: Vec<PathBuf> = Vec::new();
    collect_ns_files(dir, &mut files);
    files.sort();

    let (mut parsed, mut unexpected_fail, mut expected_fail, mut stale) = (0, 0, 0, 0);
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(f).unwrap_or_default();
        let ok = ns_syntax::parse(&src).is_ok();
        let listed = blacklist.contains(&name);
        match (ok, listed) {
            (true, false) => parsed += 1,
            (true, true) => {
                stale += 1;
                eprintln!("STALE blacklist entry (now parses): {name}");
            }
            (false, true) => expected_fail += 1,
            (false, false) => {
                unexpected_fail += 1;
                eprintln!("FAIL {name}");
            }
        }
    }
    println!(
        "parsed {parsed}, failed {unexpected_fail}, expected-fail {expected_fail}, stale {stale}"
    );
    if unexpected_fail == 0 && stale == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// One row of `corpus/runnable.toml`: a corpus module that is expected to
/// run, with everything needed to check that it still does.
///
/// `send` is per entry rather than hardcoded to `main:args:` because the
/// corpus modules that run today have no `main:args:` -- they are library
/// modules whose reachable entry point is a public method or a public
/// nested class's implicit accessor. A gate that assumed one selector
/// could not list any of them.
#[derive(serde::Deserialize)]
struct RunnableEntry {
    file: String,
    send: String,
    expected_status: i32,
    expected_stdout: String,
    /// Why this module is on the list. Required, like the blacklist's own
    /// `reason`: the list is meant to grow one deliberate row at a time.
    #[allow(dead_code)]
    reason: String,
}

fn load_whitelist(path: &Path) -> Result<Vec<RunnableEntry>, String> {
    #[derive(serde::Deserialize)]
    struct Wl {
        #[serde(default)]
        entry: Vec<RunnableEntry>,
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "note: no whitelist at {} (treating as empty)",
                path.display()
            );
            return Ok(Vec::new());
        }
        Err(e) => return Err(e.to_string()),
    };
    let wl: Wl = toml::from_str(&text).map_err(|e| e.to_string())?;
    Ok(wl.entry)
}

/// What one `ns-next run` or `load` of a whitelisted module produced.
struct RunOutcome {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Runs one module through a *child* `ns-next run`.
///
/// A child process rather than an in-process call on purpose: the module's
/// stdout, which Newspeak code writes straight to the process's stdout
/// through `platform transcript`, is what gets compared, and running it in
/// this process would interleave it with the gate's own output.
fn run_one(exe: &Path, file: &Path, send: &str) -> Result<RunOutcome, String> {
    let out = std::process::Command::new(exe)
        .arg("run")
        .arg(file)
        .arg("--send")
        .arg(send)
        .output()
        .map_err(|e| format!("could not run {}: {e}", exe.display()))?;
    Ok(RunOutcome {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// A scratch directory for one row's binary lane, removed on drop so a
/// `run-corpus` run leaves the temp directory clean on every exit path --
/// including the early `?`/`continue` returns scattered through the caller --
/// without every call site having to remember to clean up itself.
struct ScratchDir(PathBuf);

impl ScratchDir {
    /// `tag` is only for a human reading the temp directory mid-run; the
    /// counter is what actually guarantees uniqueness across the rows this
    /// process runs, alongside the process id that separates concurrent
    /// `run-corpus` invocations.
    fn new(tag: &str) -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ns-next-run-corpus-{}-{n}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("scratch dir {}: {e}", dir.display()))?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best-effort: a leftover temp directory is a nuisance, never a
        // correctness problem, so a removal failure (e.g. a virus scanner
        // still holding the file open) must not turn into a panic-during-
        // unwind or a spurious second failure report for the row.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs one module the way a deployed binary would: `build` it with
/// `--entry <send>` into a scratch file, then `load` it with no `--send` so
/// the artifact's own recorded entry is what starts it. That is deliberate,
/// not an oversight -- supplying `--send` again here would exercise the same
/// override path `build_load.rs` already covers, not the round trip this
/// gate exists to check: that the entry survives being written into the
/// artifact and read back out of it.
///
/// The scratch file lives under the system temp directory, never under
/// `corpus/`, so a `run-corpus` invocation leaves the working tree exactly
/// as it found it; `ScratchDir`'s `Drop` removes it once this row is done
/// whether the row passed or failed.
fn run_one_binary(exe: &Path, file: &Path, send: &str) -> Result<RunOutcome, String> {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "row".to_string());
    let scratch = ScratchDir::new(&stem)?;
    let artifact = scratch.path().join("app.nsb");

    let build = std::process::Command::new(exe)
        .arg("build")
        .arg(file)
        .arg(&artifact)
        .arg("--entry")
        .arg(send)
        .output()
        .map_err(|e| format!("could not build {}: {e}", exe.display()))?;
    if !build.status.success() {
        return Err(format!(
            "build failed: {}",
            String::from_utf8_lossy(&build.stderr).trim_end()
        ));
    }

    let out = std::process::Command::new(exe)
        .arg("load")
        .arg(&artifact)
        .output()
        .map_err(|e| format!("could not load {}: {e}", exe.display()))?;
    Ok(RunOutcome {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// `ns-next run-corpus`: the mirror image of `parse-corpus`. `parse-corpus`
/// works off a blacklist because parsing the corpus is expected to succeed
/// everywhere; running it is not, so this works off a whitelist -- a module
/// counts as running only once someone has recorded what running it
/// produces. Growth is therefore deliberate and a regression is loud.
///
/// Every entry is run from source and checked against the expectation: the
/// recorded exit status and stdout, and -- for a row expected to exit 0 -- an
/// empty stderr. It is then run a second way -- built into a stateless binary
/// with `--entry <row.send>` and loaded with no `--send`, so the recorded
/// entry is what starts it -- and checked against the same exit status and
/// stdout, with stderr checked against the source run's. A row's `send`
/// doubling as the build's `--entry` is why `runnable.toml` needs no field of
/// its own for this lane. Each row still counts once in `ran`/`failed`: the
/// first lane that fails decides the row.
///
/// What that is worth, stated honestly: with `expected_stdout` empty on
/// every real row -- no corpus module that currently runs writes to the
/// transcript at all -- this is a tripwire that a whitelisted module still
/// instantiates and still answers one message without error, plus a check
/// that the same module still starts from a built binary instead of from
/// source. It is not yet a differential harness over observable *output*
/// from real modules; it becomes one on the day a row can record something
/// printed. Treat a green `run-corpus` as "nothing regressed", not as "the
/// corpus is covered".
fn cmd_run_corpus(dir: &Path, whitelist: Option<&Path>) -> ExitCode {
    let wl_path = whitelist
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.parent().unwrap_or(dir).join("runnable.toml"));
    let entries = match load_whitelist(&wl_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: whitelist {}: {e}", wl_path.display());
            return ExitCode::FAILURE;
        }
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot locate this executable to re-run modules with: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut present: Vec<PathBuf> = Vec::new();
    collect_ns_files(dir, &mut present);
    let listed: std::collections::HashSet<&str> = entries.iter().map(|e| e.file.as_str()).collect();
    let skipped = present
        .iter()
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| listed.contains(n.to_string_lossy().as_ref()))
        })
        .count();

    // `ran` counts rows whose module actually executed, so a row naming a
    // deleted file, or one whose child could not be spawned, reports as a
    // failure without inflating the number a reader uses to judge coverage.
    let (mut ran, mut failed) = (0, 0);
    for entry in &entries {
        let file = dir.join(&entry.file);
        if !file.is_file() {
            failed += 1;
            eprintln!("FAIL {}: no such file under {}", entry.file, dir.display());
            continue;
        }
        let source = match run_one(&exe, &file, &entry.send) {
            Ok(o) => o,
            Err(e) => {
                failed += 1;
                eprintln!("FAIL {}: {e}", entry.file);
                continue;
            }
        };
        ran += 1;
        if source.status != Some(entry.expected_status) {
            failed += 1;
            eprintln!(
                "FAIL {}: expected exit {}, got {} (stderr: {})",
                entry.file,
                entry.expected_status,
                match source.status {
                    Some(c) => c.to_string(),
                    None => "signal".to_string(),
                },
                source.stderr.trim_end()
            );
            continue;
        }
        if source.stdout != entry.expected_stdout {
            failed += 1;
            eprintln!(
                "FAIL {}: expected stdout {:?}, got {:?}",
                entry.file, entry.expected_stdout, source.stdout
            );
            continue;
        }
        // A successful run writes nothing to stderr. Without this, a warning
        // or a stray diagnostic that both lanes print identically would pass,
        // since the binary lane's stderr is compared with this run's only.
        if entry.expected_status == 0 && !source.stderr.is_empty() {
            failed += 1;
            eprintln!(
                "FAIL {}: expected no stderr, got {:?}",
                entry.file, source.stderr
            );
            continue;
        }

        let binary = match run_one_binary(&exe, &file, &entry.send) {
            Ok(o) => o,
            Err(e) => {
                failed += 1;
                eprintln!("FAIL {}: binary: {e}", entry.file);
                continue;
            }
        };
        if binary.status != Some(entry.expected_status) {
            failed += 1;
            eprintln!(
                "FAIL {}: binary: expected exit {}, got {} (stderr: {})",
                entry.file,
                entry.expected_status,
                match binary.status {
                    Some(c) => c.to_string(),
                    None => "signal".to_string(),
                },
                binary.stderr.trim_end()
            );
            continue;
        }
        if binary.stdout != entry.expected_stdout {
            failed += 1;
            eprintln!(
                "FAIL {}: binary: expected stdout {:?}, got {:?}",
                entry.file, entry.expected_stdout, binary.stdout
            );
            continue;
        }
        if binary.stderr != source.stderr {
            failed += 1;
            eprintln!(
                "FAIL {}: binary: stderr differs from the source run's: binary {:?}, source {:?}",
                entry.file, binary.stderr, source.stderr
            );
        }
    }
    println!("ran {ran}, failed {failed}, skipped {skipped}");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn collect_ns_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ns_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ns") {
            out.push(path);
        }
    }
}

/// Load the set of blacklisted file names.
///
/// A file that is present but unreadable or unparseable is a hard error
/// (returns `Err`): silently treating it as an empty blacklist would make
/// `parse-corpus` report phantom regressions with no explanation. A genuinely
/// absent file is treated as an empty blacklist with a one-line note — the
/// repo ships `corpus/blacklist.toml`, so absence is the unusual case and is
/// most likely a caller passing a bare directory with no companion file.
fn load_blacklist(path: &Path) -> Result<std::collections::HashSet<String>, String> {
    #[derive(serde::Deserialize)]
    struct Bl {
        #[serde(default)]
        entry: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        file: String,
        #[allow(dead_code)]
        reason: String,
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "note: no blacklist at {} (treating as empty)",
                path.display()
            );
            return Ok(std::collections::HashSet::new());
        }
        Err(e) => return Err(e.to_string()),
    };
    let bl: Bl = toml::from_str(&text).map_err(|e| e.to_string())?;
    Ok(bl.entry.into_iter().map(|e| e.file).collect())
}

fn print_diagnostics(file: &Path, src: &str, diags: &[Diagnostic]) {
    for d in diags {
        let (line, col) = line_col(src, d.span.start);
        let level = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!("{}:{line}:{col}: {level}: {}", file.display(), d.message);
    }
}

fn line_col(src: &str, offset: u32) -> (usize, usize) {
    let offset = offset as usize;
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in src.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}
