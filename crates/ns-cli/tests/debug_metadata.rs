//! The debugger's oracle over every golden (`docs/design/debugger.md`,
//! decision 14): every body of every golden's modules, compiled from its
//! source under each lowering, compiles again alike and its metadata holds
//! its invariants (`Interpreter::check_debug_metadata`). One trial per
//! golden and lowering (`libtest-mimic`, `harness = false`), as in
//! `run_goldens.rs`, in process: nothing runs, so no `ns-next` is started.
//! The shipped modules' bodies are checked once, by `ns-runtime`'s
//! `every_shipped_body_compiles_again_alike`, not with every golden; the
//! same check through `build` and `load` is `golden_round_trips.rs`'s.
//! Beside it, for every golden of `goldens/language/`, the breakable
//! statements the language server is answered
//! (`Interpreter::breakable_statements`) are exactly the statements a
//! `SetBreakpoints` lands on under the optimized lowering, and each lands
//! on itself under the reference lowering; and so are those it is
//! answered for a document holding a shipped module's text
//! (`Interpreter::shipped_breakable_statements`), once per lowering.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use libtest_mimic::{Arguments, Failed, Trial};
use ns_runtime::{
    BreakableStatement, BreakpointState, Command, Event, HostSignal, Interpreter, LineBreakpoint,
    Lowering, ModuleName,
};
use ns_syntax::Span;

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
            if name.starts_with("language/") {
                let path = program.clone();
                trials.push(Trial::test(
                    format!(
                        "breakable_statements_agree_with_the_breakpoints::{lowering_name}::{name}"
                    ),
                    move || breakable_statements_agree_with_the_breakpoints(&path, lowering),
                ));
            }
        }
    }
    for (lowering_name, lowering) in LOWERINGS {
        trials.push(Trial::test(
            format!("shipped_breakable_statements_agree_with_the_breakpoints::{lowering_name}"),
            move || shipped_breakable_statements_agree_with_the_breakpoints(lowering),
        ));
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

/// How long the test waits for the VM's answer to a `SetBreakpoints`,
/// which a run served before it ended: it is already queued.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Every breakable statement of each of `program`'s modules, asked before
/// the module is loaded, is where a `SetBreakpoints` at its start lands
/// once it is loaded under `lowering`. The answer is the optimized
/// lowering's whatever the interpreter's, so an interpreter of the other
/// lowering answers the same; under the optimized lowering nothing else
/// is landed on either: a breakpoint at the module's start lands on the
/// first, and one just past a statement's start on the next, or on none
/// after the last. The reference lowering compiles a few statements the
/// optimized one compiles to no instruction, which a breakpoint there
/// lands on instead.
fn breakable_statements_agree_with_the_breakpoints(
    program: &Path,
    lowering: Lowering,
) -> Result<(), Failed> {
    let Some(resolved_modules) = resolved_modules(program)? else {
        return Ok(());
    };
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    interpreter.set_jit_enabled(false);
    let breakable: Vec<Vec<BreakableStatement>> = resolved_modules
        .iter()
        .map(|resolved| interpreter.breakable_statements(resolved))
        .collect();
    let other = match lowering {
        Lowering::Optimized => Lowering::Reference,
        Lowering::Reference => Lowering::Optimized,
    };
    let mut other_interpreter = Interpreter::bootstrap_with(other);
    for (resolved, statements) in resolved_modules.iter().zip(&breakable) {
        let others = other_interpreter.breakable_statements(resolved);
        if others != *statements {
            return Err(format!(
                "an interpreter of the other lowering answers {:?}, this one {:?}",
                describe(
                    &resolved.source,
                    others.iter().map(|breakable| Some(breakable.statement))
                ),
                describe(
                    &resolved.source,
                    statements.iter().map(|breakable| Some(breakable.statement))
                ),
            )
            .into());
        }
    }
    for resolved in &resolved_modules {
        interpreter.load_module(resolved);
    }
    let modules: Vec<Landings<'_>> = resolved_modules
        .iter()
        .zip(&breakable)
        .map(|(resolved, statements)| Landings {
            module: ModuleName::named(resolved.module.root_class.name.name.clone()),
            source: &resolved.source,
            expected: expected_landings(statements, lowering),
        })
        .collect();
    breakpoints_land_as_expected(&mut interpreter, &modules)
}

/// Every breakable statement of each shipped module the language server
/// is answered for a document holding its text
/// (`Interpreter::shipped_breakable_statements`), asked before any lazy
/// one is built, is where a `SetBreakpoints` at its start lands once all
/// are built under `lowering`, as `breakable_statements_agree_with_the_breakpoints`
/// holds a program's modules to it; an interpreter of the other lowering
/// answers the same.
fn shipped_breakable_statements_agree_with_the_breakpoints(
    lowering: Lowering,
) -> Result<(), Failed> {
    let names: Vec<&str> = ns_runtime::shipped_source_files()
        .iter()
        .filter(|file| ns_runtime::shipped_module_named_by_text(file.text) == Some(file.name))
        .map(|file| file.name)
        .collect();
    if names.len() < 7 {
        return Err(format!("only {names:?} are shipped modules").into());
    }
    let other = match lowering {
        Lowering::Optimized => Lowering::Reference,
        Lowering::Reference => Lowering::Optimized,
    };
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    interpreter.set_jit_enabled(false);
    let mut other_interpreter = Interpreter::bootstrap_with(other);
    let mut modules = Vec::with_capacity(names.len());
    for name in names {
        let statements = interpreter
            .shipped_breakable_statements(name)
            .ok_or_else(|| format!("{name}: no breakable statements are answered"))?;
        if statements.is_empty() {
            return Err(format!("{name}: no statement is breakable").into());
        }
        if other_interpreter
            .shipped_breakable_statements(name)
            .as_ref()
            != Some(&statements)
        {
            return Err(
                format!("{name}: an interpreter of the other lowering answers otherwise").into(),
            );
        }
        let text = ns_runtime::shipped_module_source(name).unwrap_or_default();
        modules.push(Landings {
            module: ModuleName::named(ns_runtime::shipped_module_id(name)),
            source: text,
            expected: expected_landings(&statements, lowering),
        });
    }
    // Builds every lazy shipped module, so that a breakpoint in one
    // resolves rather than staying pending.
    interpreter.check_debug_metadata(true)?;
    breakpoints_land_as_expected(&mut interpreter, &modules)
}

/// Each breakpoint's offset for `statements`, and the statement it lands
/// on under `lowering`: at each start, the statement itself; and under the
/// optimized lowering, at the module's start the first, and just past a
/// statement's start the next, or none after the last.
fn expected_landings(
    statements: &[BreakableStatement],
    lowering: Lowering,
) -> Vec<(u32, Option<Span>)> {
    let at_starts = statements
        .iter()
        .map(|breakable| (breakable.statement.start, Some(breakable.statement)));
    if lowering == Lowering::Reference {
        return at_starts.collect();
    }
    let first = (0, statements.first().map(|breakable| breakable.statement));
    let just_past = statements.iter().enumerate().map(|(index, breakable)| {
        let next = statements.get(index + 1);
        (
            breakable.statement.start + 1,
            next.map(|next| next.statement),
        )
    });
    std::iter::once(first)
        .chain(at_starts)
        .chain(just_past)
        .collect()
}

/// The breakpoints a module is set, each beside where it should land.
struct Landings<'text> {
    /// The module.
    module: ModuleName,
    source: &'text str,
    /// Each breakpoint's offset and the statement it should land on.
    expected: Vec<(u32, Option<Span>)>,
}

/// A `SetBreakpoints` of each of `modules` lands every breakpoint where it
/// is expected, once a run serves them.
fn breakpoints_land_as_expected(
    interpreter: &mut Interpreter,
    modules: &[Landings<'_>],
) -> Result<(), Failed> {
    let remote = interpreter.attach_debugger();
    for Landings {
        module, expected, ..
    } in modules
    {
        remote.send(Command::SetBreakpoints {
            module: module.clone(),
            breakpoints: expected
                .iter()
                .map(|(offset, _)| LineBreakpoint::at_offset(*offset))
                .collect(),
        });
        // Taken off again before anything runs, so that no breakpoint
        // stops the run that serves them.
        remote.send(Command::SetBreakpoints {
            module: module.clone(),
            breakpoints: Vec::new(),
        });
    }
    // Any run serves the configuration queued before it, as it starts;
    // the instantiation of a module of no golden's is one.
    let serving = resolved_text(SERVING)?;
    let serving = interpreter.load_module(&serving);
    match interpreter.instantiate(&serving, &[]) {
        Ok(_) => {}
        Err(HostSignal::Return) => return Err("a non-local return escaped".into()),
        Err(HostSignal::Error(error)) => return Err(error.message.into()),
    }
    for Landings {
        module,
        source,
        expected,
    } in modules
    {
        let states = breakpoints_of(&remote, module)?;
        if !breakpoints_of(&remote, module)?.is_empty() {
            return Err(format!("{module}: the second set was not empty").into());
        }
        let landed = states.iter().map(|state| state.statement);
        if !landed
            .clone()
            .eq(expected.iter().map(|(_, statement)| *statement))
        {
            return Err(format!(
                "{module}: the breakpoints landed on {:?}, the breakable statements say {:?}",
                describe(source, landed),
                describe(source, expected.iter().map(|(_, statement)| *statement)),
            )
            .into());
        }
    }
    Ok(())
}

/// The text of each of `spans` in `source`, `-` for none.
fn describe(source: &str, spans: impl Iterator<Item = Option<Span>>) -> Vec<String> {
    spans
        .map(|span| span.map_or("-", |span| span.text(source)).to_string())
        .collect()
}

/// A module whose instantiation is a run, which serves the debugger's
/// configuration as it starts.
const SERVING: &str = "class BreakableStatementsServing = Object new () () : ()
";

/// `source`, parsed and resolved.
fn resolved_text(source: &str) -> Result<ns_runtime::ResolvedModule, Failed> {
    let module = ns_syntax::parse(source).map_err(|_| "parse")?;
    let origin = ns_runtime::SourceOrigin::Embedded("serving.ns".to_string());
    ns_runtime::resolve(module, Rc::from(source), origin).map_err(|_| "resolve".into())
}

/// The VM's answer to the next `SetBreakpoints` of `module`.
fn breakpoints_of(
    remote: &ns_runtime::DebugRemote,
    module: &ModuleName,
) -> Result<Vec<BreakpointState>, Failed> {
    loop {
        match remote.next_event(EVENT_TIMEOUT) {
            Some(Event::Breakpoints {
                module: reported,
                breakpoints,
            }) if reported == *module => return Ok(breakpoints),
            Some(Event::Breakpoints { .. }) => {}
            other => {
                return Err(
                    format!("the breakpoints of {module} were expected, not {other:?}").into(),
                );
            }
        }
    }
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
