#![warn(clippy::all)]

//! `ns-next`, the binary that runs Newspeak; what it offers is `Cli`'s
//! description below, which `--help` shows.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod command_line;
mod commands;
mod entry;
mod program;
mod project;

/// Runs Newspeak: a program from source (`run`), a corpus against its
/// recorded outcomes (`run-corpus`), an image (`load`), a timed entry send
/// (`bench`), and an application written as an image (`build`). What looks
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
        /// A script of the project's `newspeak.json`: its arguments go to
        /// `main:args:` first, then any given after `--`.
        #[arg(long)]
        script: Option<String>,
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
        /// Print the source text a `--debug-source` image carries, instead
        /// of starting it. Fails on a source-free image,
        /// which by design has none.
        #[arg(long)]
        print_source: bool,
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
        /// Store every module's source text in the image (the debug variant).
        #[arg(long)]
        debug_source: bool,
        /// The lowering to compile the source with.
        #[arg(long, value_enum, default_value_t = LoweringArgument::Optimized)]
        lowering: LoweringArgument,
    },
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

fn main() -> ExitCode {
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
            script,
            arguments,
        } => commands::run::execute(
            &program,
            lowering.into(),
            cache_statistics,
            script.as_deref(),
            &arguments,
        ),
        Command::Load {
            image,
            print_source,
            arguments,
        } => commands::load::execute(&image, print_source, &arguments),
        Command::Bench {
            program,
            iterations,
            cache_statistics,
            collector_statistics,
            script,
            arguments,
        } => commands::bench::execute(
            &program,
            iterations,
            cache_statistics,
            collector_statistics,
            script.as_deref(),
            &arguments,
        ),
        Command::Build {
            program,
            out,
            debug_source,
            lowering,
        } => commands::build::execute(&program, &out, debug_source, lowering.into()),
    }
}
