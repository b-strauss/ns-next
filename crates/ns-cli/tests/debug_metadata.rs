//! The debugger's oracle over every golden (`docs/design/debugger.md`,
//! decision 14): every body of every golden's modules, compiled from its
//! source under each lowering, compiles again alike and its metadata holds
//! its invariants (`Interpreter::check_debug_metadata`). One trial per
//! golden and lowering (`libtest-mimic`, `harness = false`), as in
//! `run_goldens.rs`, in process: nothing runs, so no `ns-next` is started.
//! The shipped modules' bodies are checked once, by `ns-runtime`'s
//! `every_shipped_body_compiles_again_alike`, not with every golden; the
//! same check through `build` and `load` is `golden_round_trips.rs`'s.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use libtest_mimic::{Arguments, Failed, Trial};
use ns_runtime::{Interpreter, Lowering};

mod common;

const LOWERINGS: [(&str, Lowering); 2] = [
    ("optimized", Lowering::Optimized),
    ("reference", Lowering::Reference),
];

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    for program in common::golden_programs() {
        let name = common::golden_name(&program);
        for (lowering_name, lowering) in LOWERINGS {
            let path = program.clone();
            trials.push(Trial::test(
                format!("debug_metadata::{lowering_name}::{name}"),
                move || every_body_compiles_again_alike(&path, lowering),
            ));
        }
    }
    trials.push(Trial::test(
        "debug_metadata::floor",
        most_goldens_are_checked,
    ));
    libtest_mimic::run(&arguments, trials).exit();
}

/// The fewest goldens, and project goldens, whose modules resolve and so
/// are checked: a trial of a golden that does not passes without checking
/// anything, so a drift in how a golden's modules are read -- the project
/// file's format, a path -- would otherwise stop the oracle silently.
/// Every project golden resolves today, and all but a few of the others
/// (533 of 545 programs, 4 projects among them); the floor leaves room
/// for goldens of failures that do not resolve.
const CHECKED_FLOOR: usize = 500;
const CHECKED_PROJECT_FLOOR: usize = 4;

/// At least `CHECKED_FLOOR` goldens, `CHECKED_PROJECT_FLOOR` of them
/// projects, have modules that resolve, which their trials check.
fn most_goldens_are_checked() -> Result<(), Failed> {
    let mut checked = 0;
    let mut projects = 0;
    for program in common::golden_programs() {
        if resolved_modules(&program)?.is_some() {
            checked += 1;
            if program.is_dir() {
                projects += 1;
            }
        }
    }
    if checked < CHECKED_FLOOR || projects < CHECKED_PROJECT_FLOOR {
        return Err(format!(
            "{checked} goldens, {projects} of them projects, resolve and are checked; at least {CHECKED_FLOOR}, {CHECKED_PROJECT_FLOOR} of them projects, were expected"
        )
        .into());
    }
    Ok(())
}

/// Every body of `program`'s modules, loaded as `run` loads them under
/// `lowering` but not run, compiles again alike with tables that hold
/// their invariants. A golden whose source does not parse or resolve has
/// nothing to compile, and passes (`most_goldens_are_checked` holds how
/// many do).
fn every_body_compiles_again_alike(program: &Path, lowering: Lowering) -> Result<(), Failed> {
    let Some(resolved_modules) = resolved_modules(program)? else {
        return Ok(());
    };
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    for resolved in &resolved_modules {
        interpreter.load_module(resolved);
    }
    let count = interpreter.check_debug_metadata(false)?;
    if count == 0 {
        return Err("no body of the program was checked".into());
    }
    Ok(())
}

/// `program`'s modules resolved, as `run` resolves them; `None` when one
/// does not parse or resolve, or the project file would be refused.
fn resolved_modules(program: &Path) -> Result<Option<Vec<ns_runtime::ResolvedModule>>, Failed> {
    let Some(sources) = module_sources(program)? else {
        return Ok(None);
    };
    let mut resolved_modules = Vec::with_capacity(sources.len());
    for (name, source) in sources {
        let Ok(module) = ns_syntax::parse(&source) else {
            return Ok(None);
        };
        let origin = ns_runtime::SourceOrigin::Embedded(name);
        let Ok(resolved) = ns_runtime::resolve(module, Rc::from(source.as_str()), origin) else {
            return Ok(None);
        };
        resolved_modules.push(resolved);
    }
    Ok(Some(resolved_modules))
}

/// The modules `run` compiles for `program`, in its module order, each
/// with a name and its text: a source file alone, or a project's main
/// module, its other modules and the manifest. `None` for a project file
/// the CLI would refuse, which compiles nothing.
fn module_sources(program: &Path) -> Result<Option<Vec<(String, String)>>, Failed> {
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .map(|text| (path.display().to_string(), text))
            .map_err(|error| Failed::from(format!("read {}: {error}", path.display())))
    };
    if program.is_file() {
        return Ok(Some(vec![read(program)?]));
    }
    let project_file = program.join(common::PROJECT_FILE);
    let text = std::fs::read_to_string(&project_file)
        .map_err(|error| format!("read {}: {error}", project_file.display()))?;
    let Ok(project) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(None);
    };
    let Some(main) = project.get("main").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let mut written = vec![main];
    if let Some(modules) = project.get("modules").and_then(serde_json::Value::as_array) {
        written.extend(modules.iter().filter_map(serde_json::Value::as_str));
    }
    let mut sources = Vec::with_capacity(written.len() + 1);
    for path in written {
        let path: PathBuf = program.join(path.replace('\\', "/"));
        if !path.is_file() {
            return Ok(None);
        }
        sources.push(read(&path)?);
    }
    sources.push((
        "Manifest.ns".to_string(),
        ns_runtime::MANIFEST_SOURCE.to_string(),
    ));
    Ok(Some(sources))
}
