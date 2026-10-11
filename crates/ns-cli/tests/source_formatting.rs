//! The repository's own Newspeak sources are indented with four spaces, never
//! tabs, and commented with `///`. `corpus/` is excluded because it is
//! upstream's verbatim copy (`corpus/PROVENANCE.md`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The one source that keeps a `(* … *)`: the golden that pins both forms.
const BLOCK_COMMENT_GOLDEN: &str = "goldens/language/syntax/line_comment.ns";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every tracked `.ns` file outside `corpus/`, relative to `root`.
fn tracked_sources_outside_the_corpus(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "*.ns"])
        .output()
        .expect("git ls-files runs");
    assert!(output.status.success(), "git ls-files failed");
    let listing = String::from_utf8(output.stdout).expect("file names are UTF-8");
    listing
        .lines()
        .filter(|name| !name.starts_with("corpus/"))
        .map(str::to_string)
        .collect()
}

#[test]
fn newspeak_sources_outside_the_corpus_contain_no_tab() {
    let root = repository_root();
    let mut offences = Vec::new();
    for name in tracked_sources_outside_the_corpus(&root) {
        let text = std::fs::read_to_string(root.join(&name)).expect("a tracked file reads");
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

/// The house style is the `///` line comment, whose text an editor reads;
/// `(* … *)` stays valid Newspeak and is pinned by one golden. The check
/// lexes rather than searches, so a `(*` in a string is no offence.
#[test]
fn newspeak_sources_outside_the_corpus_comment_with_line_comments() {
    let root = repository_root();
    let mut offences = Vec::new();
    for name in tracked_sources_outside_the_corpus(&root) {
        if name == BLOCK_COMMENT_GOLDEN {
            continue;
        }
        let text = std::fs::read_to_string(root.join(&name)).expect("a tracked file reads");
        for comment in ns_syntax::lexer::Lexer::new(&text)
            .run_keeping_comments()
            .comments
        {
            if comment.kind == ns_syntax::CommentKind::Block {
                let line = text[..comment.span.start as usize].matches('\n').count() + 1;
                offences.push(format!("{name}:{line}"));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "(* … *) comments in Newspeak sources (write /// lines):\n{}",
        offences.join("\n")
    );
}
