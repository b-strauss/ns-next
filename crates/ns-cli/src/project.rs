//! The project file: a program of several modules, named in JSON. `root` is
//! the application's source; `modules` are the other modules its manifest
//! holds (`docs/design/image-format.md`, "Planned: one artifact, packaged
//! before `main:args:`").

use std::path::{Path, PathBuf};

/// The file as written. Unknown fields are refused so that a misspelt key
/// is a message rather than a module silently missing.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    root: String,
    #[serde(default)]
    modules: Vec<String>,
}

pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) modules: Vec<PathBuf>,
}

pub(crate) fn is_project_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

pub(crate) fn read_project(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    let file: ProjectFile =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let directory = path.parent().unwrap_or(Path::new(""));
    Ok(Project {
        root: resolve_written_path(directory, &file.root),
        modules: file
            .modules
            .iter()
            .map(|written| resolve_written_path(directory, written))
            .collect(),
    })
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
        std::fs::write(&absent, r#"{ "root": "App.ns" }"#)
            .unwrap_or_else(|error| panic!("{error}"));
        let project = read_project(&absent).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(project.root, directory.join("App.ns"));
        assert!(project.modules.is_empty());
        let empty = directory.join("empty.json");
        std::fs::write(&empty, r#"{ "root": "App.ns", "modules": [] }"#)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(read_project(&empty)
            .unwrap_or_else(|error| panic!("{error}"))
            .modules
            .is_empty());
    }

    #[test]
    fn an_unknown_field_and_a_missing_root_are_refused() {
        let directory = scratch("refusals");
        let unknown = directory.join("unknown.json");
        std::fs::write(&unknown, r#"{ "root": "App.ns", "entry": "main:args:" }"#)
            .unwrap_or_else(|error| panic!("{error}"));
        let error = refusal(&unknown);
        assert!(error.contains("entry"), "{error}");
        let missing = directory.join("missing.json");
        std::fs::write(&missing, r#"{ "modules": [] }"#).unwrap_or_else(|error| panic!("{error}"));
        let error = refusal(&missing);
        assert!(error.contains("root"), "{error}");
    }

    #[test]
    fn only_a_json_extension_names_a_project_file() {
        assert!(is_project_file(Path::new("a/Project.JSON")));
        assert!(!is_project_file(Path::new("App.ns")));
    }
}
