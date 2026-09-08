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
    /// Run every whitelisted .ns file under a directory, under both
    /// engines, and check each against its recorded expected output.
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
        #[arg(long, value_enum, default_value_t = EngineArg::Bytecode)]
        engine: EngineArg,
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
    /// Deserialize an image and optionally send one further message to
    /// its root instance (with the arguments `run` supplies for the
    /// selector's arity).
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
    /// Time the entry send under both engines: bootstrap, load, and (for
    /// the bytecode engine) compile once, then discard a warm-up run and
    /// time `--iterations` further runs, reporting each engine's median
    /// (with min and max) time for the entry send alone.
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
    /// Compile one method (or, with --initializer, the target class's
    /// instance initializer) and print its bytecode. Compile only: nothing
    /// is instantiated or run.
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
    /// symbol and mixin tables, and how much graph it carries.
    Inspect { artifact: PathBuf },
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

/// `--engine`: which evaluator runs Newspeak-level code. `bytecode` is the
/// default; `tree-walk` is the Milestone 1 interpreter, kept permanently as
/// the differential-testing oracle (M2 spec, "`ns-cli` additions").
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
enum EngineArg {
    Bytecode,
    TreeWalk,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Cmd::Parse { file, ast } => cmd_parse(&file, ast),
        Cmd::ParseCorpus { dir, blacklist } => cmd_parse_corpus(&dir, blacklist.as_deref()),
        Cmd::RunCorpus { dir, whitelist } => cmd_run_corpus(&dir, whitelist.as_deref()),
        Cmd::Run { file, send, engine } => cmd_run(&file, &send, engine),
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
        Cmd::Inspect { artifact } => cmd_inspect(&artifact),
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

/// Bootstraps an interpreter, loads the module's root class, and installs
/// the requested engine -- *before* anything is instantiated, so the root
/// class's instance initializer runs under it too (M2 spec, "`ns-cli`
/// additions").
fn bootstrap_with_engine(
    loaded: &LoadedSource,
    engine: EngineArg,
) -> (ns_runtime::Interpreter, ns_runtime::ClassRef) {
    let mut interp = ns_runtime::Interpreter::bootstrap();
    let class = interp.load_module(&loaded.resolved);
    if engine == EngineArg::Bytecode {
        let compiled = ns_runtime::compiler::compile_module(&class, &loaded.resolved.bindings);
        interp
            .set_engine(ns_runtime::Engine::Bytecode(compiled))
            // Only `Engine::TreeWalk` on a loaded world is ever refused,
            // and this interpreter was built from source a line ago.
            .expect("the bytecode engine is never refused");
    }
    (interp, class)
}

/// Renders the entry send's outcome exactly as Milestone 1's `run` did --
/// these strings are compared byte for byte across engines by the
/// differential harness, so they are shared and never duplicated.
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

/// The argument list `cmd_run`/`cmd_snapshot` pass to `interp.instantiate`,
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
    class: &ns_runtime::ClassRef,
    platform: ns_runtime::Value,
) -> Result<Vec<ns_runtime::Value>, String> {
    let factory = match &class.borrow().data {
        ns_runtime::ObjectData::Class(data) => data.application.mixin.factory,
        _ => None,
    };
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
            if factory.num_args == 1
                && factory.selector == ns_runtime::Symbol::intern("usingPlatform:") =>
        {
            Ok(vec![platform])
        }
        Some(factory) => Err(format!(
            "cannot instantiate `{}`: its primary factory `{}` takes {} argument(s), and \
             this CLI only knows how to send a zero-argument factory or a one-argument \
             `usingPlatform:`",
            class_name(class),
            factory.selector,
            factory.num_args
        )),
    }
}

/// The mixin name behind a class object, for `factory_args_for`'s error
/// message. `class` is always `ObjectData::Class` at every call site (both
/// come straight out of `bootstrap_with_engine`), so the fallback is
/// unreachable in practice and exists only to keep this a total function.
fn class_name(class: &ns_runtime::ClassRef) -> String {
    match &class.borrow().data {
        ns_runtime::ObjectData::Class(data) => data.application.mixin.name.to_string(),
        _ => "<non-class>".to_string(),
    }
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

/// `ns-next run`: parse, resolve, bootstrap, load, install the engine,
/// instantiate the root class with whatever its declared primary factory
/// takes, send the entry selector with whatever `entry_args` sizes for it,
/// report.
fn cmd_run(file: &Path, send: &str, engine: EngineArg) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    let (mut interp, class) = bootstrap_with_engine(&loaded, engine);
    let platform = interp.platform();
    let factory_args = match factory_args_for(&class, platform.clone()) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let instance = match interp.instantiate(class, factory_args, &bindings) {
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
    report_entry_send(interp.send(instance, selector, args, &bindings, None))
}

/// `ns-next snapshot`: `run`'s pipeline up to (and optionally including)
/// the entry send, then a serialized image of the resulting world. Always
/// the bytecode engine -- a tree-walker closure carries `ClosureBody::Ast`,
/// which the format has no representation for, and the image's own code
/// table is what a later `load` runs.
fn cmd_snapshot(file: &Path, out: &Path, send: Option<&str>, debug_source: bool) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    let (mut interp, class) = bootstrap_with_engine(&loaded, EngineArg::Bytecode);
    let platform = interp.platform();
    let factory_args = match factory_args_for(&class, platform.clone()) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let instance = match interp.instantiate(class.clone(), factory_args, &bindings) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    let mut roots = vec![instance.clone()];
    if let Some(selector) = send {
        let args = match entry_args(selector, platform.clone()) {
            Ok(args) => args,
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::FAILURE;
            }
        };
        match interp.send(
            instance,
            ns_runtime::Symbol::intern(selector),
            args,
            &bindings,
            None,
        ) {
            Ok(result) => roots.push(result),
            Err(signal) => return report_entry_send(Err(signal)),
        }
    }
    let module = interp
        .compiled_module()
        .expect("bootstrap_with_engine installed the bytecode engine");
    let bytes = match ns_runtime::image::snapshot(
        debug_source.then_some(loaded.source.as_str()),
        &loaded.origin,
        &class,
        module,
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

/// `ns-next load`: deserialize a world and, with `--send`, send one
/// message to its root instance through the same `entry_args` convention
/// `run` uses -- and report it through the same `report_entry_send`, so a
/// `load --send main:args:` of a snapshot prints exactly what `run` prints
/// and nothing else. A malformed image is a message, never a stack trace.
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
        root_class: _,
        roots,
        origin: _,
        source,
        empty_bindings: bindings,
        // A later task wires `load` on a binary (empty `roots`, an
        // `entry` to send) into this command; until then it is only
        // ever handed an image, where both are `None`/`false`.
        entry: _,
        stateless: _,
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
    let Some(selector) = send else {
        println!("loaded {}: {} root value(s)", image.display(), roots.len());
        return ExitCode::SUCCESS;
    };
    let Some(instance) = roots.into_iter().next() else {
        eprintln!("error: image has no root instance to send to");
        return ExitCode::FAILURE;
    };
    let platform = interp.platform();
    let args = match entry_args(selector, platform) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    report_entry_send(interp.send(
        instance,
        ns_runtime::Symbol::intern(selector),
        args,
        &bindings,
        None,
    ))
}

/// One engine's timing summary. Enough for the reported median to be
/// interpretable on its own -- the iteration count it was computed from,
/// plus the spread around it -- rather than a bare number nobody can
/// sanity-check.
struct BenchStats {
    iterations: u32,
    median: std::time::Duration,
    min: std::time::Duration,
    max: std::time::Duration,
}

/// The word each engine's report line leads with -- the same spelling
/// `--engine` itself uses, so a result can be grepped by the same string
/// that selects it.
fn engine_label(engine: EngineArg) -> &'static str {
    match engine {
        EngineArg::Bytecode => "bytecode",
        EngineArg::TreeWalk => "tree-walk",
    }
}

/// Renders an `instantiate`/`send` failure for `bench`'s purposes. Neither
/// call site treats it as fatal to the whole process the way `run` does --
/// `cmd_bench` folds it into that engine's `Result` instead -- so this
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
    class: &ns_runtime::ClassRef,
    factory_args: &[ns_runtime::Value],
    selector: ns_runtime::Symbol,
    entry_args: &[ns_runtime::Value],
    bindings: &Rc<std::collections::HashMap<ns_syntax::Span, ns_runtime::Binding>>,
) -> Result<std::time::Duration, String> {
    let instance = interp
        .instantiate(class.clone(), factory_args.to_vec(), bindings)
        .map_err(signal_message)?;
    let start = std::time::Instant::now();
    let outcome = interp.send(instance, selector, entry_args.to_vec(), bindings, None);
    let elapsed = start.elapsed();
    outcome.map(|_| elapsed).map_err(signal_message)
}

/// Bootstraps `engine`, discards one warm-up `timed_send`, times
/// `iterations` further ones, and reduces the samples to `BenchStats`.
/// Bootstrapping, module loading, and (for the bytecode engine) compiling
/// all happen here but before the loop starts -- once-per-process costs a
/// real program pays at startup, excluded from the per-send timing for the
/// same reason instantiation is.
fn bench_engine(
    loaded: &LoadedSource,
    bindings: &Rc<std::collections::HashMap<ns_syntax::Span, ns_runtime::Binding>>,
    send: &str,
    iterations: u32,
    engine: EngineArg,
) -> Result<BenchStats, String> {
    let (mut interp, class) = bootstrap_with_engine(loaded, engine);
    let platform = interp.platform();
    let factory_args = factory_args_for(&class, platform.clone())?;
    let entry_args_list = entry_args(send, platform)?;
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
        bindings,
    )?;

    let mut timings = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        timings.push(timed_send(
            &mut interp,
            &class,
            &factory_args,
            selector,
            &entry_args_list,
            bindings,
        )?);
    }
    timings.sort();
    let min = timings[0];
    let max = timings[timings.len() - 1];
    // Median of N, not mean (the brief's own wording): a mean lets one
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
/// timed and why, and `bench_engine`/`timed_send` for how. Runs the
/// bytecode engine first, then the tree-walker, so the two report lines
/// come out in the same order `--engine`'s own default does.
fn cmd_bench(file: &Path, send: &str, iterations: u32) -> ExitCode {
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: this is a debug build; the timings below measure debug-build overhead, \
             not the engines. Rebuild and re-run with `--release`."
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
    let bindings = loaded.resolved.bindings.clone();
    println!(
        "timing: the entry send `{send}` only, after instantiation -- parsing, resolving, \
         bootstrapping, and compiling all happened once beforehand and are excluded. \
         {iterations} timed run(s) per engine, after one discarded warm-up run."
    );
    for engine in [EngineArg::Bytecode, EngineArg::TreeWalk] {
        match bench_engine(&loaded, &bindings, send, iterations, engine) {
            Ok(stats) => println!(
                "{}: median {:?} (min {:?}, max {:?}, n={})",
                engine_label(engine),
                stats.median,
                stats.min,
                stats.max,
                stats.iterations
            ),
            Err(msg) => {
                eprintln!("error: {}: {msg}", engine_label(engine));
                return ExitCode::FAILURE;
            }
        }
    }
    // The single caveat that matters most for reading these two numbers
    // together: the baseline compiler inlines `whileTrue:`/`whileFalse:`/
    // `ifTrue:...`/`and:`/`or:` into straight-line jumps when every block
    // operand is a literal, parameter-less, temp-less block (M2 spec,
    // "Control-flow inlining"; `compiler::control_flow::recognize`). The
    // tree-walker has no such inlining and pays a real block invocation for
    // each one. A loop written in that shape -- as any straightforward
    // `whileTrue:` loop is -- therefore makes the bytecode engine skip work
    // the tree-walker still does, for a reason that has nothing to do with
    // send dispatch. Printed here, not only in a report, because this
    // command and its output will still be read long after any one report
    // is gone.
    println!(
        "note: control-flow selectors such as `whileTrue:` are inlined by the baseline \
         compiler, so the cross-engine ratio above includes loop-control overhead the \
         bytecode engine does not pay -- it is not a pure dispatch comparison. The bytecode \
         median alone is the dispatch baseline: today every send is a full lookup, the \
         inline-cache slot is reserved but not yet wired up."
    );
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

/// `ns-next disasm`: for a `.ns` file, bootstrap and compile the *whole*
/// mixin tree (`compiler::compile_module`, exactly as the bytecode engine
/// would); for a serialized artifact, decode it directly and reuse its own
/// code table instead of recompiling anything. Either way, the result is a
/// `(class, module)` pair from which a `--class` path can address any
/// mixin's code, then look up exactly one method or the target mixin's
/// initializer and print its bytecode listing. The source path never
/// instantiates and never installs the bytecode engine -- compile only; the
/// artifact path never instantiates either, since `image::load` only
/// decodes the world.
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

    // Held only so the borrows below outlive the branch: the source path owns
    // its interpreter, the artifact path owns the loaded world.
    let (class, module) = if looks_like_an_artifact(&raw) {
        match ns_runtime::image::load(&raw) {
            Ok(loaded) => {
                let module = loaded
                    .interp
                    .compiled_module()
                    .expect("load installed the bytecode engine")
                    .clone();
                (loaded.root_class.clone(), module)
            }
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
        let bindings = loaded.resolved.bindings.clone();
        let (_interp, class) = bootstrap_with_engine(&loaded, EngineArg::TreeWalk);
        let module = ns_runtime::compiler::compile_module(&class, &bindings);
        (class, module)
    };
    let root_mixin = match &class.borrow().data {
        ns_runtime::ObjectData::Class(data) => data.application.mixin.clone(),
        _ => {
            eprintln!("error: internal: loaded module did not produce a class object");
            return ExitCode::FAILURE;
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
            let code = module
                .initializers
                .get(&ns_runtime::mixin::mixin_key(&mixin))
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "internal: compile_module has no initializer for `{}` (coverage gap)",
                        mixin.name
                    )
                });
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
            let code = module
                .methods
                .get(&ns_runtime::bytecode::method_key(&method))
                .cloned()
                .unwrap_or_else(|| {
                    panic!("internal: compile_module has no code for `{selector}` (coverage gap)")
                });
            (code, selector.to_string())
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
fn cmd_inspect(artifact: &Path) -> ExitCode {
    let bytes = match std::fs::read(artifact) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading {}: {e}", artifact.display());
            return ExitCode::FAILURE;
        }
    };
    match ns_runtime::image::inspect::inspect(&bytes) {
        Ok(report) => {
            print!("{}", ns_runtime::image::inspect::render(&report));
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
/// and `mixin.nested`. Accepting anything wider -- a slot's implicit setter,
/// a builtin's native method -- would accept a selector the code and mixin
/// tables never interned, and `build` would have to grow the symbol table to
/// record it, breaking the byte-identity guarantee with `snapshot` that this
/// check exists to preserve (`docs/design/image-format.md`, "The consistency
/// guarantee, stated precisely").
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
fn root_class_answers(class: &ns_runtime::ClassRef, selector: &str) -> bool {
    let sym = ns_runtime::Symbol::intern(selector);
    let mut app = match &class.borrow().data {
        ns_runtime::ObjectData::Class(data) => Some(data.application.clone()),
        _ => None,
    };
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
    let (mut interp, class) = bootstrap_with_engine(&loaded, EngineArg::Bytecode);

    let entry = match entry {
        Some(e) => e.to_string(),
        None if root_class_answers(&class, "main:args:") => "main:args:".to_string(),
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
    if !root_class_answers(&class, &entry) {
        eprintln!(
            "error: the root class of {} does not answer `{entry}`",
            file.display()
        );
        return ExitCode::FAILURE;
    }

    let module = interp
        .compiled_module()
        .expect("bootstrap_with_engine installed the bytecode engine");
    let bytes = match ns_runtime::image::build(
        debug_source.then_some(loaded.source.as_str()),
        &loaded.origin,
        &class,
        module,
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

/// What one `ns-next run` of a whitelisted module produced.
struct EngineOutcome {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Runs one module through a *child* `ns-next run`, once per engine.
///
/// A child process rather than an in-process call on purpose: the thing
/// being compared is the module's own stdout, which Newspeak code writes
/// straight to the process's stdout through `platform transcript`. Running
/// both engines in this process would interleave their output with the
/// gate's own, leaving nothing to compare byte for byte -- which is the
/// whole point of running each entry twice.
fn run_one(exe: &Path, file: &Path, send: &str, engine: &str) -> Result<EngineOutcome, String> {
    let out = std::process::Command::new(exe)
        .arg("run")
        .arg(file)
        .arg("--send")
        .arg(send)
        .arg("--engine")
        .arg(engine)
        .output()
        .map_err(|e| format!("could not run {}: {e}", exe.display()))?;
    Ok(EngineOutcome {
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
/// Every entry is run under both engines and the two are compared on exit
/// status, stdout and stderr before either is compared to the expectation.
///
/// What that is worth, stated honestly: with `expected_stdout` empty on
/// every real row -- no corpus module that currently runs writes to the
/// transcript at all -- this is a tripwire that a whitelisted module still
/// instantiates and still answers one message without error, plus a check
/// that the two engines fail or succeed the same way. It is not yet a
/// differential harness over observable *output* from real modules; it
/// becomes one on the day a row can record something printed. Treat a
/// green `run-corpus` as "nothing regressed", not as "the corpus is
/// covered".
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

    // `ran` counts rows whose module both engines actually executed, so a
    // row naming a deleted file, or one whose child could not be spawned,
    // reports as a failure without inflating the number a reader uses to
    // judge coverage.
    let (mut ran, mut failed) = (0, 0);
    for entry in &entries {
        let file = dir.join(&entry.file);
        if !file.is_file() {
            failed += 1;
            eprintln!("FAIL {}: no such file under {}", entry.file, dir.display());
            continue;
        }
        let bytecode = match run_one(&exe, &file, &entry.send, "bytecode") {
            Ok(o) => o,
            Err(e) => {
                failed += 1;
                eprintln!("FAIL {}: {e}", entry.file);
                continue;
            }
        };
        let tree_walk = match run_one(&exe, &file, &entry.send, "tree-walk") {
            Ok(o) => o,
            Err(e) => {
                failed += 1;
                eprintln!("FAIL {}: {e}", entry.file);
                continue;
            }
        };
        ran += 1;
        if let Some(diff) = engines_disagree(&bytecode, &tree_walk) {
            failed += 1;
            eprintln!("FAIL {}: engines disagree on {diff}", entry.file);
            continue;
        }
        if bytecode.status != Some(entry.expected_status) {
            failed += 1;
            eprintln!(
                "FAIL {}: expected exit {}, got {} (stderr: {})",
                entry.file,
                entry.expected_status,
                match bytecode.status {
                    Some(c) => c.to_string(),
                    None => "signal".to_string(),
                },
                bytecode.stderr.trim_end()
            );
            continue;
        }
        if bytecode.stdout != entry.expected_stdout {
            failed += 1;
            eprintln!(
                "FAIL {}: expected stdout {:?}, got {:?}",
                entry.file, entry.expected_stdout, bytecode.stdout
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

/// Names the first stream the two engines differ on, or `None` if they
/// agree byte for byte on all three.
fn engines_disagree(bytecode: &EngineOutcome, tree_walk: &EngineOutcome) -> Option<String> {
    if bytecode.status != tree_walk.status {
        return Some(format!(
            "exit status: bytecode {:?}, tree-walk {:?}",
            bytecode.status, tree_walk.status
        ));
    }
    if bytecode.stdout != tree_walk.stdout {
        return Some(format!(
            "stdout: bytecode {:?}, tree-walk {:?}",
            bytecode.stdout, tree_walk.stdout
        ));
    }
    if bytecode.stderr != tree_walk.stderr {
        return Some(format!(
            "stderr: bytecode {:?}, tree-walk {:?}",
            bytecode.stderr, tree_walk.stderr
        ));
    }
    None
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
