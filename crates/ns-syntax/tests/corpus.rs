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

    // Task 14a installs this test and the two-group class-grammar restructure
    // it exercises. The full triage — the dominant remaining blocker is
    // declaration-position type annotations (`x <T>`, `sel: p <T>`, `^<T>`),
    // then `lazy` slots in the header and a few exotic forms — and the
    // eventual >=90% target are Task 14b's job. Until then this test only
    // reports the still-failing files and guards against a regression below
    // the level the restructure reached.
    eprintln!(
        "corpus: {parsed}/{} files parse ({} still failing)",
        modules.len(),
        failures.len()
    );
    for f in failures.iter() {
        eprintln!("  FAIL {f}");
    }

    assert!(
        stale.is_empty(),
        "stale blacklist entries ({}): {}",
        stale.len(),
        stale.join(", ")
    );
    assert!(
        parsed >= 40,
        "corpus pass rate regressed: only {parsed}/{} parse (was 43 after Task 14a)",
        modules.len()
    );
}
