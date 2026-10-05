//! The repository's own Newspeak sources are indented with four spaces, never
//! tabs. `corpus/` is excluded because it is upstream's verbatim copy
//! (`corpus/PROVENANCE.md`).

use std::path::Path;
use std::process::Command;

#[test]
fn newspeak_sources_outside_the_corpus_contain_no_tab() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("git")
        .current_dir(&root)
        .args(["ls-files", "*.ns"])
        .output()
        .expect("git ls-files runs");
    assert!(output.status.success(), "git ls-files failed");
    let listing = String::from_utf8(output.stdout).expect("file names are UTF-8");
    let mut offences = Vec::new();
    for name in listing.lines() {
        if name.starts_with("corpus/") {
            continue;
        }
        let text = std::fs::read_to_string(root.join(name)).expect("a tracked file reads");
        for (index, line) in text.lines().enumerate() {
            if line.contains('\t') {
                offences.push(format!("{name}:{}", index + 1));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "tabs in Newspeak sources (indent with four spaces):\n{}",
        offences.join("\n")
    );
}
