//! `ns-next-analyzer`: looks at Newspeak without running it -- `lsp`,
//! `dump-tokens`, `parse`, `parse-corpus`, `disasm` and `inspect`. Nothing
//! here starts the VM.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod commands;

/// Looks at Newspeak without running it: the language server (`lsp`), the
/// token dump (`dump-tokens`), parsing (`parse`, `parse-corpus`), a compiled
/// body's bytecode (`disasm`) and an image's structure (`inspect`). Running
/// a program is `ns-next`'s.
#[derive(Parser)]
#[command(name = "ns-next-analyzer", version)]
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
    /// Speak the Language Server Protocol on stdin and stdout.
    Lsp,
    /// Print each file's tokens as JSON, one document per file.
    DumpTokens { files: Vec<String> },
    /// Parse a single .ns file.
    Parse {
        file: PathBuf,
        #[arg(long)]
        ast: bool,
    },
    /// Parse every .ns file under a directory.
    ParseCorpus { directory: PathBuf },
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
}

/// The stack every command runs on. Each parses and most resolve, which
/// recurse; the parser's nesting ceiling bounds the depth -- a chain of
/// messages, which nests as deep as the source is long, is dropped and walked
/// link by link (`ns_syntax::ast::Send::receiver_chain`) by the server, and
/// walked by recursion by the compiler `disasm` runs -- so this is the 8 MiB
/// `ns-next` runs the same front end and compiler on
/// (`crates/ns-cli/src/main.rs`, `INTERPRETER_STACK_BYTES`). The thread is
/// needed regardless: the symbol interner is thread-local, so everything
/// that interns must run on one.
const ANALYZER_STACK_BYTES: usize = 8 * 1024 * 1024;

fn main() -> ExitCode {
    let command = Cli::parse().command;
    let worker = std::thread::Builder::new()
        .name("ns-next-analyzer".to_string())
        .stack_size(ANALYZER_STACK_BYTES)
        .spawn(move || execute(command))
        .expect("spawn the analyzer thread");
    worker.join().unwrap_or_else(|_| ExitCode::from(101))
}

fn execute(command: Command) -> ExitCode {
    match command {
        Command::Lsp => commands::lsp::execute(),
        Command::DumpTokens { files } => commands::dump_tokens::execute(&files),
        Command::Parse { file, ast } => commands::parse::execute(&file, ast),
        Command::ParseCorpus { directory } => commands::parse_corpus::execute(&directory),
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
    }
}
