//! `ns-next shipped-sources`: the embedded core library written out, for the
//! plugin and for an agent with a shell.

mod common;
use common::{ScratchDirectory, ns_next};
use std::ffi::OsStr;

fn expect_ok(arguments: &[&OsStr]) -> String {
    let (ok, out, error) = ns_next(arguments);
    assert!(ok, "{error}");
    out
}

#[test]
fn every_embedded_module_is_written_equal_to_the_binarys_text() {
    let directory = ScratchDirectory::new("shipped-sources");
    let target = directory.join("library");
    expect_ok(&["shipped-sources".as_ref(), target.as_os_str()]);
    let files = ns_runtime::shipped_source_files();
    assert_eq!(
        files.len(),
        8,
        "Kernel, Manifest, PlatformFactory, five more"
    );
    for file in &files {
        let written = std::fs::read(target.join(file.path))
            .unwrap_or_else(|error| panic!("{}: {error}", file.path));
        assert_eq!(written, file.text.as_bytes(), "{}", file.path);
        if file.name != "Manifest" {
            assert_eq!(
                ns_runtime::shipped_module_source(file.name),
                Some(file.text)
            );
        }
    }
    for path in [
        "Kernel.ns",
        "Manifest.ns",
        "PlatformFactory.ns",
        "platform/CollectionsFactory.ns",
    ] {
        assert!(target.join(path).is_file(), "{path}");
    }
}

#[test]
fn the_fingerprint_agrees_between_every_form() {
    let directory = ScratchDirectory::new("shipped-sources-fingerprint");
    let target = directory.join("library");
    expect_ok(&["shipped-sources".as_ref(), target.as_os_str()]);
    let printed = expect_ok(&["shipped-sources".as_ref(), "--fingerprint".as_ref()]);
    let written = std::fs::read_to_string(target.join("fingerprint")).expect("fingerprint file");
    let json = expect_ok(&["shipped-sources".as_ref(), "--json".as_ref()]);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(printed.lines().count(), 1);
    assert_eq!(written, printed);
    assert_eq!(parsed["fingerprint"], printed.trim_end());
    assert_eq!(
        printed.trim_end(),
        ns_runtime::shipped_sources_fingerprint()
    );
}

#[test]
fn json_lists_every_module_with_its_path_and_text() {
    let json = expect_ok(&["shipped-sources".as_ref(), "--json".as_ref()]);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
    let modules = parsed["modules"].as_array().expect("modules");
    let files = ns_runtime::shipped_source_files();
    assert_eq!(modules.len(), files.len());
    for (module, file) in modules.iter().zip(&files) {
        assert_eq!(module["name"], file.name);
        assert_eq!(module["path"], file.path);
        assert_eq!(module["text"], file.text);
    }
}

#[test]
fn a_foreign_file_in_the_directory_is_refused_and_nothing_is_written() {
    let directory = ScratchDirectory::new("shipped-sources-foreign");
    std::fs::write(directory.join("notes.txt"), "mine").expect("write");
    let (ok, _, error) = ns_next(&["shipped-sources".as_ref(), directory.path().as_os_str()]);
    assert!(!ok);
    assert!(error.contains("notes.txt"), "{error}");
    assert!(!directory.join("Kernel.ns").exists());

    let nested = ScratchDirectory::new("shipped-sources-nested");
    std::fs::create_dir_all(nested.join("platform")).expect("directory");
    std::fs::write(nested.join("platform/Other.ns"), "x").expect("write");
    let (ok, _, error) = ns_next(&["shipped-sources".as_ref(), nested.path().as_os_str()]);
    assert!(!ok);
    assert!(error.contains("Other.ns"), "{error}");
}

#[test]
fn its_own_files_are_overwritten() {
    let directory = ScratchDirectory::new("shipped-sources-overwrite");
    let target = directory.join("library");
    expect_ok(&["shipped-sources".as_ref(), target.as_os_str()]);
    std::fs::write(target.join("Kernel.ns"), "stale").expect("write");
    expect_ok(&["shipped-sources".as_ref(), target.as_os_str()]);
    let kernel = std::fs::read(target.join("Kernel.ns")).expect("read");
    assert_eq!(
        kernel,
        ns_runtime::shipped_module_source("Kernel")
            .expect("kernel")
            .as_bytes()
    );
}

#[test]
fn exactly_one_form_is_asked_for() {
    let (ok, _, _) = ns_next(&["shipped-sources".as_ref()]);
    assert!(!ok);
    let (ok, _, _) = ns_next(&[
        "shipped-sources".as_ref(),
        "--json".as_ref(),
        "--fingerprint".as_ref(),
    ]);
    assert!(!ok);
}
