//! The printer held to every source in the repository: formatting is a
//! fixed point after one pass, says what the source says (comments
//! included, positions and layout aside), and stays on a small stack.

use std::path::{Path, PathBuf};

fn all_sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for directory in [
        "corpus/modules",
        "goldens",
        "benchmarks",
        "crates/ns-runtime/core-library",
    ] {
        collect(&root.join(directory), &mut files);
    }
    files.sort();
    assert!(files.len() > 590, "found only {} sources", files.len());
    files
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(&path).expect("a readable source");
            (path, source)
        })
        .collect()
}

/// Every `.ns` file under `directory`, recursively (`ns-syntax` cannot
/// depend on `ns-tooling`, which has the same walk).
fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("a readable directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "ns") {
            files.push(path);
        }
    }
}

/// The first line where `before` and `after` differ, with three lines of
/// context on each side, both versions marked.
fn first_difference(before: &str, after: &str) -> String {
    let line_breaks = ['\n', '\r'];
    let before_lines: Vec<&str> = before.split(line_breaks).collect();
    let after_lines: Vec<&str> = after.split(line_breaks).collect();
    let differing = before_lines
        .iter()
        .zip(&after_lines)
        .position(|(first, second)| first != second)
        .unwrap_or(before_lines.len().min(after_lines.len()));
    let start = differing.saturating_sub(3);
    let mut report = String::new();
    for (label, lines) in [("before", &before_lines), ("after", &after_lines)] {
        report.push_str(&format!("--- {label}, from line {}:\n", start + 1));
        for (index, line) in lines.iter().enumerate().skip(start).take(7) {
            let marker = if index == differing { ">" } else { " " };
            report.push_str(&format!("{marker} {line}\n"));
        }
    }
    report
}

#[test]
fn formatting_is_idempotent() {
    for (path, source) in all_sources() {
        let once = ns_syntax::format(&source)
            .unwrap_or_else(|diagnostics| panic!("{} parses: {diagnostics:?}", path.display()));
        let twice = ns_syntax::format(&once)
            .unwrap_or_else(|diagnostics| panic!("{} reparses: {diagnostics:?}", path.display()));
        assert!(
            once == twice,
            "{} is not a fixed point after one format:\n{}",
            path.display(),
            first_difference(&once, &twice)
        );
    }
}

#[test]
fn formatting_preserves_the_module() {
    for (path, source) in all_sources() {
        let before = ns_syntax::parse(&source)
            .unwrap_or_else(|diagnostics| panic!("{} parses: {diagnostics:?}", path.display()));
        let formatted = ns_syntax::format(&source)
            .unwrap_or_else(|diagnostics| panic!("{} formats: {diagnostics:?}", path.display()));
        let after = ns_syntax::parse(&formatted)
            .unwrap_or_else(|diagnostics| panic!("{} reparses: {diagnostics:?}", path.display()));
        let before = before.without_positions();
        let after = after.without_positions();
        assert!(
            before == after,
            "{} says something else after formatting:\n{}",
            path.display(),
            first_difference(&format!("{before:#?}"), &format!("{after:#?}"))
        );
    }
}

/// Comments where the repository's sources have none: around and inside
/// every kind of node, in every placement, each source kept and a fixed
/// point after one format.
#[test]
fn comments_everywhere_keep_their_owners() {
    let sources = [
        "(* c *) Newspeak3 'Category' class A = () ()",
        "Newspeak3 (* c *) 'Category' /// d\nclass A = () ()",
        "class A = () () (* after *)",
        "class A <[T]> (* c *) = () ()",
        "class A new: x <T> (* c *) = () ()",
        "class A new: x (* c *)\n ^<A> = () ()",
        "class A = Object (* c *) <: Mixin () ()",
        "class A = Object\n/// c\n() ()",
        "class A = () (* between *) ()",
        "class A = ( | a. (* dangling *)\n | ) ()",
        "class A = ( | (* empty *) | ) ()",
        "class A = ( x. /// c\n y ) ()",
        "class A = () () : ( (* c *) public m = () )",
        "class A = () () : ( m = () /// c\n )",
        "class A = () ( public (* c *) m = () )",
        "class A = () ( lazy (* c *) x = 1. )",
        "class A = () ( m: x <Integer (* c *)> = () )",
        "class A = () ( m: x\n/// c\n n: y = () )",
        "class A = () ( m = ( | (* c *) | ^1 ) )",
        "class A = () ( m = ( | a (* c *) | ^a ) )",
        "class A = () ( m = (\n/// c\n^1 ) )",
        "class A = () ( m = ( /// c\n^1 ) )",
        "class A = () ( m = ( ^1 /// c\n) )",
        "class A = () ( m = ( (* c *) ) )",
        "class A = () ( m = ( ^(a foo /// c\n) ) )",
        "class A = () ( m = ( ^(a foo\n(* c *)) ) )",
        "class A = () ( m = ( ^x foo /// c\n; bar ) )",
        "class A = () ( m = ( ^x foo; (* c *) bar ) )",
        "class A = () ( m = ( ^{a. /// c\n b} ) )",
        "class A = () ( m = ( ^{ (* c *) a. b } ) )",
        "class A = () ( m = ( ^{a. b\n(* c *) } ) )",
        "class A = () ( m = ( ^{ (* c *) } ) )",
        "class A = () ( m = ( ^[ (* c *) ] ) )",
        "class A = () ( m = ( ^[:x | (* c *) ] ) )",
        "class A = () ( m = ( ^[:a (* c *) :b | a] ) )",
        "class A = () ( m = ( ^[ x foo.\n(* c *)\n] ) )",
        "class A = () ( m = ( ^[ /// c\n x ] ) )",
        "class A = () ( m = ( ^x <-: (* later *) foo ) )",
        "class A = () ( m = ( ^x (* c *) <-: foo ) )",
        "class A = () ( m = ( ^x foo: 1 (* c *)\n bar: 2 ) )",
        "class A = () ( m = ( ^x\n/// c\nfoo: 1 bar: 2 ) )",
        "class A = () ( m = ( ^x ifTrue: [a] /// c\n ifFalse: [b] ) )",
        "class A = () ( m = ( ^(x) (* c *) foo ) )",
        "class A = () ( m = ( ^x foo (* c *) bar ) )",
        "class A = () ( m = ( ^x foo /// c\n bar ) )",
        "class A = () ( m = ( ^x + (* c *) y ) )",
        "class A = () ( m = ( ^x + y /// c\n - z ) )",
        "class A = () ( m = ( x:: (* c *) 3. ^x ) )",
        "class A = () ( m = ( ^x foo: (* a\n     b *) 1 ) )",
        "class A = () ( m = ( ^x foo (* a\n     b *)\n bar ) )",
        "class A = () ( m = ( ^ (* c *) 1 ) )",
        "class A = () (\r  (* one\r     two *)\r  m = ( ^1 /// c\r )\r)",
    ];
    for source in sources {
        let before = ns_syntax::parse(source)
            .unwrap_or_else(|diagnostics| panic!("{source:?} parses: {diagnostics:?}"));
        let once = ns_syntax::format(source).unwrap_or_else(|_| panic!("{source:?} formats"));
        let twice = ns_syntax::format(&once)
            .unwrap_or_else(|diagnostics| panic!("{once:?} reparses: {diagnostics:?}"));
        assert!(
            once == twice,
            "{source:?} is not a fixed point:\n{}",
            first_difference(&once, &twice)
        );
        let after = ns_syntax::parse(&once)
            .expect("parsed above")
            .without_positions();
        let before = before.without_positions();
        assert!(
            before == after,
            "{source:?} says something else as {once:?}:\n{}",
            first_difference(&format!("{before:#?}"), &format!("{after:#?}"))
        );
    }
}

/// Only strings are compared here: the tree and the document of a chain
/// this long compare and print recursively.
#[test]
fn a_long_chain_formats_on_a_small_stack() {
    let mut source = String::from("class A = () (\npublic m = ( ^1");
    for _ in 0..100_000 {
        source.push_str(" + 1");
    }
    source.push_str(" )\n)\n");
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let once = ns_syntax::format(&source).unwrap_or_else(|_| panic!("the chain formats"));
            let twice = ns_syntax::format(&once).unwrap_or_else(|_| panic!("the chain reformats"));
            assert!(once == twice, "the chain is not a fixed point");
        })
        .expect("spawn")
        .join()
        .expect("formatting a long chain stays on a small stack");
}

/// A comment added at the end of a line, in each form, to every few lines
/// of a few sources with every kind of statement: wherever the attachment
/// puts it, formatting is a fixed point after one pass and keeps it. A
/// variant that no longer parses (the comment inside a string) is skipped.
#[test]
fn a_comment_added_at_a_line_end_formats_to_a_fixed_point() {
    const STRIDE: usize = 3;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sources = [
        "goldens/library/number/fraction/a_fraction_is_made_in_normal_form.ns",
        "goldens/language/syntax/line_comment.ns",
        "benchmarks/are-we-fast-yet/Richards.ns",
    ];
    let mut variants = 0;
    for name in sources {
        let source = std::fs::read_to_string(root.join(name)).expect("a readable source");
        let lines: Vec<&str> = source.lines().collect();
        for comment in [" /// zq", " (* zq *)"] {
            for index in (0..lines.len()).step_by(STRIDE) {
                let mut variant = lines
                    .clone()
                    .into_iter()
                    .map(String::from)
                    .collect::<Vec<_>>();
                variant[index].push_str(comment);
                let variant = variant.join("\n") + "\n";
                let Ok(once) = ns_syntax::format(&variant) else {
                    continue;
                };
                variants += 1;
                let twice = ns_syntax::format(&once)
                    .unwrap_or_else(|diagnostics| panic!("{once:?} reparses: {diagnostics:?}"));
                assert!(
                    once == twice,
                    "{name}, line {} with `{comment}`, is not a fixed point:\n{}",
                    index + 1,
                    first_difference(&once, &twice)
                );
                assert!(
                    once.contains("zq"),
                    "{name}, line {}: the comment is lost",
                    index + 1
                );
            }
        }
    }
    assert!(variants > 100, "only {variants} variants parsed");
}
