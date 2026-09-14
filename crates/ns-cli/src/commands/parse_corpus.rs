//! `ns-next parse-corpus`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::source::collect_ns_files;

pub(crate) fn execute(directory: &Path, blacklist: Option<&Path>) -> ExitCode {
    let blacklist_path = blacklist.map(PathBuf::from).unwrap_or_else(|| {
        directory
            .parent()
            .unwrap_or(directory)
            .join("blacklist.toml")
    });
    let blacklist = match load_blacklist(&blacklist_path) {
        Ok(blacklist) => blacklist,
        Err(error) => {
            eprintln!("error: blacklist {}: {error}", blacklist_path.display());
            return ExitCode::FAILURE;
        }
    };

    let mut files: Vec<PathBuf> = Vec::new();
    collect_ns_files(directory, &mut files);
    files.sort();

    let (mut parsed, mut unexpected_fail, mut expected_fail, mut stale) = (0, 0, 0, 0);
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read_to_string(file).unwrap_or_default();
        let ok = ns_syntax::parse(&source).is_ok();
        let listed = blacklist.contains(&name);
        match (ok, listed) {
            (true, false) => parsed += 1,
            (true, true) => {
                stale += 1;
                eprintln!("STALE blacklist entry (now parses): {name}");
            }
            (false, true) => expected_fail += 1,
            (false, false) => {
                unexpected_fail += 1;
                eprintln!("FAIL {name}");
            }
        }
    }
    println!(
        "parsed {parsed}, failed {unexpected_fail}, expected-fail {expected_fail}, stale {stale}"
    );
    if unexpected_fail == 0 && stale == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Load the set of blacklisted file names.
///
/// A file that is present but unreadable or unparseable is a hard error
/// (returns `Err`): silently treating it as an empty blacklist would make
/// `parse-corpus` report phantom regressions with no explanation. A genuinely
/// absent file is treated as an empty blacklist with a one-line note — the
/// repo ships `corpus/blacklist.toml`, so absence is the unusual case and is
/// most likely a caller passing a bare directory with no companion file.
fn load_blacklist(path: &Path) -> Result<std::collections::HashSet<String>, String> {
    #[derive(serde::Deserialize)]
    struct BlacklistFile {
        #[serde(default)]
        entry: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        file: String,
        #[allow(dead_code)]
        reason: String,
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "note: no blacklist at {} (treating as empty)",
                path.display()
            );
            return Ok(std::collections::HashSet::new());
        }
        Err(error) => return Err(error.to_string()),
    };
    let blacklist_file: BlacklistFile = toml::from_str(&text).map_err(|error| error.to_string())?;
    Ok(blacklist_file
        .entry
        .into_iter()
        .map(|entry| entry.file)
        .collect())
}
