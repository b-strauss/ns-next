#![warn(clippy::all)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod commands;
mod entry;
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
    ParseCorpus {
        directory: PathBuf,
        #[arg(long)]
        blacklist: Option<PathBuf>,
    },
    /// Run every whitelisted .ns file under a directory, from source and as
    /// a built binary, and check each against its recorded expected output.
    RunCorpus {
        directory: PathBuf,
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
        /// The lowering to compile the source with.
        #[arg(long, value_enum, default_value_t = LoweringArgument::Optimized)]
        lowering: LoweringArgument,
        /// Print the inline caches' sizes to stderr after the run.
        #[arg(long)]
        cache_statistics: bool,
    },
    /// Run a file, then serialize the resulting (quiesced) world to an
    /// image file.
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
        /// The lowering to compile the source with.
        #[arg(long, value_enum, default_value_t = LoweringArgument::Optimized)]
        lowering: LoweringArgument,
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
    /// median (with minimum and maximum) time for the entry send alone.
    ///
    /// Only the entry send is timed -- parsing, resolving, bootstrapping,
    /// and compiling all happen once beforehand and are excluded, because
    /// what the runtime's optimizations change is repeated dispatch, not
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
        /// Print the inline caches' sizes to stderr after the timed runs.
        #[arg(long)]
        cache_statistics: bool,
        /// Print the collector's counts and pauses to stderr after the
        /// timed runs.
        #[arg(long)]
        collector_statistics: bool,
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
        Command::ParseCorpus {
            directory,
            blacklist,
        } => commands::parse_corpus::execute(&directory, blacklist.as_deref()),
        Command::RunCorpus {
            directory,
            whitelist,
        } => commands::run_corpus::execute(&directory, whitelist.as_deref()),
        Command::Run {
            file,
            send,
            lowering,
            cache_statistics,
        } => commands::run::execute(&file, &send, lowering.into(), cache_statistics),
        Command::Snapshot {
            file,
            out,
            send,
            debug_source,
            lowering,
        } => {
            commands::snapshot::execute(&file, &out, send.as_deref(), debug_source, lowering.into())
        }
        Command::Load {
            image,
            send,
            print_source,
        } => commands::load::execute(&image, send.as_deref(), print_source),
        Command::Bench {
            file,
            send,
            iterations,
            cache_statistics,
            collector_statistics,
        } => commands::bench::execute(
            &file,
            &send,
            iterations,
            cache_statistics,
            collector_statistics,
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
            entry,
            debug_source,
            lowering,
        } => commands::build::execute(&file, &out, entry.as_deref(), debug_source, lowering.into()),
    }
}
