//! A program as the CLI starts it: one source file whose root is
//! instantiated with `new`, or a project -- a directory, or its
//! `newspeak.json` -- whose root is instantiated with `packageUsing:` a
//! manifest of the other modules. Either way the platform arrives only as
//! `main:args:`'s first argument (`docs/design/image-format.md`,
//! "Packaging").

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::ExitCode;
use std::rc::Rc;

use ns_runtime::modules::{self, ModuleId, ModuleIdentity};
use ns_tooling::source;

use crate::debug::{self, HostLink};
use crate::project;

pub(crate) enum Assembly {
    New,
    Package,
}

/// Every source in module order: the root, then the project file's
/// modules as written, then the manifest. `build` writes them in this
/// order and `load` rebuilds them in it, which keeps every identity hash
/// the same across `run`, `build` and `load`. Each source's
/// `resolved.identity` is set (`docs/design/modules.md`, decisions 1, 2,
/// 5 and 9), before any is compiled.
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
    /// The manifest a project's application was assembled with, once made.
    manifest: Option<ns_runtime::Root>,
    /// How many of the manifest's ambiguities were warned of already;
    /// back to 0 with each new manifest.
    ambiguities_warned: usize,
    /// Whether ambiguities are printed: `bench` assembles the application
    /// again for every sample and turns it off after the first assembly.
    pub(crate) warns: bool,
}

/// The slot of `Manifest.ns` that holds the texts `ambiguities` answers.
const AMBIGUITIES_SLOT: &str = "ambiguitiesFound";

/// The name of the manifest's source file, as the binary embeds it.
const MANIFEST_FILE: &str = "Manifest.ns";

pub(crate) fn read_program(path: &Path) -> Result<Program, ExitCode> {
    let report = |message: String| {
        eprintln!("error: {message}");
        ExitCode::FAILURE
    };
    let located = project::locate_program(path).map_err(report)?;
    let path = match located {
        project::ProgramPath::Source(path) => {
            let mut loaded = source::load_source(&path)?;
            let directory = path.parent().unwrap_or(Path::new(""));
            let identity = program_identity(&loaded, None, directory, &path).map_err(report)?;
            loaded.resolved.identity = Some(Rc::new(identity));
            return Ok(Program {
                assembly: Assembly::New,
                sources: vec![loaded],
                scripts: None,
            });
        }
        project::ProgramPath::Project(path) => path,
    };
    let project = project::read_project(&path).map_err(report)?;
    let directory = path.parent().unwrap_or(Path::new(""));
    let mut sources = vec![source::load_source(&project.main)?];
    for module in &project.modules {
        sources.push(source::load_source(&module.path)?);
    }
    let files =
        std::iter::once((&project.main, None)).chain(project.modules.iter().map(|module| {
            let given = module
                .id
                .as_deref()
                .map(|id| GivenId { id, project: &path });
            (&module.path, given)
        }));
    for (loaded, (file, given)) in sources.iter_mut().zip(files) {
        let identity = program_identity(loaded, given, directory, file).map_err(report)?;
        loaded.resolved.identity = Some(Rc::new(identity));
    }
    // `main`'s module is in no manifest, but tools name it by its id too,
    // so it shares no id and version with another module either.
    modules::check_distinct(
        sources
            .iter()
            .filter_map(|loaded| loaded.resolved.identity.as_deref()),
    )
    .map_err(report)?;
    let mut manifest = source::load_embedded(MANIFEST_FILE, ns_runtime::MANIFEST_SOURCE);
    manifest.resolved.identity = Some(Rc::new(manifest_identity(&manifest)));
    sources.push(manifest);
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
            "error: --script names a script of a project's newspeak.json, and the program \
             given is a source file"
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

/// Every module of the program by its id and version, the embedded
/// manifest of a project included.
pub(crate) fn module_names(program: &Program) -> Vec<ns_runtime::ModuleName> {
    program
        .sources
        .iter()
        .filter_map(|loaded| loaded.resolved.identity.as_deref())
        .map(ns_runtime::ModuleName::of_identity)
        .collect()
}

/// The program's `modules` and every shipped module: what a tool's name
/// -- a breakpoint's, a trace filter's -- may name a module of
/// (`KnownModule::resolve`).
pub(crate) fn known_modules(
    modules: impl IntoIterator<Item = ns_runtime::ModuleName>,
) -> Vec<ns_runtime::ModuleName> {
    modules
        .into_iter()
        .chain(
            ns_runtime::shipped_source_files().iter().map(|file| {
                ns_runtime::ModuleName::named(ns_runtime::shipped_module_id(file.name))
            }),
        )
        .collect()
}

/// The program's modules as a debugger shows them: each by its id and
/// version, with the file it was read from and its text as read.
/// A file is shown by its absolute path as the program named it, and
/// known by its canonical one, which a client's path is compared with
/// (`debug::file_identity`).
pub(crate) fn debug_modules(program: &Program) -> Vec<debug::ProgramModule> {
    program
        .sources
        .iter()
        .map(|loaded| {
            let path = match &loaded.origin {
                ns_runtime::SourceOrigin::File(path) => {
                    Some(std::path::absolute(path).unwrap_or_else(|_| path.clone()))
                }
                ns_runtime::SourceOrigin::Embedded(_) => None,
            };
            debug::ProgramModule {
                module: match loaded.resolved.identity.as_deref() {
                    Some(identity) => ns_runtime::ModuleName::of_identity(identity),
                    None => {
                        unreachable!("internal: `read_program` gives every module its identity")
                    }
                },
                identity: path.as_deref().map(debug::file_identity),
                path,
                text: Some(loaded.source.clone()),
            }
        })
        .collect()
}

fn declared_name(loaded: &source::LoadedSource) -> &str {
    &loaded.resolved.module.root_class.name.name
}

/// The id a project file's `as` gives a module, and the project file, for
/// a refusal.
struct GivenId<'text> {
    id: &'text str,
    project: &'text Path,
}

/// The identity of the program's module `loaded`, read from `file`: its
/// path relative to `directory`, the project's or the source file's, the
/// id `given` names or else the one the module declares, and the version
/// it declares. Refuses a malformed declaration, an `as` that does not end
/// in the class's name, and an id in the reserved namespace, each
/// prefixed by the file that holds it.
fn program_identity(
    loaded: &source::LoadedSource,
    given: Option<GivenId<'_>>,
    directory: &Path,
    file: &Path,
) -> Result<ModuleIdentity, String> {
    let in_file = |error: String| format!("{}: {error}", file.display());
    let (declared, version) =
        modules::declared_identity(&loaded.resolved.module, &loaded.source).map_err(in_file)?;
    // A module that declares a reserved id is refused even under an `as`:
    // its source claims to be the binary's (decision 9).
    modules::check_unreserved(&declared).map_err(in_file)?;
    let id = match given {
        Some(given) => {
            let in_project = |error: String| {
                format!(
                    "{}: `as` of `{}`: {error}",
                    given.project.display(),
                    file.display()
                )
            };
            let id = ModuleId::parse(given.id, declared_name(loaded)).map_err(in_project)?;
            modules::check_unreserved(&id).map_err(in_project)?;
            id
        }
        None => declared,
    };
    // Held to the rule an image's module record is held to, so that `run`
    // refuses what `build` could not write: no control character, a file
    // that is more than `.ns`, and the rest of a project's path.
    let path = path_in_project(directory, file)?;
    modules::check_project_path(&path).map_err(in_file)?;
    Ok(ModuleIdentity { path, id, version })
}

/// The manifest's identity: the id its `:module:` declares, in the
/// namespace that is the binary's, and no version, as the shipped modules
/// have; its path names the binary's file.
fn manifest_identity(manifest: &source::LoadedSource) -> ModuleIdentity {
    match modules::declared_identity(&manifest.resolved.module, &manifest.source) {
        Ok((id, None)) if id.is_reserved() => ModuleIdentity {
            path: modules::core_path(MANIFEST_FILE),
            id,
            version: None,
        },
        _ => panic!("internal: the embedded {MANIFEST_FILE} declares no id of the binary's"),
    }
}

/// `file`'s path relative to `directory`, `/`-separated, a `..` for each
/// directory it lies outside: what a module is told apart by
/// (`docs/design/modules.md`, decision 1), so that the paths of the
/// building machine are written nowhere. Both are made absolute against
/// the working directory and their `.` and `..` resolved by their text;
/// the file system is not asked, so a symbolic link is not seen through.
/// A file on another drive or share than the directory has no such path,
/// and is refused.
fn path_in_project(directory: &Path, file: &Path) -> Result<String, String> {
    ns_runtime::reach::record(ns_runtime::reach::Reach::Platform);
    let absolute = |path: &Path| {
        // An empty path is the working directory, which `absolute` refuses.
        let path = if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            path
        };
        std::path::absolute(path)
            .map(|path| normalized_components(&path))
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    let base = absolute(directory)?;
    let target = absolute(file)?;
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(first, second)| same_component(first, second))
        .count();
    if common == 0 {
        return Err(format!(
            "{} lies on another drive or share than the project's directory {}, so it has no \
             path relative to the project",
            file.display(),
            directory.display()
        ));
    }
    let parents = std::iter::repeat_n(String::from(".."), base.len() - common);
    let names = target[common..]
        .iter()
        .map(|component| component.to_string_lossy().into_owned());
    Ok(parents.chain(names).collect::<Vec<_>>().join("/"))
}

/// The components of an absolute `path`, its `.` dropped and each `..`
/// taking the name before it away.
fn normalized_components(path: &Path) -> Vec<OsString> {
    let mut components: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match components.last() {
                Some(Component::Normal(_)) => {
                    components.pop();
                }
                // Above the root is the root.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => components.push(component),
            },
            other => components.push(other),
        }
    }
    components.iter().map(component_text).collect()
}

/// A component's text, a prefix's in its plain form: a verbatim prefix
/// names the drive or the share its plain form names (`\\?\C:` is `C:`,
/// `\\?\UNC\server\share` is `\\server\share`), so that a file named one
/// way lies in a directory named the other.
fn component_text(component: &Component<'_>) -> OsString {
    let Component::Prefix(prefix) = component else {
        return component.as_os_str().to_os_string();
    };
    match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
            OsString::from(format!("{}:", char::from(drive)))
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            let mut text = OsString::from(r"\\");
            text.push(server);
            text.push(r"\");
            text.push(share);
            text
        }
        Prefix::Verbatim(_) | Prefix::DeviceNS(_) => component.as_os_str().to_os_string(),
    }
}

/// Whether two components name one directory: on Windows, whose file
/// systems ignore case, whatever the case of their letters.
fn same_component(first: &OsString, second: &OsString) -> bool {
    if cfg!(windows) {
        first.eq_ignore_ascii_case(second)
    } else {
        first == second
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
        manifest: None,
        ambiguities_warned: 0,
        warns: true,
    }
}

/// Prints a warning for each ambiguity the manifest recorded that none was
/// printed for yet (`docs/design/modules.md`, decision 4): a request of
/// `at:version:` that matched several versions, which took the highest
/// silently otherwise. The texts are read out of the manifest's slot
/// rather than asked of it, so that no host run is started for them: a
/// debugger's step carried past a run's end, or a pause asked between
/// runs, would stop in the manifest instead of in the program.
pub(crate) fn warn_of_ambiguities(loaded: &mut Loaded) {
    let Some(manifest) = &loaded.manifest else {
        return;
    };
    let texts = loaded
        .interpreter
        .enter(|mutator| manifest_ambiguities(mutator, manifest));
    if loaded.warns {
        for text in texts.iter().skip(loaded.ambiguities_warned) {
            eprintln!("warning: {text}");
        }
    }
    loaded.ambiguities_warned = loaded.ambiguities_warned.max(texts.len());
}

/// The manifests a loaded image's application kept, each with how many
/// ambiguities it held when the image was loaded: those its assembly
/// recorded, which `build` warned of.
pub(crate) struct KeptManifests(Vec<(ns_runtime::Root, usize)>);

/// The manifests the image `image` holds: every instance of its manifest
/// module's root class (the module record at `core:Manifest.ns`, which the
/// decoder holds to `newspeak.core.Manifest`). None for an image without a
/// manifest -- a one-file program's -- or whose application did not keep
/// it, since the graph holds only what the application reached.
pub(crate) fn kept_manifests(image: &mut ns_runtime::image::LoadedImage) -> KeptManifests {
    let manifest_path = modules::core_path(MANIFEST_FILE);
    let Some(module) = image
        .modules
        .iter()
        .find(|module| module.identity.path == manifest_path)
    else {
        return KeptManifests(Vec::new());
    };
    let class = &module.class;
    let manifests: Vec<ns_runtime::Root> = image.interpreter.enter(|mutator| {
        let class = crate::entry::class_reference(mutator, class);
        mutator
            .heap()
            .instances_of(class)
            .into_iter()
            .map(|instance| mutator.stash(ns_runtime::Value::object(instance)))
            .collect()
    });
    KeptManifests(
        manifests
            .into_iter()
            .map(|manifest| {
                let count = image
                    .interpreter
                    .enter(|mutator| manifest_ambiguities(mutator, &manifest).len());
                (manifest, count)
            })
            .collect(),
    )
}

/// Prints a warning for each ambiguity a kept manifest recorded since the
/// load -- the requests `main:args:` made -- as `warn_of_ambiguities` does
/// for `run` (`docs/design/modules.md`, decision 4).
pub(crate) fn warn_of_kept_ambiguities(
    interpreter: &mut ns_runtime::Interpreter,
    kept: &mut KeptManifests,
) {
    for (manifest, warned) in &mut kept.0 {
        let texts = interpreter.enter(|mutator| manifest_ambiguities(mutator, manifest));
        for text in texts.iter().skip(*warned) {
            eprintln!("warning: {text}");
        }
        *warned = (*warned).max(texts.len());
    }
}

/// The Strings of the manifest's ambiguities slot, which only its
/// `at:version:` writes, each time with a longer copy.
fn manifest_ambiguities(
    mutator: &ns_runtime::Mutator<'_>,
    manifest: &ns_runtime::Root,
) -> Vec<String> {
    let value = mutator.fetch(manifest);
    let heap = mutator.heap();
    let slot = ns_runtime::Symbol::intern(AMBIGUITIES_SLOT);
    let field = value.as_object().and_then(|object| {
        let application = heap.class_application(mutator.class_of(value))?;
        let position = application
            .mixin
            .slots
            .iter()
            .position(|declared| declared.name == slot)?;
        heap.instance_field(object, application.own_slot_base + position)
    });
    field
        .and_then(|field| field.as_object())
        .and_then(|array| heap.array_elements(array))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|element| element.as_object().and_then(|text| heap.str_value(text)))
        .collect()
}

/// The application: the root sent `new`, or `packageUsing:` a manifest of
/// every other module but the manifest's own. A root whose factory is not
/// the one its assembly sends is refused before anything runs, naming the
/// factory it has. Under a debugger, `link` ends the program before either
/// host run once a `terminate` arrived.
pub(crate) fn instantiate_application(
    program: &Program,
    loaded: &mut Loaded,
    link: Option<&HostLink>,
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
        (Assembly::Package, Some("packageUsing:")) => vec![build_manifest(program, loaded, link)?],
        (Assembly::Package, other) => {
            eprintln!(
                "error: cannot start `{name}`: its primary factory is `{}`, and a project's root \
                 is instantiated with `packageUsing:` its manifest",
                other.unwrap_or("new")
            );
            return Err(ExitCode::FAILURE);
        }
    };
    if debug::terminated_before_run(link) {
        return Err(debug::report_terminated());
    }
    let application = loaded
        .interpreter
        .instantiate(&loaded.classes[0], &arguments);
    warn_of_ambiguities(loaded);
    application.map_err(crate::entry::report_instantiation)
}

pub(crate) fn selector_of(pattern: &ns_syntax::ast::MessagePattern) -> String {
    match pattern {
        ns_syntax::ast::MessagePattern::Unary { selector, .. } => selector.name.clone(),
        ns_syntax::ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ns_syntax::ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| format!("{}:", part.keyword.name))
            .collect(),
    }
}

/// The manifest over the project's modules -- every source but the root and
/// the manifest itself -- `Manifest ids:versions:ranks:definitions:`, each
/// module's id, version's text or nil, rank and class object
/// (`docs/design/modules.md`, decisions 3 and 4).
fn build_manifest(
    program: &Program,
    loaded: &mut Loaded,
    link: Option<&HostLink>,
) -> Result<ns_runtime::Root, ExitCode> {
    let last = program.sources.len() - 1;
    let identities: Vec<&ModuleIdentity> = program.sources[1..last]
        .iter()
        .map(|loaded| match loaded.resolved.identity.as_deref() {
            Some(identity) => identity,
            None => unreachable!("internal: `read_program` gives every module its identity"),
        })
        .collect();
    let ranks = ranks(&identities);
    let classes = &loaded.classes[1..last];
    let arguments = loaded.interpreter.enter(|mutator| {
        let ids = identities
            .iter()
            .map(|identity| {
                ns_runtime::Value::object(mutator.new_string(identity.id.as_str().to_string()))
            })
            .collect();
        let versions = identities
            .iter()
            .map(|identity| match &identity.version {
                Some(version) => ns_runtime::Value::object(mutator.new_string(version.to_string())),
                None => ns_runtime::Value::nil(),
            })
            .collect();
        let ranks = ranks
            .iter()
            .map(|rank| ns_runtime::Value::small_integer(*rank))
            .collect();
        let definitions = classes.iter().map(|class| mutator.fetch(class)).collect();
        [ids, versions, ranks, definitions].map(|elements| {
            let array = mutator.new_array(elements);
            mutator.stash(ns_runtime::Value::object(array))
        })
    });
    if debug::terminated_before_run(link) {
        return Err(debug::report_terminated());
    }
    let manifest = loaded
        .interpreter
        .instantiate(&loaded.classes[last], &arguments)
        .map_err(crate::entry::report_instantiation)?;
    loaded.manifest = Some(manifest.clone());
    // A new manifest has recorded nothing yet.
    loaded.ambiguities_warned = 0;
    Ok(manifest)
}

/// Each module's place, counted from 0, among the modules of its id by
/// semver's precedence: the order the manifest's `at:version:` walks, so
/// that `Manifest.ns` compares integers rather than versions. A module
/// without a version is alone with its id (`modules::check_distinct`), so
/// its place is 0.
fn ranks(identities: &[&ModuleIdentity]) -> Vec<i64> {
    identities
        .iter()
        .map(|identity| {
            let below = identities
                .iter()
                .filter(|other| {
                    other.id == identity.id
                        && matches!(
                            (&other.version, &identity.version),
                            (Some(lower), Some(version)) if lower < version
                        )
                })
                .count();
            i64::try_from(below).unwrap_or(i64::MAX)
        })
        .collect()
}

/// The modules as `image::package` takes them, in module order, each
/// named by its identity -- its path in the project, never the directory
/// it was read from. Takes the classes rather than `Loaded`, because the
/// caller holds `Loaded`'s interpreter mutably while it encodes.
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
            identity: match source.resolved.identity.as_deref() {
                Some(identity) => identity,
                None => unreachable!("internal: `read_program` gives every module its identity"),
            },
            with_source,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative(directory: &str, file: &str) -> String {
        path_in_project(Path::new(directory), Path::new(file))
            .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_module_s_path_is_relative_to_the_project_with_slashes() {
        assert_eq!(
            relative("project", "project/libs/a/Util.ns"),
            "libs/a/Util.ns"
        );
        assert_eq!(
            relative("project", "project/./libs//Util.ns"),
            "libs/Util.ns"
        );
        assert_eq!(
            relative("project/app", "project/shared/Util.ns"),
            "../shared/Util.ns"
        );
        assert_eq!(relative("project", "project/libs/../Util.ns"), "Util.ns");
        assert_eq!(relative("", "App.ns"), "App.ns");
    }

    #[cfg(windows)]
    #[test]
    fn a_module_on_another_drive_has_no_path_in_the_project() {
        // Every path absolute with its own drive or share, so that the
        // answer does not depend on the drive the working directory is on.
        for file in [r"\\server\share\A.ns", r"D:\A.ns"] {
            let error = match path_in_project(Path::new(r"C:\project"), Path::new(file)) {
                Ok(path) => panic!("{file} answered {path}"),
                Err(error) => error,
            };
            assert!(error.contains("another drive or share"), "{file}: {error}");
        }
        assert_eq!(
            path_in_project(Path::new(r"C:\project"), Path::new(r"c:\project\libs\A.ns")),
            Ok("libs/A.ns".to_string())
        );
    }

    /// A verbatim path names the drive or share its plain form names, so
    /// that a file named one way lies in a directory named the other.
    #[cfg(windows)]
    #[test]
    fn a_verbatim_path_lies_on_the_drive_its_plain_form_names() {
        for (directory, file) in [
            (r"\\?\C:\project", r"C:\project\libs\A.ns"),
            (r"C:\project", r"\\?\c:\project\libs\A.ns"),
            (
                r"\\?\UNC\server\share\project",
                r"\\server\share\project\libs\A.ns",
            ),
            (
                r"\\server\share\project",
                r"\\?\UNC\server\share\project\libs\A.ns",
            ),
        ] {
            assert_eq!(
                path_in_project(Path::new(directory), Path::new(file)),
                Ok("libs/A.ns".to_string()),
                "{directory} and {file}"
            );
        }
        let error = match path_in_project(Path::new(r"\\?\C:\project"), Path::new(r"D:\A.ns")) {
            Ok(path) => panic!(r"D:\A.ns answered {path}"),
            Err(error) => error,
        };
        assert!(error.contains("another drive or share"), "{error}");
    }
}
