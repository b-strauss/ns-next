//! The modules a stop shows: the program's own, which the host hands over
//! with their files and texts, and every module the VM names, a shipped
//! one included. A module's current text comes from the VM
//! (`Command::ModuleText`) and is kept beside its revision, so that a stop
//! asks again only whether it changed; a module whose current text is its
//! file's as the program was loaded is shown by its path, every other by a
//! `sourceReference` the `source` request answers. A shipped module's text
//! is the binary's own and never changes, so it is never asked of the VM.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ns_runtime::ModuleTextAnswer;
use ns_syntax::Span;
use ns_tooling::lines::{LineColumn, LineIndex};

use super::protocol::{self, SourceArgument};

/// A module of the program as the host loaded it.
pub(crate) struct ProgramModule {
    /// Its root class's name, as `FrameLocation::module` names it.
    pub(crate) name: String,
    /// The file it was read from, absolute; `None` for an image's module.
    pub(crate) path: Option<PathBuf>,
    /// Its text as loaded; `None` for a module of an image without source.
    pub(crate) text: Option<String>,
}

/// A module's current text as the VM last answered it.
struct CurrentText {
    revision: u64,
    text: String,
    lines: LineIndex,
}

pub(crate) struct Sources {
    program: Vec<ProgramModule>,
    /// Every module's current text the VM answered: kept only beside a
    /// `ModuleTextAnswer::Text`, as the revision's contract asks.
    texts: BTreeMap<String, CurrentText>,
    /// The stop generation each module's text was last asked at: a text
    /// asked at the current stop is current, since nothing installs while
    /// the program stands.
    checked: BTreeMap<String, u64>,
    /// The modules whose `ModuleText` the VM has not answered yet.
    asked: BTreeMap<String, u64>,
    /// `sourceReference` ids, stable per module for the session: the id is
    /// the module's index here plus 1.
    references: Vec<String>,
    /// How many `ModuleTextAnswer::Text` answers the VM sent.
    text_answers: usize,
}

impl Sources {
    pub(crate) fn new(program: Vec<ProgramModule>) -> Sources {
        Sources {
            program,
            texts: BTreeMap::new(),
            checked: BTreeMap::new(),
            asked: BTreeMap::new(),
            references: Vec::new(),
            text_answers: 0,
        }
    }

    /// Whether `module`'s text has to be asked of the VM before a reader
    /// at the stop `generation` reads it: never a shipped module's, which
    /// is taken from the binary the first time it is read.
    pub(crate) fn needs_text(&mut self, module: &str, generation: u64) -> bool {
        if let Some(text) = self.shipped(module) {
            if !self.texts.contains_key(module) {
                let lines = LineIndex::new(text);
                self.texts.insert(
                    module.to_string(),
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

    /// The text of the shipped module `module`, unless a module of the
    /// program has its name, which a frame's module then names.
    fn shipped(&self, module: &str) -> Option<&'static str> {
        if self.program_module(module).is_some() {
            return None;
        }
        ns_runtime::shipped_module_source(module)
    }

    /// Records that `module`'s text is asked at the stop `generation`, and
    /// answers the revision the ask names; `None` when an ask is on its
    /// way already, whose answer the reader waits for.
    pub(crate) fn ask(&mut self, module: &str, generation: u64) -> Option<Option<u64>> {
        if self.asked.contains_key(module) {
            return None;
        }
        self.asked.insert(module.to_string(), generation);
        Some(self.texts.get(module).map(|current| current.revision))
    }

    /// The VM's answer to the ask of `module`'s text.
    pub(crate) fn answered(&mut self, module: &str, revision: u64, answer: ModuleTextAnswer) {
        let Some(generation) = self.asked.remove(module) else {
            return;
        };
        self.checked.insert(module.to_string(), generation);
        match answer {
            ModuleTextAnswer::Unchanged => {}
            ModuleTextAnswer::Text(text) => {
                self.text_answers += 1;
                let lines = LineIndex::new(&text);
                self.texts.insert(
                    module.to_string(),
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
    pub(crate) fn refused(&mut self, module: &str) {
        if let Some(generation) = self.asked.remove(module) {
            self.checked.insert(module.to_string(), generation);
        }
    }

    pub(crate) fn text_answers(&self) -> usize {
        self.text_answers
    }

    /// `module`'s current text, as the VM last answered it.
    pub(crate) fn text(&self, module: &str) -> Option<&str> {
        self.texts.get(module).map(|current| current.text.as_str())
    }

    /// The text breakpoints' lines are counted in: the current text the VM
    /// last answered, or else the program's as loaded, or a shipped
    /// module's own.
    pub(crate) fn text_for_positions(&self, module: &str) -> Option<&str> {
        self.text(module)
            .or_else(|| {
                self.program_module(module)
                    .and_then(|program| program.text.as_deref())
            })
            .or_else(|| self.shipped(module))
    }

    /// Where `span` of `module`'s current text starts and ends.
    pub(crate) fn range(&self, module: &str, span: Span) -> Option<(LineColumn, LineColumn)> {
        let current = self.texts.get(module)?;
        Some((
            current.lines.position(&current.text, span.start),
            current.lines.position(&current.text, span.end),
        ))
    }

    /// How a client finds `module`'s text: by the path of the file it was
    /// read from while its current text is the file's, by a reference
    /// otherwise -- a shipped module, an image's, or one an install changed.
    pub(crate) fn source(&mut self, module: &str) -> protocol::Source {
        let name = format!("{module}.ns");
        let file = self.program_module(module).and_then(|program| {
            let path = program.path.as_ref()?;
            let unchanged = program.text.is_some() && program.text.as_deref() == self.text(module);
            unchanged.then(|| path.display().to_string())
        });
        match file {
            Some(path) => protocol::Source {
                name,
                path: Some(path),
                source_reference: None,
            },
            None => protocol::Source {
                name,
                path: None,
                source_reference: Some(self.reference_of(module)),
            },
        }
    }

    /// `module`'s reference, the same for the whole session.
    fn reference_of(&mut self, module: &str) -> i64 {
        let index = match self.references.iter().position(|named| named == module) {
            Some(index) => index,
            None => {
                self.references.push(module.to_string());
                self.references.len() - 1
            }
        };
        i64::try_from(index + 1).unwrap_or(i64::MAX)
    }

    /// The module the reference `reference` names.
    pub(crate) fn module_of_reference(&self, reference: i64) -> Option<&str> {
        let index = usize::try_from(reference.checked_sub(1)?).ok()?;
        self.references.get(index).map(String::as_str)
    }

    /// The module a client's source names: by its reference, by the path
    /// of a program's file, or by its name, `<Module>.ns`, for a module
    /// of the program, a shipped one, or one a stop showed.
    pub(crate) fn module_named_by(&self, source: &SourceArgument) -> Option<String> {
        if let Some(reference) = source.source_reference.filter(|reference| *reference > 0) {
            return self.module_of_reference(reference).map(str::to_string);
        }
        if let Some(path) = &source.path {
            return self
                .program
                .iter()
                .find(|program| {
                    program
                        .path
                        .as_deref()
                        .is_some_and(|file| same_file(file, Path::new(path)))
                })
                .map(|program| program.name.clone());
        }
        let module = source.name.as_deref()?.strip_suffix(".ns")?;
        let known = self.program_module(module).is_some()
            || self.shipped(module).is_some()
            || self.texts.contains_key(module)
            || self.references.iter().any(|named| named == module);
        known.then(|| module.to_string())
    }

    fn program_module(&self, module: &str) -> Option<&ProgramModule> {
        self.program.iter().find(|program| program.name == module)
    }
}

/// Whether two paths name one file: compared as absolute paths, on
/// Windows without regard to ASCII case, as its file names ignore it, and
/// without resolving `..` or a symbolic link. That suffices: both come
/// from the program's own reading -- the host's, made absolute, and the
/// path a client took from a frame's source or from the file the program
/// was started with -- not from two independent spellings of one file.
fn same_file(first: &Path, second: &Path) -> bool {
    let absolute = |path: &Path| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let (first, second) = (absolute(first), absolute(second));
    if cfg!(windows) {
        first
            .to_string_lossy()
            .eq_ignore_ascii_case(&second.to_string_lossy())
    } else {
        first == second
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Sources {
        Sources::new(vec![ProgramModule {
            name: "Hello".to_string(),
            path: Some(PathBuf::from("/programs/Hello.ns")),
            text: Some("class Hello = ()".to_string()),
        }])
    }

    #[test]
    fn a_program_module_is_shown_by_its_path_while_its_text_is_the_file_s() {
        let mut sources = sources();
        assert_eq!(sources.ask("Hello", 1), Some(None));
        sources.answered(
            "Hello",
            4,
            ModuleTextAnswer::Text("class Hello = ()".to_string()),
        );
        let source = sources.source("Hello");
        assert_eq!(source.path.as_deref(), Some("/programs/Hello.ns"));
        assert_eq!(source.source_reference, None);

        assert_eq!(sources.ask("Hello", 2), Some(Some(4)));
        sources.answered(
            "Hello",
            5,
            ModuleTextAnswer::Text("class Hello = (x)".to_string()),
        );
        let source = sources.source("Hello");
        assert_eq!(source.path, None);
        assert_eq!(source.source_reference, Some(1));
        assert_eq!(sources.module_of_reference(1), Some("Hello"));
    }

    #[test]
    fn a_reference_is_stable_per_module() {
        let mut sources = sources();
        let kernel = sources.source("Kernel").source_reference;
        let platform = sources.source("PlatformFactory").source_reference;
        assert_ne!(kernel, platform);
        assert_eq!(sources.source("Kernel").source_reference, kernel);
    }

    #[test]
    fn a_text_asked_at_a_stop_is_current_for_that_stop_only() {
        let mut sources = sources();
        assert!(sources.needs_text("Library", 1));
        assert_eq!(sources.ask("Library", 1), Some(None));
        assert_eq!(sources.ask("Library", 1), None, "an ask is on its way");
        sources.answered("Library", 2, ModuleTextAnswer::Text("text".to_string()));
        assert!(!sources.needs_text("Library", 1));
        assert!(sources.needs_text("Library", 2));
        assert_eq!(sources.ask("Library", 2), Some(Some(2)));
        sources.answered("Library", 2, ModuleTextAnswer::Unchanged);
        assert_eq!(sources.text("Library"), Some("text"));
        assert_eq!(sources.text_answers(), 1);
    }

    #[test]
    fn a_source_names_a_module_by_reference_path_or_name() {
        let mut sources = sources();
        let reference = sources.source("Kernel").source_reference;
        let named = |source: SourceArgument| sources.module_named_by(&source);
        assert_eq!(
            named(SourceArgument {
                source_reference: reference,
                ..SourceArgument::default()
            })
            .as_deref(),
            Some("Kernel")
        );
        assert_eq!(
            named(SourceArgument {
                path: Some("/programs/Hello.ns".to_string()),
                ..SourceArgument::default()
            })
            .as_deref(),
            Some("Hello")
        );
        assert_eq!(
            named(SourceArgument {
                name: Some("Hello.ns".to_string()),
                ..SourceArgument::default()
            })
            .as_deref(),
            Some("Hello")
        );
        assert_eq!(
            named(SourceArgument {
                name: Some("Elsewhere.ns".to_string()),
                ..SourceArgument::default()
            }),
            None
        );
    }

    #[test]
    fn a_shipped_module_s_text_is_the_binary_s_and_never_asked() {
        let mut sources = sources();
        assert!(!sources.needs_text("Kernel", 1));
        assert_eq!(
            sources.text("Kernel"),
            ns_runtime::shipped_module_source("Kernel")
        );
        assert!(sources.text_for_positions("PlatformFactory").is_some());
        let named = sources.module_named_by(&SourceArgument {
            name: Some("Kernel.ns".to_string()),
            ..SourceArgument::default()
        });
        assert_eq!(named.as_deref(), Some("Kernel"));
        assert_eq!(sources.text_answers(), 0);
    }
}
