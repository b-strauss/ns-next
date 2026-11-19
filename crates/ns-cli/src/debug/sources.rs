//! The modules a stop shows: the program's own, which the host hands over
//! with their files and texts, and every module the VM names, a shipped
//! one included. A module's current text comes from the VM
//! (`Command::ModuleText`) and is kept beside its revision, so that a stop
//! asks again only whether it changed; a module whose current text is its
//! file's as the program was loaded is shown by its path, every other by a
//! `sourceReference` the `source` request answers. A shipped module's text
//! is the binary's own and never changes, so it is never asked of the VM.
//!
//! A shipped module is a module of its own, whatever the program's names:
//! a program may declare a `Kernel` of its own, which is another module,
//! `Kernel` beside the binary's `newspeak.core.Kernel` (`ModuleName`). A
//! client names a program module by its file's path, and
//! a shipped one by the path of its file in the core library the client
//! shows -- a directory of the binary's shipped sources it hands over at
//! `attach` (`coreLibrary`), taken only when its fingerprint is the
//! binary's -- or by the reference a stop showed it by; a shipped frame is
//! shown by that file's path when there is one. No shipped module is ever
//! named by its name, so that neither module takes the other's
//! breakpoints.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ns_runtime::{ModuleName, ModuleTextAnswer};
use ns_syntax::Span;
use ns_tooling::lines::{LineColumn, LineIndex};

use super::protocol::{self, SourceArgument};
use ns_runtime::tool_names::KnownModule;

/// The file a directory of shipped sources holds its fingerprint in, as
/// `ns-next shipped-sources` writes it.
const FINGERPRINT_FILE: &str = "fingerprint";

/// A module of the program as the host loaded it.
pub(crate) struct ProgramModule {
    /// Its id and version, as `FrameLocation::module` names it.
    pub(crate) module: ModuleName,
    /// The file it was read from, absolute; `None` for an image's module
    /// and the embedded manifest.
    pub(crate) path: Option<PathBuf>,
    /// The file's `file_identity`, made once as the module is loaded.
    pub(crate) identity: Option<PathBuf>,
    /// Its text as loaded; `None` for a module of an image without source.
    pub(crate) text: Option<String>,
}

/// A file of the core library the client shows.
struct LibraryFile {
    /// The module it holds: a shipped one, or the program's manifest.
    module: ModuleName,
    /// Its path, as the agent shows it.
    path: PathBuf,
    /// Its `file_identity`.
    identity: PathBuf,
    /// Its text, the binary's.
    text: &'static str,
}

/// A module's current text as the VM last answered it.
struct CurrentText {
    revision: u64,
    text: String,
    lines: LineIndex,
}

pub(crate) struct Sources {
    program: Vec<ProgramModule>,
    /// The core library's files, once a client handed over a directory of
    /// the binary's own shipped sources.
    library: Vec<LibraryFile>,
    /// Every module's current text the VM answered: kept only beside a
    /// `ModuleTextAnswer::Text`, as the revision's contract asks.
    texts: BTreeMap<ModuleName, CurrentText>,
    /// The stop generation each module's text was last asked at: a text
    /// asked at the current stop is current, since nothing installs while
    /// the program stands.
    checked: BTreeMap<ModuleName, u64>,
    /// The modules whose `ModuleText` the VM has not answered yet.
    asked: BTreeMap<ModuleName, u64>,
    /// `sourceReference` ids, stable per module for the session: the id is
    /// the module's index here plus 1.
    references: Vec<ModuleName>,
    /// How many `ModuleTextAnswer::Text` answers the VM sent.
    text_answers: usize,
}

impl Sources {
    pub(crate) fn new(program: Vec<ProgramModule>) -> Sources {
        Sources {
            program,
            library: Vec::new(),
            texts: BTreeMap::new(),
            checked: BTreeMap::new(),
            asked: BTreeMap::new(),
            references: Vec::new(),
            text_answers: 0,
        }
    }

    /// Takes `directory` for the core library the client shows: the
    /// binary's shipped sources in `core-library/`'s layout, as `ns-next
    /// shipped-sources` writes them. Refused, with the reason, unless its
    /// fingerprint is the binary's own, since only then is every file's
    /// text the binary's and a line in it a line of the module that runs.
    pub(crate) fn take_library(&mut self, directory: &Path) -> Result<(), String> {
        let fingerprint =
            std::fs::read_to_string(directory.join(FINGERPRINT_FILE)).map_err(|error| {
                format!(
                    "the core library at {} is not used: its fingerprint cannot be read: {error}",
                    directory.display()
                )
            })?;
        let own = ns_runtime::shipped_sources_fingerprint();
        if fingerprint.trim() != own {
            return Err(format!(
                "the core library at {} is not used: its fingerprint {} is not this binary's {own}",
                directory.display(),
                fingerprint.trim()
            ));
        }
        self.library = ns_runtime::shipped_source_files()
            .into_iter()
            .map(|file| {
                let path = file
                    .path
                    .split('/')
                    .fold(directory.to_path_buf(), |path, part| path.join(part));
                LibraryFile {
                    module: ModuleName::named(ns_runtime::shipped_module_id(file.name)),
                    identity: file_identity(&path),
                    path,
                    text: file.text,
                }
            })
            .collect();
        Ok(())
    }

    /// Whether `module`'s text has to be asked of the VM before a reader
    /// at the stop `generation` reads it: never a shipped module's, which
    /// is taken from the binary the first time it is read.
    pub(crate) fn needs_text(&mut self, module: &ModuleName, generation: u64) -> bool {
        if let Some(text) = shipped(module) {
            if !self.texts.contains_key(module) {
                let lines = LineIndex::new(text);
                self.texts.insert(
                    module.clone(),
                    CurrentText {
                        revision: 0,
                        text: text.to_string(),
                        lines,
                    },
                );
            }
            return false;
        }
        self.checked.get(module) != Some(&generation)
    }

    /// Records that `module`'s text is asked at the stop `generation`, and
    /// answers the revision the ask names; `None` when an ask is on its
    /// way already, whose answer the reader waits for.
    pub(crate) fn ask(&mut self, module: &ModuleName, generation: u64) -> Option<Option<u64>> {
        if self.asked.contains_key(module) {
            return None;
        }
        self.asked.insert(module.clone(), generation);
        Some(self.texts.get(module).map(|current| current.revision))
    }

    /// The VM's answer to the ask of `module`'s text.
    pub(crate) fn answered(
        &mut self,
        module: &ModuleName,
        revision: u64,
        answer: ModuleTextAnswer,
    ) {
        let Some(generation) = self.asked.remove(module) else {
            return;
        };
        self.checked.insert(module.clone(), generation);
        match answer {
            ModuleTextAnswer::Unchanged => {}
            ModuleTextAnswer::Text(text) => {
                self.text_answers += 1;
                let lines = LineIndex::new(&text);
                self.texts.insert(
                    module.clone(),
                    CurrentText {
                        revision,
                        text,
                        lines,
                    },
                );
            }
            ModuleTextAnswer::None => {
                self.texts.remove(module);
            }
        }
    }

    /// The VM refused the ask of `module`'s text -- the program went on
    /// meanwhile: the reader goes on with the text it has.
    pub(crate) fn refused(&mut self, module: &ModuleName) {
        if let Some(generation) = self.asked.remove(module) {
            self.checked.insert(module.clone(), generation);
        }
    }

    pub(crate) fn text_answers(&self) -> usize {
        self.text_answers
    }

    /// `module`'s current text, as the VM last answered it.
    pub(crate) fn text(&self, module: &ModuleName) -> Option<&str> {
        self.texts.get(module).map(|current| current.text.as_str())
    }

    /// The text breakpoints' lines are counted in: the current text the VM
    /// last answered, or else the program's as loaded, or a shipped
    /// module's own.
    pub(crate) fn text_for_positions(&self, module: &ModuleName) -> Option<&str> {
        self.text(module)
            .or_else(|| {
                self.program_module(module)
                    .and_then(|program| program.text.as_deref())
            })
            .or_else(|| shipped(module))
    }

    /// Where `span` of `module`'s current text starts and ends.
    pub(crate) fn range(
        &self,
        module: &ModuleName,
        span: Span,
    ) -> Option<(LineColumn, LineColumn)> {
        let current = self.texts.get(module)?;
        Some((
            current.lines.position(&current.text, span.start),
            current.lines.position(&current.text, span.end),
        ))
    }

    /// How a client finds `module`'s text: by the path of the file it was
    /// read from while its current text is the file's -- a program's own
    /// file, or the core library's file of a shipped module or of the
    /// manifest -- and by a reference otherwise: a shipped module without
    /// a core library, an image's module, or one an install changed.
    pub(crate) fn source(&mut self, module: &ModuleName) -> protocol::Source {
        let name = file_name(module);
        let current = self.text(module);
        let file = self
            .program_module(module)
            .and_then(|program| {
                let path = program.path.as_ref()?;
                let unchanged = program.text.is_some() && program.text.as_deref() == current;
                unchanged.then(|| path.display().to_string())
            })
            .or_else(|| {
                let library = self.library_file_of(module)?;
                (current == Some(library.text)).then(|| library.path.display().to_string())
            });
        match file {
            Some(path) => protocol::Source {
                name,
                path: Some(path),
                source_reference: None,
                presentation_hint: None,
            },
            None => protocol::Source {
                name,
                path: None,
                source_reference: Some(self.reference_of(module)),
                presentation_hint: None,
            },
        }
    }

    /// `module`'s reference, the same for the whole session.
    fn reference_of(&mut self, module: &ModuleName) -> i64 {
        let index = match self.references.iter().position(|named| named == module) {
            Some(index) => index,
            None => {
                self.references.push(module.clone());
                self.references.len() - 1
            }
        };
        i64::try_from(index + 1).unwrap_or(i64::MAX)
    }

    /// The module the reference `reference` names.
    pub(crate) fn module_of_reference(&self, reference: i64) -> Option<&ModuleName> {
        let index = usize::try_from(reference.checked_sub(1)?).ok()?;
        self.references.get(index)
    }

    /// The module a client's source names: by its reference; by its path
    /// -- a program module's file, or a file of the core library taken,
    /// which names its shipped module and never a program module of the
    /// same name; or by its name, `<Class>.ns`, only for a program module
    /// that has no file of its own -- an image's, the embedded manifest --
    /// or one a stop showed by reference that is no shipped module, and
    /// only while that name is one such module's alone: two versions of a
    /// module are told apart by their references.
    pub(crate) fn module_named_by(&self, source: &SourceArgument) -> Option<ModuleName> {
        if let Some(reference) = source.source_reference.filter(|reference| *reference > 0) {
            return self.module_of_reference(reference).cloned();
        }
        if let Some(path) = &source.path {
            let named = file_identity(Path::new(path));
            let program = self.program.iter().find(|program| {
                program
                    .identity
                    .as_deref()
                    .is_some_and(|identity| same_identity(identity, &named))
            });
            if let Some(program) = program {
                return Some(program.module.clone());
            }
            return self
                .library
                .iter()
                .find(|file| same_identity(&file.identity, &named))
                .map(|file| file.module.clone());
        }
        let name = source.name.as_deref()?;
        let without_file = self
            .program
            .iter()
            .filter(|program| program.path.is_none())
            .map(|program| &program.module)
            .chain(
                self.texts
                    .keys()
                    .chain(&self.references)
                    .filter(|module| self.program_module(module).is_none()),
            )
            .filter(|module| shipped(module).is_none() && file_name(module) == name);
        let mut named: Vec<&ModuleName> = Vec::new();
        for module in without_file {
            if !named.contains(&module) {
                named.push(module);
            }
        }
        match named[..] {
            [only] => Some(only.clone()),
            _ => None,
        }
    }

    /// The module the program or the binary has -- a program's module,
    /// the embedded manifest included, or a shipped one -- that `text`
    /// names, as a class name a client typed is read against
    /// (`ns_runtime::tool_names::parse_class_name`), with the versions
    /// the program has of its id: `text` itself where it is such an id,
    /// and otherwise,
    /// for an undotted `text`, the one such module whose class `text`
    /// names -- `newspeak.core.Kernel` for `Kernel` where the program has
    /// no `Kernel` -- so that a name in the form before ids,
    /// `Kernel.Array`, is refused with the form it means.
    pub(crate) fn module_of(&self, text: &str) -> Option<KnownModule> {
        let ids: Vec<String> = self
            .program
            .iter()
            .map(|program| program.module.id.clone())
            .chain(
                ns_runtime::shipped_source_files()
                    .iter()
                    .map(|file| ns_runtime::shipped_module_id(file.name)),
            )
            .collect();
        let id = if ids.iter().any(|id| id == text) {
            text.to_string()
        } else if text.contains('.') {
            return None;
        } else {
            let mut by_class = ids.iter().filter(|id| id.rsplit('.').next() == Some(text));
            let first = by_class.next()?;
            if !by_class.all(|other| other == first) {
                return None;
            }
            first.clone()
        };
        let versions = self
            .program
            .iter()
            .filter(|program| program.module.id == id)
            .filter_map(|program| program.module.version.clone())
            .collect();
        Some(KnownModule { id, versions })
    }

    /// Whether `module`'s current text declares the class `path` names
    /// from its root down, the root's own name left out: `None` when the
    /// agent knows no text of the module, or the text does not parse, so
    /// that the VM, which filters by the declarations it runs, decides.
    pub(crate) fn declares_class(&self, module: &ModuleName, path: &[String]) -> Option<bool> {
        let text = match self.texts.get(module) {
            Some(current) => current.text.as_str(),
            None => self
                .program_module(module)
                .and_then(|program| program.text.as_deref())
                .or_else(|| self.library_file_of(module).map(|file| file.text))
                .or_else(|| shipped(module))?,
        };
        let parsed = ns_syntax::parse(text).ok()?;
        let mut declaration = &parsed.root_class;
        for segment in path {
            let nested = declaration
                .instance_side
                .members
                .iter()
                .find_map(|member| match member {
                    ns_syntax::ast::Member::Class(class) if class.name.name == *segment => {
                        Some(&**class)
                    }
                    _ => None,
                });
            match nested {
                Some(class) => declaration = class,
                None => return Some(false),
            }
        }
        Some(true)
    }

    fn program_module(&self, module: &ModuleName) -> Option<&ProgramModule> {
        self.program
            .iter()
            .find(|program| program.module == *module)
    }

    fn library_file_of(&self, module: &ModuleName) -> Option<&LibraryFile> {
        self.library.iter().find(|file| file.module == *module)
    }
}

/// The text of the shipped module `module` names, by its id, which no
/// program's module may have (`docs/design/modules.md`, decision 9);
/// `None` for a program's. The text the agent shows, never what decides
/// whether a frame is shipped, which the VM says (`FrameDescription::shipped`).
fn shipped(module: &ModuleName) -> Option<&'static str> {
    if module.version.is_some() {
        return None;
    }
    ns_runtime::shipped_module_source(ns_runtime::shipped_module_named_by_id(&module.id)?)
}

/// The name a client shows `module`'s source by: its class's file,
/// `<Class>.ns`, the last segment of its id.
fn file_name(module: &ModuleName) -> String {
    let class = module.id.rsplit('.').next().unwrap_or(&module.id);
    format!("{class}.ns")
}

/// What a path is compared by to tell whether it names a module's file:
/// its canonical form, every `..` and symbolic link resolved. The two
/// sides are spelled independently -- a module's path as the program
/// wrote it, `../shared/Tree.ns` in a project file or a link the program
/// was started through, and the client's as its editor resolved the file
/// -- so only the file system can tell they name one file. A path it
/// cannot resolve is made absolute and its `.` and `..` resolved by their
/// spelling.
pub(crate) fn file_identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        lexically_normal(&absolute)
    })
}

/// `path` with its `.` components dropped and each `..` taking away the
/// component before it.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(
                    normal.components().next_back(),
                    Some(std::path::Component::Normal(_))
                ) {
                    normal.pop();
                } else if !normal.has_root() {
                    normal.push(component);
                }
            }
            other => normal.push(other),
        }
    }
    normal
}

/// Whether two `file_identity`s are one file's: on Windows without regard
/// to ASCII case, as its file names ignore it, and without the verbatim
/// prefix, since a canonical path has it and a path that no longer
/// resolves -- a module's file deleted after the program read it -- has
/// not.
fn same_identity(first: &Path, second: &Path) -> bool {
    if cfg!(windows) {
        let (first, second) = (first.to_string_lossy(), second.to_string_lossy());
        without_verbatim_prefix(&first).eq_ignore_ascii_case(&without_verbatim_prefix(&second))
    } else {
        first == second
    }
}

/// A Windows path without its verbatim prefix: `\\?\C:\x` as `C:\x`, and
/// `\\?\UNC\server\share` as `\\server\share`.
fn without_verbatim_prefix(path: &str) -> std::borrow::Cow<'_, str> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        std::borrow::Cow::Owned(format!(r"\\{rest}"))
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        std::borrow::Cow::Borrowed(rest)
    } else {
        std::borrow::Cow::Borrowed(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Sources {
        Sources::new(vec![ProgramModule {
            module: ModuleName::named("Hello"),
            path: Some(PathBuf::from("/programs/Hello.ns")),
            identity: Some(file_identity(Path::new("/programs/Hello.ns"))),
            text: Some("class Hello = ()".to_string()),
        }])
    }

    fn hello() -> ModuleName {
        ModuleName::named("Hello")
    }

    fn kernel() -> ModuleName {
        ModuleName::named("newspeak.core.Kernel")
    }

    #[cfg(windows)]
    #[test]
    fn a_canonical_and_a_spelled_path_of_one_file_agree() {
        assert!(same_identity(
            Path::new(r"\\?\C:\programs\Hello.ns"),
            Path::new(r"c:\Programs\Hello.ns")
        ));
        assert!(same_identity(
            Path::new(r"\\?\UNC\server\share\Hello.ns"),
            Path::new(r"\\server\share\Hello.ns")
        ));
        assert!(!same_identity(
            Path::new(r"\\?\C:\programs\Hello.ns"),
            Path::new(r"C:\programs\Other.ns")
        ));
        assert_eq!(without_verbatim_prefix(r"C:\x"), r"C:\x");
    }

    /// A module's file deleted after the program read it is still named
    /// by its path: its identity was made canonical, the client's path
    /// now resolves by its spelling only. Windows' only: elsewhere the two
    /// forms differ only where a link was resolved, which the spelling
    /// cannot undo.
    #[cfg(windows)]
    #[test]
    fn a_module_whose_file_is_gone_is_still_named_by_its_path() {
        // The repository's own scratch directory: the system's may be
        // spelled with short names, which only the file system resolves.
        let directory = scratch("identity");
        let file = directory.join("Gone.ns");
        std::fs::write(&file, "class Gone = ()").expect("the file");
        let sources = Sources::new(vec![ProgramModule {
            module: ModuleName::named("Gone"),
            path: Some(file.clone()),
            identity: Some(file_identity(&file)),
            text: Some("class Gone = ()".to_string()),
        }]);
        std::fs::remove_dir_all(&directory).expect("remove the directory");
        let named = sources.module_named_by(&SourceArgument {
            path: Some(file.display().to_string()),
            ..SourceArgument::default()
        });
        assert_eq!(named, Some(ModuleName::named("Gone")));
    }

    /// A directory of this test's own under the repository's scratch
    /// directory, created afresh.
    fn scratch(name: &str) -> PathBuf {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.temp")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        directory
    }

    #[test]
    fn a_path_through_dot_dot_names_the_module_its_file_is() {
        let sources = sources();
        let named = sources.module_named_by(&SourceArgument {
            path: Some("/programs/lib/./../Hello.ns".to_string()),
            ..SourceArgument::default()
        });
        assert_eq!(named, Some(hello()));
        assert_eq!(
            lexically_normal(Path::new("/a/b/../../../c/./d")),
            PathBuf::from("/c/d")
        );
        assert_eq!(
            lexically_normal(Path::new("../a/../b")),
            PathBuf::from("../b")
        );
    }

    #[test]
    fn a_program_module_is_shown_by_its_path_while_its_text_is_the_file_s() {
        let mut sources = sources();
        assert_eq!(sources.ask(&hello(), 1), Some(None));
        sources.answered(
            &hello(),
            4,
            ModuleTextAnswer::Text("class Hello = ()".to_string()),
        );
        let source = sources.source(&hello());
        assert_eq!(source.path.as_deref(), Some("/programs/Hello.ns"));
        assert_eq!(source.source_reference, None);

        assert_eq!(sources.ask(&hello(), 2), Some(Some(4)));
        sources.answered(
            &hello(),
            5,
            ModuleTextAnswer::Text("class Hello = (x)".to_string()),
        );
        let source = sources.source(&hello());
        assert_eq!(source.path, None);
        assert_eq!(source.source_reference, Some(1));
        assert_eq!(sources.module_of_reference(1), Some(&hello()));
    }

    #[test]
    fn a_reference_is_stable_per_module() {
        let mut sources = sources();
        let kernel = sources.source(&kernel()).source_reference;
        let platform = sources
            .source(&ModuleName::named("newspeak.core.PlatformFactory"))
            .source_reference;
        assert_ne!(kernel, platform);
        assert_eq!(
            sources
                .source(&ModuleName::named("newspeak.core.Kernel"))
                .source_reference,
            kernel
        );
    }

    #[test]
    fn a_text_asked_at_a_stop_is_current_for_that_stop_only() {
        let mut sources = sources();
        let library = ModuleName::named("Library");
        assert!(sources.needs_text(&library, 1));
        assert_eq!(sources.ask(&library, 1), Some(None));
        assert_eq!(sources.ask(&library, 1), None, "an ask is on its way");
        sources.answered(&library, 2, ModuleTextAnswer::Text("text".to_string()));
        assert!(!sources.needs_text(&library, 1));
        assert!(sources.needs_text(&library, 2));
        assert_eq!(sources.ask(&library, 2), Some(Some(2)));
        sources.answered(&library, 2, ModuleTextAnswer::Unchanged);
        assert_eq!(sources.text(&library), Some("text"));
        assert_eq!(sources.text_answers(), 1);
    }

    /// A reference and a path name their modules; a name only a program
    /// module without a file of its own, never a shipped one nor one with
    /// a file.
    #[test]
    fn a_source_names_a_module_by_reference_path_or_a_file_less_name() {
        let mut sources = Sources::new(vec![
            ProgramModule {
                module: ModuleName::named("Hello"),
                path: Some(PathBuf::from("/programs/Hello.ns")),
                identity: Some(file_identity(Path::new("/programs/Hello.ns"))),
                text: Some("class Hello = ()".to_string()),
            },
            ProgramModule {
                module: ModuleName::named("newspeak.core.Manifest"),
                path: None,
                identity: None,
                text: Some("class Manifest = ()".to_string()),
            },
        ]);
        let reference = sources.source(&kernel()).source_reference;
        let named = |source: SourceArgument| sources.module_named_by(&source);
        let by_name = |name: &str| SourceArgument {
            name: Some(name.to_string()),
            ..SourceArgument::default()
        };
        assert_eq!(
            named(SourceArgument {
                source_reference: reference,
                ..SourceArgument::default()
            }),
            Some(kernel())
        );
        assert_eq!(
            named(SourceArgument {
                path: Some("/programs/Hello.ns".to_string()),
                ..SourceArgument::default()
            }),
            Some(hello())
        );
        assert_eq!(
            named(by_name("Manifest.ns")),
            Some(ModuleName::named("newspeak.core.Manifest"))
        );
        assert_eq!(named(by_name("Hello.ns")), None, "it has a file");
        assert_eq!(named(by_name("Kernel.ns")), None, "a shipped module");
        assert_eq!(named(by_name("Elsewhere.ns")), None);
    }

    #[test]
    fn a_shipped_module_s_text_is_the_binary_s_and_never_asked() {
        let mut sources = sources();
        assert!(!sources.needs_text(&kernel(), 1));
        assert_eq!(
            sources.text(&kernel()),
            ns_runtime::shipped_module_source("Kernel")
        );
        assert!(
            sources
                .text_for_positions(&ModuleName::named("newspeak.core.PlatformFactory"))
                .is_some()
        );
        assert_eq!(sources.text_answers(), 0);
    }

    /// A program module named like a shipped one is another module: its
    /// text is its own, the shipped module's the binary's, and each is
    /// known by its own id.
    #[test]
    fn a_program_module_named_like_a_shipped_one_is_another_module() {
        let mut sources = Sources::new(vec![ProgramModule {
            module: ModuleName::named("Kernel"),
            path: Some(PathBuf::from("/programs/Kernel.ns")),
            identity: Some(file_identity(Path::new("/programs/Kernel.ns"))),
            text: Some("class Kernel = ()".to_string()),
        }]);
        let own = ModuleName::named("Kernel");
        assert!(sources.needs_text(&own, 1));
        assert_eq!(sources.text_for_positions(&own), Some("class Kernel = ()"));
        assert!(!sources.needs_text(&kernel(), 1));
        assert_eq!(
            sources.text_for_positions(&kernel()),
            ns_runtime::shipped_module_source("Kernel")
        );
        let id_of = |text: &str| sources.module_of(text).map(|known| known.id);
        assert_eq!(id_of("Kernel"), Some("Kernel".to_string()));
        assert_eq!(
            id_of("newspeak.core.Kernel"),
            Some("newspeak.core.Kernel".to_string())
        );
        assert_eq!(
            id_of("CollectionsFactory"),
            Some("newspeak.core.CollectionsFactory".to_string()),
            "a shipped module by its class's name"
        );
        assert_eq!(id_of("Kernel.Array"), None);
        assert_eq!(id_of("Elsewhere"), None);
    }

    /// Two versions of a module without files of their own -- an
    /// image's -- share their file's name, so a name names neither: each
    /// is named by its reference.
    #[test]
    fn two_versions_of_a_file_less_module_are_not_named_by_their_name() {
        let version = |version: &str| ModuleName {
            id: "com.acme.util.Util".to_string(),
            version: Some(version.to_string()),
        };
        let mut sources = Sources::new(
            ["1.0.0", "2.0.0"]
                .map(|number| ProgramModule {
                    module: version(number),
                    path: None,
                    identity: None,
                    text: Some("class Util = ()".to_string()),
                })
                .into(),
        );
        let first = sources.source(&version("1.0.0"));
        let second = sources.source(&version("2.0.0"));
        assert_eq!(first.name, "Util.ns");
        assert_ne!(first.source_reference, second.source_reference);
        let named = sources.module_named_by(&SourceArgument {
            name: Some("Util.ns".to_string()),
            ..SourceArgument::default()
        });
        assert_eq!(named, None);
        let by_reference = sources.module_named_by(&SourceArgument {
            source_reference: second.source_reference,
            ..SourceArgument::default()
        });
        assert_eq!(by_reference, Some(version("2.0.0")));
    }

    /// A core library of the binary's fingerprint names its shipped
    /// modules by their files' paths, and shows them by those paths; one
    /// of another fingerprint is refused.
    #[test]
    fn a_core_library_names_and_shows_the_shipped_modules_by_their_files() {
        let directory = scratch("core-library");
        for file in ns_runtime::shipped_source_files() {
            let path = directory.join(file.path);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
            std::fs::write(&path, file.text).expect("a file");
        }
        let fingerprint = directory.join(FINGERPRINT_FILE);
        std::fs::write(&fingerprint, "0000000000000000\n").expect("the fingerprint");
        let mut sources = sources();
        let refused = sources.take_library(&directory);
        assert!(
            refused
                .as_ref()
                .is_err_and(|reason| reason.contains("is not used")),
            "another fingerprint is taken"
        );
        std::fs::write(
            &fingerprint,
            format!("{}\n", ns_runtime::shipped_sources_fingerprint()),
        )
        .expect("the fingerprint");
        assert!(sources.take_library(&directory).is_ok());

        let kernel_file = directory.join("Kernel.ns");
        let by_path = |path: &Path| SourceArgument {
            path: Some(path.display().to_string()),
            ..SourceArgument::default()
        };
        assert_eq!(
            sources.module_named_by(&by_path(&kernel_file)),
            Some(kernel())
        );
        assert_eq!(
            sources.module_named_by(&by_path(&directory.join("Manifest.ns"))),
            Some(ModuleName::named("newspeak.core.Manifest"))
        );
        sources.needs_text(&kernel(), 1);
        let shown = sources.source(&kernel());
        assert_eq!(shown.path, Some(kernel_file.display().to_string()));
        assert_eq!(shown.source_reference, None);
        std::fs::remove_dir_all(&directory).expect("remove the directory");
    }
}
