//! The project file: a program of several modules, named in JSON. `main` is
//! the application module's source; `modules` are the other modules its
//! manifest holds, each a path or a path with the id `as` gives it
//! (`docs/design/image-format.md`, "Packaging";
//! `docs/design/modules.md`, decision 5). Every
//! project file is named `newspeak.json`, so a project is its directory. Its
//! schema is `schemas/newspeak.schema.json`, and `read_project` enforces the
//! same rules with messages of its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::command_line::split_command_line;

/// The one name a project file has, as every Node package's is
/// `package.json`: a command given a directory finds the project in it, and
/// no other JSON file is mistaken for one.
pub(crate) const PROJECT_FILE_NAME: &str = "newspeak.json";

/// What a command was given to run, once found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProgramPath {
    /// One source file, whose root is instantiated with `new`.
    Source(PathBuf),
    /// A project file, whose root is instantiated with `packageUsing:`.
    Project(PathBuf),
}

/// The program `path` names: a directory's `newspeak.json`, a file of that
/// name, or a source file. A directory without one, and a JSON file of any
/// other name, are refused with the name a project file must have rather
/// than read as a source.
pub(crate) fn locate_program(path: &Path) -> Result<ProgramPath, String> {
    ns_runtime::reach::record(ns_runtime::reach::Reach::Platform);
    if path.is_dir() {
        let project = path.join(PROJECT_FILE_NAME);
        if project.is_file() {
            return Ok(ProgramPath::Project(project));
        }
        return Err(format!(
            "{} holds no {PROJECT_FILE_NAME}: a directory is run as the project its \
             {PROJECT_FILE_NAME} describes",
            path.display()
        ));
    }
    if path
        .file_name()
        .is_some_and(|name| name == PROJECT_FILE_NAME)
    {
        return Ok(ProgramPath::Project(path.to_path_buf()));
    }
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Err(format!(
            "{} is not a project file: a project file is named {PROJECT_FILE_NAME}",
            path.display()
        ));
    }
    Ok(ProgramPath::Source(path.to_path_buf()))
}

/// The file as written. Other keys are ignored, so that a file may carry what
/// an editor or another tool wants beside what ns-next reads.
#[derive(serde::Deserialize)]
struct ProjectFile {
    /// Where an editor finds the schema. Never read, but held to the
    /// schema's type for it, so that a file the schema refuses is not run.
    #[serde(rename = "$schema", default, deserialize_with = "a_string")]
    _schema: Option<String>,
    /// The application module's source.
    main: String,
    #[serde(default)]
    modules: Vec<ModuleEntry>,
    /// Named command lines, each already split into its arguments.
    #[serde(default, deserialize_with = "scripts_without_repeats")]
    scripts: BTreeMap<String, Vec<String>>,
}

/// The `scripts` object, refusing a name written twice: a map would keep the
/// last script silently, so `--script` would run arguments other than the
/// ones a reader of the file sees first, as a key of the file written twice
/// would. A name is also refused when it is empty or begins or ends with
/// whitespace ([`check_script_name`]). Each script is split into its
/// arguments here ([`split_command_line`]), so that one which does not split
/// refuses the file before anything runs, whichever script is asked for.
fn scripts_without_repeats<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Vec<String>>, D::Error> {
    struct ScriptsVisitor;

    impl<'de> serde::de::Visitor<'de> for ScriptsVisitor {
        type Value = BTreeMap<String, Vec<String>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an object of scripts, each a command-line string")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut access: A,
        ) -> Result<Self::Value, A::Error> {
            let mut scripts = BTreeMap::new();
            while let Some(name) = access.next_key::<String>()? {
                check_script_name(&name).map_err(serde::de::Error::custom)?;
                let ScriptText(text) = access.next_value()?;
                if scripts.contains_key(&name) {
                    return Err(serde::de::Error::custom(format!(
                        "script `{name}` is written twice"
                    )));
                }
                let arguments = split_command_line(&text).map_err(|problem| {
                    serde::de::Error::custom(format!("script `{name}` holds {problem}"))
                })?;
                scripts.insert(name, arguments);
            }
            Ok(scripts)
        }
    }

    deserializer.deserialize_map(ScriptsVisitor)
}

/// One script's text. A `String` alone would be refused, when it is no
/// string, as `expected a string`, which does not say where; this names
/// `scripts`.
struct ScriptText(String);

impl<'de> serde::Deserialize<'de> for ScriptText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ScriptTextVisitor;

        impl serde::de::Visitor<'_> for ScriptTextVisitor {
            type Value = ScriptText;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a script of `scripts`, one command-line string")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<ScriptText, E> {
                Ok(ScriptText(text.to_string()))
            }

            fn visit_string<E: serde::de::Error>(self, text: String) -> Result<ScriptText, E> {
                Ok(ScriptText(text))
            }
        }

        deserializer.deserialize_string(ScriptTextVisitor)
    }
}

/// A script's name is non-empty and neither begins nor ends with whitespace:
/// such a name is easily mistyped on a command line and invisible in a list
/// of names. The schema states the same with the pattern
/// `^\S([\s\S]*\S)?$`, so whitespace is what a JSON Schema pattern's `\s`
/// matches ([`is_pattern_whitespace`]).
fn check_script_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a script name is empty".to_string());
    }
    if name.starts_with(is_pattern_whitespace) || name.ends_with(is_pattern_whitespace) {
        return Err(format!(
            "script name `{name}` has leading or trailing whitespace"
        ));
    }
    Ok(())
}

/// Whether `c` is whitespace as a JSON Schema pattern's `\s` means it,
/// ECMA-262's: Unicode's `White_Space`, which `char::is_whitespace` tests,
/// less U+0085, which ECMA-262 does not count, and with U+FEFF, which it
/// does.
fn is_pattern_whitespace(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
}

/// A string where the key is present. `Option<String>` alone would also take
/// `null`, which the schema refuses.
fn a_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    <String as serde::Deserialize>::deserialize(deserializer).map(Some)
}

/// One module of `modules`: its source, resolved against the project's
/// directory, and the id `as` gives it in place of the one it declares.
pub(crate) struct ProjectModule {
    pub(crate) path: PathBuf,
    pub(crate) id: Option<String>,
}

/// One entry of `modules` as written: a path, or an object of `path` and,
/// optionally, `as` -- nothing else, so that a misspelt `as` refuses the
/// file rather than leaving the module its declared id.
struct ModuleEntry {
    path: String,
    id: Option<String>,
}

impl<'de> serde::Deserialize<'de> for ModuleEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ModuleEntryVisitor;

        impl<'de> serde::de::Visitor<'de> for ModuleEntryVisitor {
            type Value = ModuleEntry;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a module of `modules`: a path or an object of `path` and, optionally, `as`",
                )
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<ModuleEntry, E> {
                Ok(ModuleEntry {
                    path: text.to_string(),
                    id: None,
                })
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<ModuleEntry, A::Error> {
                let mut path: Option<String> = None;
                let mut id: Option<String> = None;
                while let Some(key) = access.next_key::<String>()? {
                    let field = match key.as_str() {
                        "path" => &mut path,
                        "as" => &mut id,
                        other => {
                            return Err(serde::de::Error::custom(format!(
                                "a module of `modules` takes `path` and `as`, not `{other}`"
                            )));
                        }
                    };
                    if field.is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "a module of `modules` names `{key}` twice"
                        )));
                    }
                    let EntryText(text) = access.next_value_seed(EntryKey(key))?;
                    *field = Some(text);
                }
                let Some(path) = path else {
                    return Err(serde::de::Error::custom(
                        "a module of `modules` without `path`",
                    ));
                };
                if let Some(id) = &id {
                    check_id_syntax(id).map_err(|error| {
                        serde::de::Error::custom(format!("`as` of `{path}`: {error}"))
                    })?;
                }
                Ok(ModuleEntry { path, id })
            }
        }

        deserializer.deserialize_any(ModuleEntryVisitor)
    }
}

/// The string under a key of a module entry.
struct EntryText(String);

/// Reads the value under the module entry's key it holds as a string, so
/// that a refusal of anything else names the key.
struct EntryKey(String);

impl<'de> serde::de::DeserializeSeed<'de> for EntryKey {
    type Value = EntryText;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<EntryText, D::Error> {
        struct EntryTextVisitor(String);

        impl serde::de::Visitor<'_> for EntryTextVisitor {
            type Value = EntryText;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "`{}` of a module of `modules`, a string", self.0)
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<EntryText, E> {
                Ok(EntryText(text.to_string()))
            }
        }

        deserializer.deserialize_string(EntryTextVisitor(self.0))
    }
}

/// Refuses an `as` that is no id by its grammar alone -- identifiers
/// separated by dots -- as the schema's pattern does; whether its last
/// part is the module's class's name waits for the module's source
/// (`program.rs`).
fn check_id_syntax(id: &str) -> Result<(), String> {
    let last = id.rsplit('.').next().unwrap_or(id);
    ns_runtime::modules::ModuleId::parse(id, last).map(|_| ())
}

pub(crate) struct Project {
    pub(crate) main: PathBuf,
    pub(crate) modules: Vec<ProjectModule>,
    /// Sorted by name, for the message that lists them.
    pub(crate) scripts: BTreeMap<String, Vec<String>>,
}

pub(crate) fn read_project(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    let in_file = |error: String| format!("{}: {error}", path.display());
    // A derived `Deserialize` also takes a struct from an array of its fields
    // in order, which the schema's `"type": "object"` refuses. The struct is
    // still read from the text, not from this value: a `Value`'s map keeps
    // the last of a key written twice, where the struct refuses it.
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| in_file(error.to_string()))?;
    if !value.is_object() {
        return Err(in_file(
            "a project file is a JSON object, of `main` and `modules`".to_string(),
        ));
    }
    let file: ProjectFile =
        serde_json::from_str(&text).map_err(|error| in_file(error.to_string()))?;
    check_module_path("main", &file.main).map_err(in_file)?;
    for entry in &file.modules {
        check_module_path("modules", &entry.path).map_err(in_file)?;
    }
    let directory = path.parent().unwrap_or(Path::new(""));
    let main = resolve_written_path(directory, &file.main);
    let mut modules: Vec<ProjectModule> = Vec::with_capacity(file.modules.len());
    for entry in file.modules {
        let written = &entry.path;
        let module = resolve_written_path(directory, written);
        if same_path(&module, &main) {
            return Err(in_file(format!(
                "`modules` names `main`'s module `{written}` again"
            )));
        }
        if modules
            .iter()
            .any(|earlier| same_path(&earlier.path, &module))
        {
            return Err(in_file(format!("`modules` names `{written}` twice")));
        }
        modules.push(ProjectModule {
            path: module,
            id: entry.id,
        });
    }
    Ok(Project {
        main,
        modules,
        scripts: file.scripts,
    })
}

/// Refuses a path that is empty or names no `.ns` file, naming the key it
/// was written under; the schema's `minLength` and `pattern` say the same.
fn check_module_path(key: &str, written: &str) -> Result<(), String> {
    if written.is_empty() {
        return Err(format!("`{key}` holds an empty path"));
    }
    if !written.ends_with(".ns") {
        return Err(format!("`{key}` names `{written}`, which is no `.ns` file"));
    }
    Ok(())
}

/// Whether two resolved paths name one file as written: `Path::components`
/// folds a doubled separator, and a `.` component is dropped here, because
/// the components keep a leading one. The file system is not asked, so a
/// symbolic link, a `..` that returns, and a difference of letter case on a
/// case-insensitive file system are not seen through; two such modules are
/// refused later, by `program.rs`'s check that no two modules share an id
/// and a version.
fn same_path(first: &Path, second: &Path) -> bool {
    ns_runtime::reach::record(ns_runtime::reach::Reach::Platform);
    fn significant(path: &Path) -> impl Iterator<Item = std::path::Component<'_>> {
        path.components()
            .filter(|component| *component != std::path::Component::CurDir)
    }
    significant(first).eq(significant(second))
}

/// A path as a project file writes it: either separator, relative to the
/// project file or absolute. Both separators are accepted everywhere so one
/// project file serves every operating system: the foreign separator is
/// rewritten to the host's and the platform's own path parsing does the rest,
/// which keeps drive letters, UNC and verbatim prefixes intact.
pub(crate) fn resolve_written_path(project_directory: &Path, written: &str) -> PathBuf {
    ns_runtime::reach::record(ns_runtime::reach::Reach::Platform);
    let (foreign, native) = if cfg!(windows) {
        ("/", "\\")
    } else {
        ("\\", "/")
    };
    let normalized = PathBuf::from(written.replace(foreign, native));
    // `/x/A.ns` on Windows has a root but is not absolute; it names the
    // current drive's root, so joining it onto the project directory would
    // replace the directory anyway.
    if normalized.is_absolute() || normalized.has_root() {
        normalized
    } else {
        project_directory.join(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("ns-next-project-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("{error}"));
        directory
    }

    fn refusal(path: &Path) -> String {
        match read_project(path) {
            Ok(_) => panic!("{} was accepted", path.display()),
            Err(error) => error,
        }
    }

    /// Writes `contents` as a project file in a fresh scratch directory and
    /// answers the reader's refusal.
    fn refusal_of(name: &str, contents: &str) -> String {
        let directory = scratch(name);
        let path = directory.join(PROJECT_FILE_NAME);
        std::fs::write(&path, contents).unwrap_or_else(|error| panic!("{error}"));
        refusal(&path)
    }

    #[test]
    fn a_path_that_is_empty_or_names_no_ns_file_is_refused_naming_its_key() {
        let error = refusal_of("empty-main", r#"{ "main": "" }"#);
        assert!(
            error.contains("`main`") && error.contains("empty"),
            "{error}"
        );
        let error = refusal_of("main-not-ns", r#"{ "main": "App.txt" }"#);
        assert!(error.contains("`main`") && error.contains(".ns"), "{error}");
        let error = refusal_of(
            "module-not-ns",
            r#"{ "main": "App.ns", "modules": ["A.ns", "B"] }"#,
        );
        assert!(
            error.contains("`modules`") && error.contains("B"),
            "{error}"
        );
    }

    #[test]
    fn a_module_named_twice_or_equal_to_main_is_refused_in_any_spelling() {
        let error = refusal_of(
            "duplicate-spellings",
            r#"{ "main": "App.ns", "modules": ["lib/A.ns", "lib\\A.ns"] }"#,
        );
        assert!(error.contains("twice"), "{error}");
        let error = refusal_of(
            "module-is-main",
            r#"{ "main": "App.ns", "modules": ["./App.ns"] }"#,
        );
        assert!(error.contains("`main`"), "{error}");
    }

    #[test]
    fn a_relative_path_is_relative_to_the_project_file_with_either_separator() {
        let directory = Path::new("projects").join("awfy");
        assert_eq!(
            resolve_written_path(&directory, "modules/Sieve.ns"),
            directory.join("modules").join("Sieve.ns")
        );
        assert_eq!(
            resolve_written_path(&directory, "modules\\Sieve.ns"),
            directory.join("modules").join("Sieve.ns")
        );
        assert_eq!(
            resolve_written_path(&directory, "../shared/Tree.ns"),
            directory.join("..").join("shared").join("Tree.ns")
        );
    }

    #[test]
    fn an_absolute_path_is_taken_as_written() {
        let directory = Path::new("projects");
        #[cfg(windows)]
        {
            assert_eq!(
                resolve_written_path(directory, "C:\\x\\A.ns"),
                PathBuf::from("C:\\x\\A.ns")
            );
            assert_eq!(
                resolve_written_path(directory, "C:/x/A.ns"),
                PathBuf::from("C:\\x\\A.ns")
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                resolve_written_path(directory, "/x/A.ns"),
                PathBuf::from("/x/A.ns")
            );
            assert_eq!(
                resolve_written_path(directory, "\\x\\A.ns"),
                PathBuf::from("/x/A.ns")
            );
        }
    }

    #[test]
    fn a_doubled_separator_names_the_same_path_as_a_single_one() {
        let directory = Path::new("projects");
        let doubled = resolve_written_path(directory, "a//b/C.ns");
        let single = resolve_written_path(directory, "a/b/C.ns");
        assert!(doubled.components().eq(single.components()));
    }

    #[cfg(windows)]
    #[test]
    fn a_unc_path_is_absolute_with_either_separator() {
        let directory = Path::new("projects");
        let expected = PathBuf::from(r"\\server\share\A.ns");
        assert!(expected.is_absolute());
        assert_eq!(
            resolve_written_path(directory, r"\\server\share\A.ns"),
            expected
        );
        assert_eq!(
            resolve_written_path(directory, "//server/share/A.ns"),
            expected
        );
    }

    #[test]
    fn a_module_is_a_path_or_an_entry_of_path_and_as() {
        let directory = scratch("module-entries");
        let path = directory.join(PROJECT_FILE_NAME);
        std::fs::write(
            &path,
            r#"{ "main": "App.ns", "modules": ["a/Util.ns", { "path": "b/Util.ns", "as": "legacy.Util" }, { "path": "C.ns" }] }"#,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let project = read_project(&path).unwrap_or_else(|error| panic!("{error}"));
        let read: Vec<(PathBuf, Option<&str>)> = project
            .modules
            .iter()
            .map(|module| (module.path.clone(), module.id.as_deref()))
            .collect();
        assert_eq!(
            read,
            [
                (resolve_written_path(&directory, "a/Util.ns"), None),
                (
                    resolve_written_path(&directory, "b/Util.ns"),
                    Some("legacy.Util")
                ),
                (resolve_written_path(&directory, "C.ns"), None),
            ]
        );
    }

    #[test]
    fn a_module_entry_is_refused_naming_what_it_lacks_or_holds_besides() {
        for (index, (entry, problem)) in [
            (r#"{ "as": "legacy.A" }"#, "without `path`"),
            (r#"{ "path": "A.ns", "x": 1 }"#, "`x`"),
            (r#"{ "path": "A.ns", "path": "B.ns" }"#, "`path` twice"),
            (r#"{ "path": "A.ns", "as": 1 }"#, "`as`"),
            (r#"{ "path": "A.ns", "as": "legacy..A" }"#, "no module id"),
            (r#"{ "path": "A.txt" }"#, "no `.ns` file"),
            (r#"["A.ns"]"#, "a path or an object"),
        ]
        .iter()
        .enumerate()
        {
            let error = refusal_of(
                &format!("module-entry-{index}"),
                &format!(r#"{{ "main": "App.ns", "modules": [{entry}] }}"#),
            );
            assert!(error.contains(problem), "{entry}: {error}");
        }
    }

    /// An entry's `path` is a module's path as a plain string is, so one
    /// module named both ways is named twice.
    #[test]
    fn a_path_named_plainly_and_in_an_entry_is_named_twice() {
        let error = refusal_of(
            "module-entry-twice",
            r#"{ "main": "App.ns", "modules": ["A.ns", { "path": "A.ns", "as": "legacy.A" }] }"#,
        );
        assert!(error.contains("`modules` names `A.ns` twice"), "{error}");
    }

    #[test]
    fn modules_may_be_absent_or_empty() {
        let directory = scratch("optional-modules");
        let absent = directory.join("absent.json");
        std::fs::write(&absent, r#"{ "main": "App.ns" }"#)
            .unwrap_or_else(|error| panic!("{error}"));
        let project = read_project(&absent).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(project.main, directory.join("App.ns"));
        assert!(project.modules.is_empty());
        let empty = directory.join("empty.json");
        std::fs::write(&empty, r#"{ "main": "App.ns", "modules": [] }"#)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            read_project(&empty)
                .unwrap_or_else(|error| panic!("{error}"))
                .modules
                .is_empty()
        );
    }

    #[test]
    fn a_file_without_main_is_refused_naming_main_whatever_else_it_holds() {
        let directory = scratch("missing-main");
        for (name, contents) in [
            ("legacy.json", r#"{ "root": "App.ns" }"#),
            ("empty.json", r#"{ "modules": [] }"#),
        ] {
            let path = directory.join(name);
            std::fs::write(&path, contents).unwrap_or_else(|error| panic!("{error}"));
            let error = refusal(&path);
            assert!(error.contains("`main`"), "{error}");
        }
    }

    #[test]
    fn other_keys_are_ignored() {
        let directory = scratch("other-keys");
        let path = directory.join(PROJECT_FILE_NAME);
        std::fs::write(
            &path,
            r#"{ "$schema": "../schemas/newspeak.schema.json", "main": "App.ns", "entry": "x" }"#,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let project = read_project(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(project.main, directory.join("App.ns"));
    }

    #[test]
    fn a_file_that_is_no_json_object_is_refused_saying_so() {
        let error = refusal_of("array", r#"["x", "App.ns"]"#);
        assert!(error.contains("JSON object"), "{error}");
    }

    #[test]
    fn a_key_written_twice_is_refused() {
        let error = refusal_of("duplicate-key", r#"{ "main": "A.ns", "main": "App.ns" }"#);
        assert!(error.contains("duplicate"), "{error}");
    }

    #[test]
    fn a_script_name_written_twice_is_refused_naming_the_script() {
        let error = refusal_of(
            "duplicate-script",
            r#"{ "main": "App.ns", "scripts": { "timed": "1", "timed": "2" } }"#,
        );
        assert!(error.contains("script `timed` is written twice"), "{error}");
    }

    #[test]
    fn an_empty_script_name_is_refused() {
        let error = refusal_of(
            "empty-script-name",
            r#"{ "main": "App.ns", "scripts": { "": "1" } }"#,
        );
        assert!(error.contains("a script name is empty"), "{error}");
    }

    /// Each case is the name as written in the JSON text, a raw string
    /// holding JSON escapes, and the name the message quotes: a leading
    /// space, a trailing space, a leading tab (`\t`), a trailing no-break
    /// space (U+00A0, `\u00a0`) and a leading byte order mark (U+FEFF,
    /// `\ufeff`).
    #[test]
    fn a_script_name_with_surrounding_whitespace_is_refused_naming_it() {
        for (index, (written, name)) in [
            (" x", " x"),
            ("x ", "x "),
            (r"\tx", "\tx"),
            (r"x\u00a0", "x\u{a0}"),
            (r"\ufeffx", "\u{feff}x"),
        ]
        .iter()
        .enumerate()
        {
            let error = refusal_of(
                &format!("spaced-script-name-{index}"),
                &format!(r#"{{ "main": "App.ns", "scripts": {{ "{written}": "" }} }}"#),
            );
            assert!(
                error.contains(&format!(
                    "script name `{name}` has leading or trailing whitespace"
                )),
                "{written}: {error}"
            );
        }
    }

    /// U+0085 is no whitespace to a JSON Schema pattern, so the reader
    /// takes it at an end as the schema does.
    #[test]
    fn a_script_name_may_hold_whitespace_inside_and_begin_with_a_dash() {
        let directory = scratch("inner-space-script-name");
        let path = directory.join(PROJECT_FILE_NAME);
        std::fs::write(
            &path,
            r#"{ "main": "App.ns", "scripts": { "two words": "", "-x": "", "a\u0085": "" } }"#,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let project = read_project(&path).unwrap_or_else(|error| panic!("{error}"));
        let names: Vec<&str> = project.scripts.keys().map(String::as_str).collect();
        assert_eq!(names, ["-x", "a\u{85}", "two words"]);
    }

    #[test]
    fn a_script_is_split_into_arguments_when_the_file_is_read() {
        let directory = scratch("split-script");
        let path = directory.join(PROJECT_FILE_NAME);
        std::fs::write(
            &path,
            r#"{ "main": "App.ns", "scripts": { "two": "first 'second word' \"\"", "none": "" } }"#,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let project = read_project(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(project.scripts["two"], ["first", "second word", ""]);
        assert!(project.scripts["none"].is_empty());
    }

    /// The schema takes any string as a script, so these refusals are the
    /// reader's alone.
    #[test]
    fn a_script_that_does_not_split_is_refused_naming_it() {
        for (index, (script, problem)) in [
            ("'unterminated", "unterminated `'`"),
            (r#"\"unterminated"#, r#"unterminated `"`"#),
            (r"a\\", "backslash at its end"),
        ]
        .iter()
        .enumerate()
        {
            let error = refusal_of(
                &format!("malformed-script-{index}"),
                &format!(r#"{{ "main": "App.ns", "scripts": {{ "timed": "{script}" }} }}"#),
            );
            assert!(
                error.contains("script `timed`") && error.contains(problem),
                "{script}: {error}"
            );
        }
    }

    #[test]
    fn a_script_that_is_no_string_is_refused_naming_scripts() {
        for (index, script) in [r#"["--timed"]"#, "1", "null"].iter().enumerate() {
            let error = refusal_of(
                &format!("script-no-string-{index}"),
                &format!(r#"{{ "main": "App.ns", "scripts": {{ "timed": {script} }} }}"#),
            );
            assert!(
                error.contains("invalid type") && error.contains("`scripts`"),
                "{script}: {error}"
            );
        }
    }

    #[test]
    fn a_schema_reference_that_is_no_string_is_refused() {
        for (name, contents) in [
            ("schema-number", r#"{ "$schema": 1, "main": "App.ns" }"#),
            ("schema-null", r#"{ "$schema": null, "main": "App.ns" }"#),
        ] {
            let error = refusal_of(name, contents);
            assert!(error.contains("string"), "{error}");
        }
    }

    #[test]
    fn a_directory_is_the_project_its_newspeak_json_describes() {
        let directory = scratch("directory");
        let project = directory.join(PROJECT_FILE_NAME);
        std::fs::write(&project, r#"{ "main": "App.ns" }"#)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            locate_program(&directory),
            Ok(ProgramPath::Project(project.clone()))
        );
        assert_eq!(locate_program(&project), Ok(ProgramPath::Project(project)));
    }

    #[test]
    fn a_directory_without_a_newspeak_json_is_refused_naming_the_file() {
        let directory = scratch("no-project");
        let error = match locate_program(&directory) {
            Ok(found) => panic!("{} was accepted as {found:?}", directory.display()),
            Err(error) => error,
        };
        assert!(error.contains(PROJECT_FILE_NAME), "{error}");
    }

    #[test]
    fn a_json_file_of_another_name_is_refused_naming_the_file() {
        for name in ["app.json", "Project.JSON"] {
            let error = match locate_program(Path::new(name)) {
                Ok(found) => panic!("{name} was accepted as {found:?}"),
                Err(error) => error,
            };
            assert!(error.contains(PROJECT_FILE_NAME), "{error}");
        }
    }

    #[test]
    fn any_other_file_is_a_source() {
        assert_eq!(
            locate_program(Path::new("App.ns")),
            Ok(ProgramPath::Source(PathBuf::from("App.ns")))
        );
    }
}
