//! Every Newspeak source of the repository outside `corpus/` and
//! `prior-art/` is written exactly as `ns-next-analyzer fmt` writes it.

#[test]
fn every_repository_source_is_formatted() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for directory in ["goldens", "benchmarks", "crates/ns-runtime/core-library"] {
        ns_tooling::source::collect_ns_files(&root.join(directory), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 380,
        "the gate found only {} sources",
        files.len()
    );
    let mut unformatted = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("a readable source");
        match ns_syntax::format(&source) {
            Ok(formatted) if formatted == source => {}
            Ok(formatted) => unformatted.push(format!(
                "{}: first difference at line {}",
                file.display(),
                first_differing_line(&source, &formatted)
            )),
            Err(_) => unformatted.push(format!("{}: does not parse", file.display())),
        }
    }
    assert!(
        unformatted.is_empty(),
        "run `ns-next-analyzer fmt`:\n{}",
        unformatted.join("\n")
    );
}

fn first_differing_line(left: &str, right: &str) -> usize {
    left.lines()
        .zip(right.lines())
        .position(|(first, second)| first != second)
        .unwrap_or_else(|| left.lines().count().min(right.lines().count()))
        + 1
}
