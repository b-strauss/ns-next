#![warn(clippy::all)]

//! `ns-next`, the binary that runs Newspeak; what it offers is `Cli`'s
//! description below, which `--help` shows.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Parser, Subcommand};

mod command_line;
mod commands;
mod debug;
mod entry;
mod listing;
mod mcp;
mod program;
mod project;
mod target_runner;
mod trace;
mod trace_file;

/// Runs Newspeak: a program from source (`run`), a corpus against its
/// recorded outcomes (`run-corpus`), an image (`load`), a timed entry send
/// (`bench`), an application written as an image (`build`), and the core
/// library's sources the binary embeds (`shipped-sources`), and a Model
/// Context Protocol server through which an agent debugs a program
/// (`mcp`). What looks
/// at Newspeak without running it -- parsing, disassembly, image inspection,
/// the language server -- is `ns-next-analyzer`.
#[derive(Parser)]
#[command(name = "ns-next", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Which lowering compiles a source (`docs/design/compiler.md`, "The
/// reference lowering"): `optimized` is what every program runs under,
/// `reference` the naive lowering it is checked against.
#[derive(Clone, Copy, clap::ValueEnum)]
enum LoweringArgument {
    Optimized,
    Reference,
}

impl From<LoweringArgument> for ns_runtime::Lowering {
    fn from(argument: LoweringArgument) -> Self {
        match argument {
            LoweringArgument::Optimized => ns_runtime::Lowering::Optimized,
            LoweringArgument::Reference => ns_runtime::Lowering::Reference,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Run every whitelisted .ns file under a directory, from source and as
    /// a built binary, and check each against its recorded expected output.
    RunCorpus {
        directory: PathBuf,
        #[arg(long)]
        whitelist: Option<PathBuf>,
    },
    /// Run a program end to end: a `.ns` source file, whose root is
    /// instantiated with `new`, or a project -- a directory holding a
    /// `newspeak.json`, or that file itself -- whose root is instantiated
    /// with `packageUsing:` a manifest of its other modules. Either way the
    /// application is sent `main: platform args:`.
    #[command(group(
        ArgGroup::new("printing")
            .multiple(true)
            .args(["print_bytecode", "print_code", "print_inlining"])
    ))]
    #[command(group(
        ArgGroup::new("traced")
            .multiple(true)
            .args(["trace", "print_bytecode", "print_code", "print_inlining"])
    ))]
    Run {
        /// A `.ns` source file, a project directory or its `newspeak.json`;
        /// the current directory when omitted.
        #[arg(default_value = ".")]
        program: PathBuf,
        /// The lowering to compile the source with.
        #[arg(long, value_enum, default_value_t = LoweringArgument::Optimized)]
        lowering: LoweringArgument,
        /// Print the inline caches' sizes to stderr after the run.
        #[arg(long)]
        cache_statistics: bool,
        /// Print the collector's counts and pauses to stderr after the run.
        #[arg(long)]
        collector_statistics: bool,
        /// Print the JIT's counts and the baseline compiler's time to stderr
        /// after the run. It makes machine code count what it otherwise
        /// leaves uncounted, which changes the code: time without it.
        #[arg(long)]
        jit_statistics: bool,
        /// Write a line on stderr for every event of the categories
        /// named, separated by commas: `compile`, `osr`, `optimize`,
        /// `inlining`, `deoptimize`, `code-zone`, `caches`, or `all`.
        #[arg(long, value_name = "CATEGORIES")]
        trace: Option<String>,
        /// Trace only the bodies a pattern selects, one pattern each time
        /// the option is given (`,` is a selector, so nothing splits a
        /// pattern):
        /// `<id>[@<version>][/<Class>(.<Class>)*][ class]>><selector>`, a
        /// trailing `...` making a prefix (`*` is a selector's character).
        #[arg(long, value_name = "PATTERN", requires = "trace")]
        trace_filter: Vec<String>,
        /// Print on stderr each body's bytecode when its module is made,
        /// between `--- bytecode {<body>}` and `--- end`.
        #[arg(long)]
        print_bytecode: bool,
        /// Print on stderr each body's machine code when it is installed,
        /// baseline and optimized, between `--- code {<body>} …` and
        /// `--- end`.
        #[arg(long)]
        print_code: bool,
        /// Print on stderr the inlining plan of every optimizing compile,
        /// with the copy installed or the reason it was refused, between
        /// `--- inlining {<body>} …` and `--- end`.
        #[arg(long)]
        print_inlining: bool,
        /// Print only the bodies a pattern selects, one pattern each time
        /// the option is given, in `--trace-filter`'s syntax.
        #[arg(long, value_name = "PATTERN", requires = "printing")]
        print_filter: Vec<String>,
        /// Write the trace and the listings into the file at PATH, one
        /// JSON object a line, instead of on stderr: a body described
        /// once with its source positions, then named by its number.
        #[arg(long, value_name = "PATH", requires = "traced")]
        trace_file: Option<PathBuf>,
        /// A script of the project's `newspeak.json`: its arguments go to
        /// `main:args:` first, then any given after `--`.
        #[arg(long)]
        script: Option<String>,
        /// Listen for a debugger: on 127.0.0.1 at a port the system
        /// chooses, printed with a token on stderr before anything runs; a
        /// client speaks the Debug Adapter Protocol there and presents the
        /// token in `attach`.
        #[arg(long, conflicts_with = "debug_connect")]
        debug: bool,
        /// Connect to a debugger listening at ADDRESS (`host:port`) and
        /// speak the Debug Adapter Protocol to it.
        #[arg(long, value_name = "ADDRESS")]
        debug_connect: Option<String>,
        /// The program's arguments, after `--`: `main:args:` receives them
        /// as its second argument, an `Array` of `String`s (§8.4).
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Decode an image `build` wrote and send its application `main:
    /// platform args:`, with this process's platform: the first platform
    /// the application ever sees.
    Load {
        image: PathBuf,
        /// Print the source text the image carries, instead of starting it.
        /// Fails on an image written with `--strip-source`, which has none.
        #[arg(long)]
        print_source: bool,
        /// Listen for a debugger: on 127.0.0.1 at a port the system
        /// chooses, printed with a token on stderr before anything runs; a
        /// client speaks the Debug Adapter Protocol there and presents the
        /// token in `attach`.
        #[arg(long, conflicts_with = "debug_connect")]
        debug: bool,
        /// Connect to a debugger listening at ADDRESS (`host:port`) and
        /// speak the Debug Adapter Protocol to it.
        #[arg(long, value_name = "ADDRESS")]
        debug_connect: Option<String>,
        /// The program's arguments, after `--`: `main:args:` receives them
        /// as its second argument, an `Array` of `String`s (§8.4).
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Time `main: platform args:`: load and compile once, then discard a
    /// warm-up run and time `--iterations` further runs, reporting the
    /// median (with minimum and maximum) time for the entry send alone. Each
    /// sample assembles a fresh application the way `run` does, untimed.
    ///
    /// Only the entry send is timed -- parsing, resolving, bootstrapping,
    /// and compiling all happen once beforehand and are excluded, because
    /// what the runtime's optimizations change is repeated dispatch, not
    /// one-time startup. ALWAYS measure with `--release`: a debug build's
    /// numbers are not comparable to anything and this subcommand warns on
    /// stderr when it detects one.
    Bench {
        /// A `.ns` file, a project directory or its `newspeak.json`,
        /// assembled as `run` does; the current directory when omitted.
        #[arg(default_value = ".")]
        program: PathBuf,
        /// How many timed runs to take the median of, after one discarded
        /// warm-up run.
        #[arg(long, default_value_t = 10)]
        iterations: u32,
        /// Print the inline caches' sizes to stderr after the timed runs.
        #[arg(long)]
        cache_statistics: bool,
        /// Print the collector's counts and pauses to stderr after the
        /// timed runs.
        #[arg(long)]
        collector_statistics: bool,
        /// Print the JIT's counts and the baseline compiler's time to stderr
        /// after the run. It makes machine code count what it otherwise
        /// leaves uncounted, which changes the code: time without it.
        #[arg(long)]
        jit_statistics: bool,
        /// A script of the project's `newspeak.json`: its arguments go to
        /// `main:args:` first, then any given after `--`.
        #[arg(long)]
        script: Option<String>,
        /// The program's arguments, after `--`: `main:args:` receives them
        /// as its second argument, an `Array` of `String`s (§8.4).
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Assemble the application exactly as `run` does -- its factory and
    /// every initializer run, with no platform in existence -- and write it
    /// just before `main:args:`, as an image whose one root is the
    /// application. A `.ns` source file, a project directory or its
    /// `newspeak.json`.
    Build {
        program: PathBuf,
        out: PathBuf,
        /// Leave every module's source text out of the image. An image
        /// without it runs as well, but a program loaded from it cannot be
        /// edited and `load --print-source` has nothing to print.
        #[arg(long)]
        strip_source: bool,
        /// The lowering to compile the source with.
        #[arg(long, value_enum, default_value_t = LoweringArgument::Optimized)]
        lowering: LoweringArgument,
    },
    /// Write the `.ns` sources this binary embeds -- the kernel, the
    /// manifest and the factories -- into DIRECTORY in the layout of
    /// `core-library/`, with a `fingerprint` file; or print their
    /// fingerprint. The directory is created, and refused when it holds
    /// anything that is not one of these files.
    #[command(group(
        clap::ArgGroup::new("form")
            .required(true)
            .args(["directory", "fingerprint"])
    ))]
    ShippedSources {
        directory: Option<PathBuf>,
        /// Print the fingerprint of the embedded sources, one line, instead
        /// of writing them. It changes with any embedded `.ns` text.
        #[arg(long)]
        fingerprint: bool,
    },
    /// Serve the Model Context Protocol on standard input and output: an
    /// agent such as Claude debugs a program through its tools, each
    /// program run in a child under the debugger, with this executable
    /// unless the tool's `runtime` names another.
    Mcp,
}

/// The native stack the interpreter runs on. Nothing re-enters the run loop
/// from Rust (`docs/design/execution.md`, "No re-entry from Rust"), so what
/// recurses on it is the front end and the compiler, whose depth the parser's
/// nesting ceiling bounds -- except along a chain of messages, which nests as
/// deep as the source is long and which the compiler still walks by
/// recursion (`docs/OPEN-WORK.md`). The test suite runs the deepest nesting the parser
/// accepts through every stage on libtest's default 2 MiB threads in a debug
/// build; this is four times that. Reserved, not committed: only what a
/// program uses is backed by memory. The thread itself is needed regardless,
/// because the symbol interner is thread-local.
const INTERPRETER_STACK_BYTES: usize = 8 * 1024 * 1024;

/// The allocator for everything Rust allocates; the Newspeak heap is
/// `ns-memory`'s own reservation and never comes through it. mimalloc rather
/// than the platform's: the workspace manifest gives the measurement.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    // First, before anything else runs: the workspace's crates are built for
    // x86-64-v2, the precompiled standard library is not, so nothing before
    // this check executes a v2 instruction.
    if let Some(refusal) = ns_tooling::processor::refusal_for_missing_features("ns-next") {
        eprintln!("{refusal}");
        return ExitCode::from(1);
    }
    let command = Cli::parse().command;
    // The whole command runs on the interpreter's thread, not only its sends:
    // the symbol interner is thread-local, so every symbol a command interns
    // has to be interned where it is used.
    let worker = std::thread::Builder::new()
        .name("ns-next".to_string())
        .stack_size(INTERPRETER_STACK_BYTES)
        .spawn(move || execute(command))
        .expect("spawn the interpreter thread");
    // A panic on that thread has already been reported by the panic hook;
    // exit with the status a panic on the main thread would have had.
    worker.join().unwrap_or_else(|_| ExitCode::from(101))
}

fn execute(command: Command) -> ExitCode {
    match command {
        Command::RunCorpus {
            directory,
            whitelist,
        } => commands::run_corpus::execute(&directory, whitelist.as_deref()),
        Command::Run {
            program,
            lowering,
            cache_statistics,
            collector_statistics,
            jit_statistics,
            trace,
            trace_filter,
            print_bytecode,
            print_code,
            print_inlining,
            print_filter,
            trace_file,
            script,
            debug,
            debug_connect,
            arguments,
        } => commands::run::execute(
            &program,
            lowering.into(),
            cache_statistics,
            collector_statistics,
            jit_statistics,
            commands::run::TraceOptions {
                categories: trace.as_deref(),
                filter: trace_filter.iter().map(String::as_str).collect(),
                printing: ns_runtime::Printing {
                    bytecode: print_bytecode,
                    code: print_code,
                    inlining: print_inlining,
                },
                print_filter: print_filter.iter().map(String::as_str).collect(),
                file: trace_file.as_deref(),
            },
            script.as_deref(),
            debug::DebugOptions::from_arguments(debug, debug_connect),
            &arguments,
        ),
        Command::Load {
            image,
            print_source,
            debug,
            debug_connect,
            arguments,
        } => commands::load::execute(
            &image,
            print_source,
            debug::DebugOptions::from_arguments(debug, debug_connect),
            &arguments,
        ),
        Command::Bench {
            program,
            iterations,
            cache_statistics,
            collector_statistics,
            jit_statistics,
            script,
            arguments,
        } => commands::bench::execute(
            &program,
            iterations,
            cache_statistics,
            collector_statistics,
            jit_statistics,
            script.as_deref(),
            &arguments,
        ),
        Command::Build {
            program,
            out,
            strip_source,
            lowering,
        } => commands::build::execute(&program, &out, !strip_source, lowering.into()),
        Command::ShippedSources {
            directory,
            fingerprint,
        } => commands::shipped_sources::execute(directory.as_deref(), fingerprint),
        Command::Mcp => mcp::execute(),
    }
}
