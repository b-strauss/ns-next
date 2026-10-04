//! A program as the CLI starts it: one source file whose root is
//! instantiated with `new`, or a project -- a directory, or its
//! `newspeak.json` -- whose root is instantiated with `packageUsing:` a
//! manifest of the other modules. Either way the platform arrives only as
//! `main:args:`'s first argument (`docs/design/image-format.md`,
//! "Packaging").

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ns_tooling::source;

use crate::project;

pub(crate) enum Assembly {
    New,
    Package,
}

/// Every source in module order: the root, then the project file's
/// modules as written, then the manifest. `build` writes them in this
/// order and `load` rebuilds them in it, which keeps every identity hash
/// the same across `run`, `build` and `load`.
pub(crate) struct Program {
    pub(crate) assembly: Assembly,
    pub(crate) sources: Vec<source::LoadedSource>,
    /// The project file's path, for messages, and its scripts; `None` for a
    /// source file.
    pub(crate) scripts: Option<(PathBuf, BTreeMap<String, Vec<String>>)>,
}

pub(crate) struct Loaded {
    pub(crate) interpreter: ns_runtime::Interpreter,
    pub(crate) classes: Vec<ns_runtime::Root>,
}

pub(crate) fn read_program(path: &Path) -> Result<Program, ExitCode> {
    let located = project::locate_program(path).map_err(|message| {
        eprintln!("error: {message}");
        ExitCode::FAILURE
    })?;
    let path = match located {
        project::ProgramPath::Source(path) => {
            return Ok(Program {
                assembly: Assembly::New,
                sources: vec![source::load_source(&path)?],
                scripts: None,
            });
        }
        project::ProgramPath::Project(path) => path,
    };
    let project = project::read_project(&path).map_err(|message| {
        eprintln!("error: {message}");
        ExitCode::FAILURE
    })?;
    let mut sources = vec![source::load_source(&project.main)?];
    for module in &project.modules {
        sources.push(source::load_source(module)?);
    }
    // Two modules of one name: the manifest could answer only one of them.
    for (index, later) in sources.iter().enumerate().skip(1) {
        let name = declared_name(later);
        if let Some(earlier) = sources[1..index]
            .iter()
            .find(|earlier| declared_name(earlier) == name)
        {
            eprintln!(
                "error: two modules declare `{name}`: {} and {}",
                describe_origin(&earlier.origin),
                describe_origin(&later.origin)
            );
            return Err(ExitCode::FAILURE);
        }
    }
    sources.push(source::load_embedded(
        "Manifest.ns",
        ns_runtime::MANIFEST_SOURCE,
    ));
    Ok(Program {
        assembly: Assembly::Package,
        sources,
        scripts: Some((path, project.scripts)),
    })
}

/// What `main:args:` receives: the script's arguments, when `script` names
/// one, then those given after `--`. A script is a project's, so naming one
/// for a source file, or one the project lacks, is refused before anything
/// runs.
pub(crate) fn entry_arguments(
    program: &Program,
    script: Option<&str>,
    arguments: &[String],
) -> Result<Vec<String>, ExitCode> {
    let Some(name) = script else {
        return Ok(arguments.to_vec());
    };
    let Some((project_file, scripts)) = &program.scripts else {
        eprintln!(
            "error: --script names a script of a project's newspeak.json, and the program              given is a source file"
        );
        return Err(ExitCode::FAILURE);
    };
    let Some(script_arguments) = scripts.get(name) else {
        let known = if scripts.is_empty() {
            "it has none".to_string()
        } else {
            format!(
                "its scripts are {}",
                scripts
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        eprintln!(
            "error: {} has no script `{name}`; {known}",
            project_file.display()
        );
        return Err(ExitCode::FAILURE);
    };
    Ok(script_arguments.iter().chain(arguments).cloned().collect())
}

fn declared_name(loaded: &source::LoadedSource) -> &str {
    &loaded.resolved.module.root_class.name.name
}

fn describe_origin(origin: &ns_runtime::SourceOrigin) -> String {
    match origin {
        ns_runtime::SourceOrigin::File(path) => path.display().to_string(),
        ns_runtime::SourceOrigin::Embedded(name) => format!("the embedded {name}"),
    }
}

/// Compiles every module into one interpreter, in module order.
pub(crate) fn load_program(program: &Program, lowering: ns_runtime::Lowering) -> Loaded {
    let mut interpreter = ns_runtime::Interpreter::bootstrap_with(lowering);
    let classes = program
        .sources
        .iter()
        .map(|loaded| interpreter.load_module(&loaded.resolved))
        .collect();
    Loaded {
        interpreter,
        classes,
    }
}

/// The application: the root sent `new`, or `packageUsing:` a manifest of
/// every other module but the manifest's own. A root whose factory is not
/// the one its assembly sends is refused before anything runs, naming the
/// factory it has.
pub(crate) fn instantiate_application(
    program: &Program,
    loaded: &mut Loaded,
) -> Result<ns_runtime::Root, ExitCode> {
    let root = &program.sources[0];
    let name = declared_name(root);
    let factory = root
        .resolved
        .module
        .root_class
        .factory
        .as_ref()
        .map(selector_of);
    let arguments = match (&program.assembly, factory.as_deref()) {
        (Assembly::New, None | Some("new")) => Vec::new(),
        (Assembly::New, Some(other)) => {
            eprintln!(
                "error: cannot start `{name}`: its primary factory is `{other}`, and a program \
                 started from a source file is instantiated with `new` -- the platform arrives \
                 as `main:args:`'s first argument"
            );
            return Err(ExitCode::FAILURE);
        }
        (Assembly::Package, Some("packageUsing:")) => vec![build_manifest(program, loaded)?],
        (Assembly::Package, other) => {
            eprintln!(
                "error: cannot start `{name}`: its primary factory is `{}`, and a project's root \
                 is instantiated with `packageUsing:` its manifest",
                other.unwrap_or("new")
            );
            return Err(ExitCode::FAILURE);
        }
    };
    loaded
        .interpreter
        .instantiate(&loaded.classes[0], &arguments)
        .map_err(crate::entry::report_instantiation)
}

pub(crate) fn selector_of(pattern: &ns_syntax::ast::MessagePattern) -> String {
    match pattern {
        ns_syntax::ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ns_syntax::ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ns_syntax::ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, _)| format!("{}:", keyword.name))
            .collect(),
    }
}

/// The manifest over the project's modules -- every source but the root and
/// the manifest itself -- refused when a module's name is one of `Object`'s
/// public members, which would answer the send before the manifest's
/// `doesNotUnderstand:` could.
fn build_manifest(program: &Program, loaded: &mut Loaded) -> Result<ns_runtime::Root, ExitCode> {
    let last = program.sources.len() - 1;
    let modules: Vec<(ns_runtime::Symbol, ns_runtime::Root)> = (1..last)
        .map(|index| {
            (
                ns_runtime::Symbol::intern(declared_name(&program.sources[index])),
                loaded.classes[index].clone(),
            )
        })
        .collect();
    let (names, definitions) = loaded.interpreter.enter(|mutator| {
        let names = modules
            .iter()
            .map(|(name, _)| mutator.new_symbol(*name))
            .collect();
        let definitions = modules
            .iter()
            .map(|(_, class)| mutator.fetch(class))
            .collect();
        let names = mutator.new_array(names);
        let definitions = mutator.new_array(definitions);
        (
            mutator.stash(ns_runtime::Value::object(names)),
            mutator.stash(ns_runtime::Value::object(definitions)),
        )
    });
    let manifest = loaded
        .interpreter
        .instantiate(&loaded.classes[last], &[names, definitions])
        .map_err(crate::entry::report_instantiation)?;
    for (name, _) in &modules {
        if loaded.interpreter.responds_publicly(&manifest, *name) {
            eprintln!(
                "error: the module `{name}` cannot be reached through the manifest: `{name}` is a \
                 public member of every object, which answers before the manifest can"
            );
            return Err(ExitCode::FAILURE);
        }
    }
    Ok(manifest)
}

/// The modules as `image::package` takes them, in module order. Takes the
/// classes rather than `Loaded`, because the caller holds `Loaded`'s
/// interpreter mutably while it encodes.
pub(crate) fn module_sources<'program, 'gc>(
    program: &'program Program,
    classes: &[ns_runtime::Root],
    mutator: &ns_runtime::Mutator<'gc>,
    with_source: bool,
) -> Vec<ns_runtime::image::ModuleSource<'program, 'gc>> {
    program
        .sources
        .iter()
        .zip(classes)
        .map(|(source, class)| ns_runtime::image::ModuleSource {
            class: crate::entry::class_reference(mutator, class),
            origin: &source.origin,
            source: with_source.then_some(source.source.as_str()),
        })
        .collect()
}
