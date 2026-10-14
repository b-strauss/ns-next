//! Every golden with every hint applied answers exactly what its `.toml`
//! records, under both lowerings: the hints' fixes change no program's
//! behaviour (`docs/design/editor-support.md`, "Hints"). The goldens
//! themselves stay as they are written; the rewritten program runs from a
//! scratch copy. Every golden's sources also reach a fixed point: the
//! result parses and a second pass changes nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;

const LOWERINGS: [&str; 2] = ["optimized", "reference"];

/// The `.ns` files of a golden program: the file itself, or every one in a
/// project's directory.
fn sources_of(program: &Path) -> Vec<PathBuf> {
    if program.is_dir() {
        let mut files = Vec::new();
        ns_tooling::source::collect_ns_files(program, &mut files);
        files.sort();
        files
    } else {
        vec![program.to_path_buf()]
    }
}

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    for program in common::golden_programs() {
        let name = common::golden_name(&program);
        let sources = sources_of(&program);
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
            trials.push(Trial::test(
                format!("hinted::{lowering}::{name}"),
                move || runs_alike(&program, lowering),
            ));
        }
    }
    libtest_mimic::run(&arguments, trials).exit();
}

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
        if !ns_syntax::hints::hints_of(&applied).is_empty() {
            return Err(format!(
                "{}: hints are left after applying every hint",
                source.display()
            )
            .into());
        }
    }
    Ok(())
}

/// Copies `program` into a scratch directory with every hint applied to its
/// sources, runs it under `lowering`, and compares with the recorded outcome.
fn runs_alike(program: &Path, lowering: &str) -> Result<(), Failed> {
    let scratch = common::ScratchDirectory::new("hinted");
    let file_name = program.file_name().expect("a golden has a name");
    let copy = scratch.join(&file_name.to_string_lossy());
    if program.is_dir() {
        copy_directory(program, &copy);
    } else {
        std::fs::copy(program, &copy).expect("copy the golden");
    }
    for source in sources_of(&copy) {
        let text = std::fs::read_to_string(&source).expect("a readable source");
        std::fs::write(&source, ns_syntax::hints::apply_all(&text))
            .expect("write the hinted source");
    }
    let expectation = common::expectation_of(program);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(&copy)
        .arg("--lowering")
        .arg(lowering)
        .args(common::program_arguments(program))
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
