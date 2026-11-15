//! The shipped sources the binary writes out (`shipped_source_files`): their
//! recorded paths are the files' real places below `core-library/`, so a
//! file moved without its record fails here.

use ns_runtime::{
    shipped_module_named_by_text, shipped_module_source, shipped_source_files,
    shipped_sources_fingerprint,
};
use std::path::Path;

fn core_library_directory() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("core-library")
}

#[test]
fn every_recorded_path_names_the_file_the_text_was_embedded_from() {
    let directory = core_library_directory();
    for file in shipped_source_files() {
        let on_disk = std::fs::read(directory.join(file.path))
            .unwrap_or_else(|error| panic!("{} ({}): {error}", file.name, file.path));
        assert_eq!(
            on_disk,
            file.text.as_bytes(),
            "{} is not the text embedded for {}",
            file.path,
            file.name
        );
    }
}

#[test]
fn every_source_file_under_the_directory_is_recorded() {
    let directory = core_library_directory();
    let mut recorded: Vec<String> = shipped_source_files()
        .iter()
        .map(|file| file.path.to_string())
        .collect();
    let mut found = Vec::new();
    let mut pending = vec![directory.clone()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current).expect("read the core library") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "ns") {
                let relative = path.strip_prefix(&directory).expect("below the directory");
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    recorded.sort();
    found.sort();
    assert_eq!(recorded, found);
}

#[test]
fn a_shipped_module_source_is_the_text_of_its_file() {
    for file in shipped_source_files() {
        if file.name != "Manifest" {
            assert_eq!(shipped_module_source(file.name), Some(file.text));
        }
    }
}

#[test]
fn the_fingerprint_is_sixteen_hexadecimal_digits_and_stable() {
    let fingerprint = shipped_sources_fingerprint();
    assert_eq!(fingerprint.len(), 16, "{fingerprint}");
    assert!(fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(fingerprint, shipped_sources_fingerprint());
}

/// A text is a shipped module's only byte for byte: each module's own text
/// names it, `Manifest.ns`'s none, since a program compiles it as its own,
/// and a text one byte longer, shorter or different none.
#[test]
fn only_a_shipped_module_s_exact_text_names_it() {
    for file in shipped_source_files() {
        let expected = (file.name != "Manifest").then_some(file.name);
        assert_eq!(shipped_module_named_by_text(file.text), expected);
        let longer = format!("{} ", file.text);
        let shorter = &file.text[..file.text.len() - 1];
        let mut changed = file.text.as_bytes().to_vec();
        let last = changed.len() - 1;
        changed[last - 1] = if changed[last - 1] == b')' {
            b' '
        } else {
            b')'
        };
        let changed = String::from_utf8(changed).expect("an ASCII byte replaced");
        for text in [longer.as_str(), shorter, changed.as_str()] {
            assert_eq!(shipped_module_named_by_text(text), None, "{}", file.name);
        }
    }
}
