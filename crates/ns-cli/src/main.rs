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
        /// Store the module's source text in the image (the debug
        /// variant). Off by default: a loaded world runs from the image's
        /// code table and mixin table, so the source is never needed to
        /// run it -- only to read it back as `.ns`.
        #[arg(long)]
        debug_source: bool,
    },
    /// Deserialize an image and optionally send one further message to
    /// its root instance (`[platform, nil]`, as `run` does).
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
        Cmd::Disasm {
            file,
            selector,
            initializer,
            class,
        } => cmd_disasm(&file, selector.as_deref(), initializer, class.as_deref()),
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

/// `ns-next run`: parse, resolve, bootstrap, load, install the engine,
/// instantiate the root class with whatever its declared primary factory
/// takes, send the entry selector with `[platform, nil]`, report.
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
/// message to its root instance with the same `[platform, nil]` convention
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

/// `ns-next disasm`: bootstrap and load, compile the *whole* mixin tree
/// (`compiler::compile_module`, exactly as the bytecode engine would) so a
/// `--class` path can address any mixin's code, then look up exactly one
/// method or the target mixin's initializer and print its bytecode
/// listing. Never instantiates and never installs the bytecode engine --
/// compile only.
fn cmd_disasm(
    file: &Path,
    selector: Option<&str>,
    initializer: bool,
    class_path: Option<&str>,
) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    let (_interp, class) = bootstrap_with_engine(&loaded, EngineArg::TreeWalk);
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
    let module = ns_runtime::compiler::compile_module(&class, &bindings);
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
