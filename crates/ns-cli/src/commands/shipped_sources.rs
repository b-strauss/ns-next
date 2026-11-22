//! `ns-next shipped-sources`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The file beside the sources that holds their fingerprint.
const FINGERPRINT_FILE: &str = "fingerprint";

/// `ns-next shipped-sources`: the `.ns` sources this binary embeds, which
/// `Kernel.ns`, the factories and the manifest are, in one of two forms --
/// their fingerprint on one line (`fingerprint`), or written below
/// `directory` in `core-library/`'s layout with a `fingerprint` file. A
/// directory is written only when everything in it is one of these files,
/// so a mistyped path never has its own files overwritten.
pub(crate) fn execute(directory: Option<&Path>, fingerprint: bool) -> ExitCode {
    if fingerprint {
        let line = format!("{}\n", ns_runtime::shipped_sources_fingerprint());
        return match crate::entry::print_output(&line) {
            Ok(()) => ExitCode::SUCCESS,
            Err(code) => code,
        };
    }
    let Some(directory) = directory else {
        // clap's required group makes this unreachable.
        report_line!("error: shipped-sources needs a directory or --fingerprint");
        return ExitCode::FAILURE;
    };
    match write(directory) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            report_line!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn write(directory: &Path) -> Result<(), String> {
    let files = ns_runtime::shipped_source_files();
    let mut own_files: BTreeSet<String> = files.iter().map(|file| file.path.to_string()).collect();
    own_files.insert(FINGERPRINT_FILE.to_string());
    let own_directories: BTreeSet<String> = own_files
        .iter()
        .flat_map(|path| {
            path.match_indices('/')
                .map(|(index, _)| path[..index].to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    if directory.exists() {
        refuse_foreign_entries(directory, "", &own_files, &own_directories)?;
    }
    for file in &files {
        write_file(&directory.join(file.path), file.text.as_bytes())?;
    }
    let line = format!("{}\n", ns_runtime::shipped_sources_fingerprint());
    write_file(&directory.join(FINGERPRINT_FILE), line.as_bytes())
}

/// Fails on the first entry of `directory` (`prefix` is its path below the
/// target, `/`-separated) that is not one of this command's own.
fn refuse_foreign_entries(
    directory: &Path,
    prefix: &str,
    own_files: &BTreeSet<String>,
    own_directories: &BTreeSet<String>,
) -> Result<(), String> {
    if !directory.is_dir() {
        return Err(format!("{} is not a directory", directory.display()));
    }
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("reading {}: {error}", directory.display()))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        paths.push(
            entry
                .map_err(|error| format!("reading {}: {error}", directory.display()))?
                .path(),
        );
    }
    paths.sort();
    for path in paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let relative = format!("{prefix}{name}");
        if path.is_dir() {
            if !own_directories.contains(&relative) {
                return Err(foreign(directory, &relative));
            }
            refuse_foreign_entries(&path, &format!("{relative}/"), own_files, own_directories)?;
        } else if !own_files.contains(&relative) {
            return Err(foreign(directory, &relative));
        }
    }
    Ok(())
}

fn foreign(directory: &Path, relative: &str) -> String {
    format!(
        "{} holds {relative}, which is not a shipped source; refusing to write there",
        directory.display()
    )
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|error| format!("writing {}: {error}", path.display()))
}
