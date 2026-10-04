//! The project file: a program of several modules, named in JSON. `main` is
//! the application module's source; `modules` are the other modules its
//! manifest holds (`docs/design/image-format.md`, "Packaging"). Every
//! project file is named `newspeak.json`, so a project is its directory. Its
//! schema is `schemas/newspeak.schema.json`, and `read_project` enforces the
//! same rules with messages of its own.

use std::path::{Path, PathBuf};

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
    modules: Vec<String>,
}

/// A string where the key is present. `Option<String>` alone would also take
/// `null`, which the schema refuses.
fn a_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    <String as serde::Deserialize>::deserialize(deserializer).map(Some)
}

pub(crate) struct Project {
    pub(crate) main: PathBuf,
    pub(crate) modules: Vec<PathBuf>,
}

pub(crate) fn read_project(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    let file: ProjectFile =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let in_file = |error: String| format!("{}: {error}", path.display());
    check_module_path("main", &file.main).map_err(in_file)?;
    for written in &file.modules {
        check_module_path("modules", written).map_err(in_file)?;
    }
    let directory = path.parent().unwrap_or(Path::new(""));
    let main = resolve_written_path(directory, &file.main);
    let mut modules: Vec<PathBuf> = Vec::with_capacity(file.modules.len());
    for written in &file.modules {
        let module = resolve_written_path(directory, written);
        if same_path(&module, &main) {
            return Err(in_file(format!(
                "`modules` names `main`'s module `{written}` again"
            )));
        }
        if modules.iter().any(|earlier| same_path(earlier, &module)) {
            return Err(in_file(format!("`modules` names `{written}` twice")));
        }
        modules.push(module);
    }
    Ok(Project { main, modules })
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
/// symbolic link or a `..` that returns is not seen through.
fn same_path(first: &Path, second: &Path) -> bool {
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
        assert!(read_project(&empty)
            .unwrap_or_else(|error| panic!("{error}"))
            .modules
            .is_empty());
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
