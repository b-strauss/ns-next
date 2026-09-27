use std::path::PathBuf;

fn corpus_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

/// Every corpus module parses: the corpus is real Newspeak, and the parser
/// follows the specification's grammar (`docs/design/frontend.md`,
/// "Grammar"), so a module that fails is a parser defect or a program the
/// specification refuses -- something to decide, never to set aside.
#[test]
fn every_corpus_file_parses() {
    let mut modules: Vec<PathBuf> = std::fs::read_dir(corpus_directory().join("modules"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ns"))
        .collect();
    modules.sort();
    assert!(modules.len() > 50, "corpus looks empty: {}", modules.len());

    let failures: Vec<String> = modules
        .iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(path).unwrap();
            ns_syntax::parse(&source).err().map(|errors| {
                format!(
                    "{}: {:?}",
                    path.file_name().unwrap().to_string_lossy(),
                    errors.first()
                )
            })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "\nparse failures ({}):\n{}\n",
        failures.len(),
        failures.join("\n")
    );
}
