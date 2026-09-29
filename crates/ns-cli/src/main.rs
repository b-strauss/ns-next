#![warn(clippy::all)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod commands;
mod entry;
mod program;
mod project;
mod source;

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
    /// Parse a single .ns file.
    Parse {
        file: PathBuf,
        #[arg(long)]
        ast: bool,
    },
    /// Parse every .ns file under a directory.
    ParseCorpus { directory: PathBuf },
    /// Run every whitelisted .ns file under a directory, from source and as
    /// a built binary, and check each against its recorded expected output.
    RunCorpus {
        directory: PathBuf,
        #[arg(long)]
        whitelist: Option<PathBuf>,
    },
    /// Run a program end to end: a `.ns` source file, whose root is
    /// instantiated with `new`, or a `.json` project file, whose root is
    /// instantiated with `packageUsing:` a manifest of its other modules.
    /// Either way the application is sent `main: platform args:`.
    Run {
        /// A `.ns` source file or a `.json` project file.
        file: PathBuf,
        /// The lowering to compile the source with.
        #[arg(long, value_enum, default_value_t = LoweringArgument::Optimized)]
        lowering: LoweringArgument,
        /// Print the inline caches' sizes to stderr after the run.
        #[arg(long)]
        cache_statistics: bool,
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
        /// A `.ns` file or a `.json` project file, assembled as `run` does.
        file: PathBuf,
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
        /// The program's arguments, after `--`: `main:args:` receives them
        /// as its second argument, an `Array` of `String`s (§8.4).
        #[arg(last = true)]
        arguments: Vec<String>,
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
        /// The lowering to compile a .ns source with; optimized when absent.
        /// Refused for an artifact, which carries the code it was built with.
        #[arg(long, value_enum)]
        lowering: Option<LoweringArgument>,
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
    /// Assemble the application exactly as `run` does -- its factory and
    /// every initializer run, with no platform in existence -- and write it
    /// just before `main:args:`, as an image whose one root is the
    /// application. A `.ns` source file or a `.json` project file.
    Build {
        file: PathBuf,
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
/// nesting ceiling bounds. The test suite runs the deepest nesting the parser
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
        Command::Parse { file, ast } => commands::parse::execute(&file, ast),
        Command::ParseCorpus { directory } => commands::parse_corpus::execute(&directory),
        Command::RunCorpus {
            directory,
            whitelist,
        } => commands::run_corpus::execute(&directory, whitelist.as_deref()),
        Command::Run {
            file,
            lowering,
            cache_statistics,
            arguments,
        } => commands::run::execute(&file, lowering.into(), cache_statistics, &arguments),
        Command::Load {
            image,
            print_source,
            arguments,
        } => commands::load::execute(&image, print_source, &arguments),
        Command::Bench {
            file,
            iterations,
            cache_statistics,
            collector_statistics,
            arguments,
        } => commands::bench::execute(
            &file,
            iterations,
            cache_statistics,
            collector_statistics,
            &arguments,
        ),
        Command::Disasm {
            file,
            selector,
            initializer,
            class,
            lowering,
        } => commands::disasm::execute(
            &file,
            selector.as_deref(),
            initializer,
            class.as_deref(),
            lowering.map(Into::into),
        ),
        Command::Inspect { artifact, full } => commands::inspect::execute(&artifact, full),
        Command::Build {
            file,
            out,
            debug_source,
            lowering,
        } => commands::build::execute(&file, &out, debug_source, lowering.into()),
    }
}
