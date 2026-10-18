//! The shipped core library has no hint: every slot it can initialize in
//! its declaration is, and every slot that can be read-only is. Only the
//! core library is held to it; goldens pin a program's form along with
//! its behaviour, and the benchmarks stay close to the programs they
//! adapt.

const CORE_LIBRARY: &str = "crates/ns-runtime/core-library";

fn core_library_sources() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    ns_tooling::source::collect_ns_files(&root.join(CORE_LIBRARY), &mut files);
    files.sort();
    assert!(
        files.len() >= 8,
        "the gate found only {} sources",
        files.len()
    );
    files
}

#[test]
fn the_core_library_has_no_hint() {
    let mut hinted = Vec::new();
    for file in core_library_sources() {
        let source = std::fs::read_to_string(&file).expect("a readable source");
        for hint in ns_analyzer::hints::hints_of(&source) {
            let line = source[..hint.span.start as usize].matches('\n').count() + 1;
            hinted.push(format!("{}:{line}: {}", file.display(), hint.message()));
        }
    }
    assert!(
        hinted.is_empty(),
        "run `cargo nextest run -p ns-analyzer --run-ignored only apply_every_hint_to_the_core_library`:\n{}",
        hinted.join("\n")
    );
}

/// Rewrites the core library with every hint applied, in the formatter's
/// layout. Run deliberately, then regenerate the image fixture: its header
/// carries the fingerprint of these sources.
#[test]
#[ignore = "rewrites crates/ns-runtime/core-library; run deliberately"]
fn apply_every_hint_to_the_core_library() {
    for file in core_library_sources() {
        let source = std::fs::read_to_string(&file).expect("a readable source");
        let applied = ns_analyzer::hints::apply_all(&source);
        let formatted = ns_syntax::format(&applied).unwrap_or(applied);
        if formatted != source {
            std::fs::write(&file, formatted).expect("write the source");
        }
    }
}
