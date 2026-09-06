#![warn(clippy::all)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
        /// A selector to send (with `[platform, nil]`, as `run` does)
        /// before snapshotting; its result becomes a second root.
        #[arg(long)]
        send: Option<String>,
    },
    /// Deserialize an image and optionally send one further message to
    /// its root instance (`[platform, nil]`, as `run` does).
    Load {
        image: PathBuf,
        #[arg(long)]
        send: Option<String>,
    },
    /// Compile one method (or, with --initializer, the root class's
    /// instance initializer) and print its bytecode. Compile only: nothing
    /// is instantiated or run.
    Disasm {
        file: PathBuf,
        /// The method selector, e.g. `main:args:`.
        selector: Option<String>,
        /// Disassemble the root class's compiled instance initializer instead.
        #[arg(long)]
        initializer: bool,
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
        Cmd::Run { file, send, engine } => cmd_run(&file, &send, engine),
        Cmd::Snapshot { file, out, send } => cmd_snapshot(&file, &out, send.as_deref()),
        Cmd::Load { image, send } => cmd_load(&image, send.as_deref()),
        Cmd::Disasm {
            file,
            selector,
            initializer,
        } => cmd_disasm(&file, selector.as_deref(), initializer),
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
        interp.set_engine(ns_runtime::Engine::Bytecode(compiled));
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

/// `ns-next run`: parse, resolve, bootstrap, load, install the engine,
/// instantiate the root class with `platform`, send the entry selector
/// with `[platform, nil]`, report.
fn cmd_run(file: &Path, send: &str, engine: EngineArg) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    let (mut interp, class) = bootstrap_with_engine(&loaded, engine);
    let platform = interp.platform();
    // `usingPlatform: platform`'s one factory parameter (`newspeak-spec.pdf`
    // §8.4's Core Library factory convention).
    let instance = match interp.instantiate(class, vec![platform.clone()], &bindings) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    // `main:args:`'s two parameters: `Nil` for the environment-args list --
    // no command-line-argument passthrough into Newspeak yet.
    let selector = ns_runtime::Symbol::intern(send);
    report_entry_send(interp.send(
        instance,
        selector,
        vec![platform, ns_runtime::Value::Nil],
        &bindings,
        None,
    ))
}

/// `ns-next snapshot`: `run`'s pipeline up to (and optionally including)
/// the entry send, then a serialized image of the resulting world. Always
/// the bytecode engine -- a tree-walker closure carries `ClosureBody::Ast`,
/// which the format has no representation for, and the image's own code
/// table is what a later `load` runs.
fn cmd_snapshot(file: &Path, out: &Path, send: Option<&str>) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    let (mut interp, class) = bootstrap_with_engine(&loaded, EngineArg::Bytecode);
    let platform = interp.platform();
    let instance = match interp.instantiate(class.clone(), vec![platform.clone()], &bindings) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    let mut roots = vec![instance.clone()];
    if let Some(selector) = send {
        let args = vec![platform.clone(), ns_runtime::Value::Nil];
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
        &loaded.source,
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
/// message to its root instance with the same `[platform, nil]` convention
/// `run` uses -- and report it through the same `report_entry_send`, so a
/// `load --send main:args:` of a snapshot prints exactly what `run` prints
/// and nothing else. A malformed image is a message, never a stack trace.
fn cmd_load(image: &Path, send: Option<&str>) -> ExitCode {
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
        bindings,
        roots,
    } = match ns_runtime::image::load(&bytes) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: loading {}: {e}", image.display());
            return ExitCode::FAILURE;
        }
    };
    let Some(selector) = send else {
        println!("loaded {}: {} root value(s)", image.display(), roots.len());
        return ExitCode::SUCCESS;
    };
    let Some(instance) = roots.into_iter().next() else {
        eprintln!("error: image has no root instance to send to");
        return ExitCode::FAILURE;
    };
    let platform = interp.platform();
    let args = vec![platform, ns_runtime::Value::Nil];
    report_entry_send(interp.send(
        instance,
        ns_runtime::Symbol::intern(selector),
        args,
        &bindings,
        None,
    ))
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

/// `ns-next disasm`: bootstrap and load (so `compile_initializer` sees the
/// loaded class's `slot_index`), then compile exactly one method or the
/// initializer and print its bytecode listing. Never instantiates and
/// never installs the bytecode engine -- compile only.
fn cmd_disasm(file: &Path, selector: Option<&str>, initializer: bool) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    let (_interp, class) = bootstrap_with_engine(&loaded, EngineArg::TreeWalk);
    let (code, name) = match (selector, initializer) {
        (None, true) => {
            let class_name = match &class.borrow().data {
                ns_runtime::ObjectData::Class(data) => data.application.mixin.name.to_string(),
                _ => "?".to_string(),
            };
            (
                ns_runtime::compiler::compile_initializer(&class, &bindings),
                class_name,
            )
        }
        (Some(selector), false) => {
            let method = match &class.borrow().data {
                ns_runtime::ObjectData::Class(data) => data
                    .application
                    .mixin
                    .methods
                    .get(&ns_runtime::Symbol::intern(selector))
                    .cloned(),
                _ => None,
            };
            let Some(method) = method else {
                eprintln!("error: no method `{selector}` found on the root class");
                return ExitCode::FAILURE;
            };
            (
                ns_runtime::compiler::compile_method(&method, &bindings),
                selector.to_string(),
            )
        }
        _ => {
            eprintln!("error: disasm takes exactly one of <selector> or --initializer");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", ns_runtime::bytecode::disassemble(&code, &name));
    ExitCode::SUCCESS
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
