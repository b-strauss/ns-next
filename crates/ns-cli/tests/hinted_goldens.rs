//! Every golden with every hint applied answers exactly what its `.toml`
//! records, under both lowerings: the hints' fixes change no program's
//! behaviour (`docs/design/editor-support.md`, "Hints"). The goldens
//! themselves stay as they are written; the rewritten program runs from a
//! scratch copy. Every golden's sources also reach a fixed point: the
//! result parses and a second pass changes nothing.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;

const LOWERINGS: [&str; 2] = ["optimized", "reference"];

/// The repository root, two levels above this crate, with no `..` left in
/// it, so that a source's place in the repository is a plain prefix strip.
fn repository_root() -> PathBuf {
    normalized(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// `path` with every `.` and `..` resolved lexically. The goldens' paths and
/// the files a project names both carry `..`, and two spellings of one file
/// must compare equal for the copy to keep every relative path resolving.
fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// The `.ns` files of a golden program: the file itself, or for a project
/// every file its `newspeak.json` names (`main` and `modules`) together with
/// every one inside its directory. A project may name sources outside its
/// own directory -- the awfy goldens run the benchmarks -- and those are the
/// largest programs the hints meet, so leaving them out would make the
/// oracle vacuous exactly where it matters. Refused: an empty set, which
/// would check nothing, and a source outside the repository, which the
/// scratch copy cannot lay out.
fn sources_of(program: &Path) -> Result<Vec<PathBuf>, String> {
    let program = normalized(program);
    let mut files = Vec::new();
    if program.is_dir() {
        ns_tooling::source::collect_ns_files(&program, &mut files);
        let project = program.join(common::PROJECT_FILE);
        let text = std::fs::read_to_string(&project)
            .map_err(|error| format!("read {}: {error}", project.display()))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| format!("parse {}: {error}", project.display()))?;
        let main = value.get("main").into_iter();
        let modules = value
            .get("modules")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten();
        for named in main.chain(modules) {
            let name = named
                .as_str()
                .ok_or_else(|| format!("{}: a source is not a string", project.display()))?;
            files.push(program.join(name));
        }
    } else {
        files.push(program.clone());
    }
    let mut files: Vec<PathBuf> = files.iter().map(|file| normalized(file)).collect();
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Err(format!("{} has no source to check", program.display()));
    }
    let root = repository_root();
    if let Some(outside) = files.iter().find(|file| !file.starts_with(&root)) {
        return Err(format!(
            "{} names {}, which is outside the repository {}",
            program.display(),
            outside.display(),
            root.display()
        ));
    }
    Ok(files)
}

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    for program in common::golden_programs() {
        let name = common::golden_name(&program);
        let sources = match sources_of(&program) {
            Ok(sources) => sources,
            Err(problem) => {
                trials.push(Trial::test(format!("fixed_point::{name}"), move || {
                    Err(problem.into())
                }));
                continue;
            }
        };
        let changed = sources.iter().any(|source| {
            let text = std::fs::read_to_string(source).expect("a readable source");
            ns_syntax::hints::apply_all(&text) != text
        });
        let checked = sources.clone();
        trials.push(Trial::test(format!("fixed_point::{name}"), move || {
            reaches_a_fixed_point(&checked)
        }));
        if !changed || common::expectation_of(&program).skipped().is_some() {
            continue;
        }
        for lowering in LOWERINGS {
            let program = program.clone();
            let sources = sources.clone();
            trials.push(Trial::test(
                format!("hinted::{lowering}::{name}"),
                move || runs_alike(&program, &sources, lowering),
            ));
        }
    }
    libtest_mimic::run(&arguments, trials).exit();
}

/// Every parsable source still parses with every hint applied, and applying
/// every hint again changes nothing.
fn reaches_a_fixed_point(sources: &[PathBuf]) -> Result<(), Failed> {
    for source in sources {
        let text = std::fs::read_to_string(source).expect("a readable source");
        if ns_syntax::parse(&text).is_err() {
            continue;
        }
        let applied = ns_syntax::hints::apply_all(&text);
        if ns_syntax::parse(&applied).is_err() {
            return Err(format!(
                "{}: the hinted source does not parse:\n{applied}",
                source.display()
            )
            .into());
        }
        let again = ns_syntax::hints::apply_all(&applied);
        if again != applied {
            return Err(format!(
                "{}: applying every hint a second time changes the source:\n\
                 --- after one pass\n{applied}--- after two\n{again}",
                source.display()
            )
            .into());
        }
    }
    Ok(())
}

/// Copies `program` and every source it names into a scratch directory at
/// their places relative to the repository root, so that each relative path
/// a project file holds still resolves; applies every hint to the copied
/// sources; runs the copy under `lowering`; and compares with the recorded
/// outcome.
fn runs_alike(program: &Path, sources: &[PathBuf], lowering: &str) -> Result<(), Failed> {
    let scratch = common::ScratchDirectory::new("hinted");
    let root = repository_root();
    let in_scratch = |path: &Path| -> PathBuf {
        let relative = path
            .strip_prefix(&root)
            .expect("sources_of refuses a path outside the repository");
        scratch.path().join(relative)
    };
    let program = normalized(program);
    let copy = in_scratch(&program);
    if program.is_dir() {
        copy_directory(&program, &copy);
    }
    for source in sources {
        let copied = in_scratch(source);
        if !copied.exists() {
            std::fs::create_dir_all(copied.parent().expect("a source has a directory"))
                .expect("create the source's directory");
            std::fs::copy(source, &copied).expect("copy a source");
        }
        let text = std::fs::read_to_string(&copied).expect("a readable source");
        std::fs::write(&copied, ns_syntax::hints::apply_all(&text))
            .expect("write the hinted source");
    }
    let expectation = common::expectation_of(&program);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(&copy)
        .arg("--lowering")
        .arg(lowering)
        .args(common::program_arguments(&program))
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    common::compare(
        &output,
        &expectation,
        &format!("hinted, under the {lowering} lowering"),
    )
    .map_err(Failed::from)
}

fn copy_directory(original: &Path, copy: &Path) {
    std::fs::create_dir_all(copy).expect("create the copy");
    for entry in std::fs::read_dir(original)
        .expect("read the golden")
        .flatten()
    {
        let path = entry.path();
        let target = copy.join(entry.file_name());
        if path.is_dir() {
            copy_directory(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy a file");
        }
    }
}
