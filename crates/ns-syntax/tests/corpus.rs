use std::collections::HashSet;
use std::path::PathBuf;

#[derive(serde::Deserialize, Default)]
struct Blacklist {
    #[serde(default)]
    entry: Vec<Entry>,
}

#[derive(serde::Deserialize)]
struct Entry {
    file: String,
    reason: String,
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

#[test]
fn every_corpus_file_parses_or_is_blacklisted() {
    let corpus = corpus_dir();
    let bl_text = std::fs::read_to_string(corpus.join("blacklist.toml")).unwrap();
    let bl: Blacklist = toml::from_str(&bl_text).unwrap();
    let blacklisted: HashSet<&str> = bl.entry.iter().map(|e| e.file.as_str()).collect();
    assert!(
        bl.entry.iter().all(|e| !e.reason.trim().is_empty()),
        "blacklist entry missing a reason"
    );

    let mut modules: Vec<PathBuf> = std::fs::read_dir(corpus.join("modules"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ns"))
        .collect();
    modules.sort();
    assert!(modules.len() > 50, "corpus looks empty: {}", modules.len());

    let mut parsed = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();

    for path in &modules {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(path).unwrap();
        match ns_syntax::parse(&src) {
            Ok(_) => {
                parsed += 1;
                if blacklisted.contains(name.as_str()) {
                    stale.push(name);
                }
            }
            Err(err) => {
                if !blacklisted.contains(name.as_str()) {
                    failures.push(format!("{name}: {:?}", err.first()));
                }
            }
        }
    }

    // Libtest captures this; it is the quick "where are we" line.
    eprintln!("corpus: {parsed}/{} files parse", modules.len());

    // The real Milestone 1 assertions (Task 14b): every non-blacklisted file
    // parses, and no blacklist entry is stale (a listed file that now parses).
    assert!(
        failures.is_empty() && stale.is_empty(),
        "\nunexpected parse failures ({}):\n{}\n\nstale blacklist entries ({}):\n{}\n",
        failures.len(),
        failures.join("\n"),
        stale.len(),
        stale.join("\n"),
    );
}
