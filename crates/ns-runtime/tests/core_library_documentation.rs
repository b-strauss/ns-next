//! Every member of a shipped core-library module is documented
//! (CLAUDE.md, "Conventions"): a `///` comment directly above every class,
//! slot and method, whatever its access.

use std::path::Path;

use ns_syntax::ast::{ClassBody, ClassDeclaration, MessagePattern, Module};

/// The modules the rule holds today.
const DOCUMENTED_MODULES: [&str; 3] = ["Kernel.ns", "PlatformFactory.ns", "CollectionsFactory.ns"];

#[test]
fn every_member_of_a_documented_module_has_a_comment() {
    let mut undocumented = Vec::new();
    for name in DOCUMENTED_MODULES {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("core-library")
            .join(name);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let module = match ns_syntax::parse(&source) {
            Ok(module) => module,
            Err(_) => panic!("{name} does not parse"),
        };
        let comments = comment_spans(&source);
        for (member, start) in member_starts(&module) {
            if !directly_after_a_comment(&source, &comments, start) {
                undocumented.push(format!("{name}: {member}"));
            }
        }
    }
    assert!(
        undocumented.is_empty(),
        "members without a `///` comment directly above them:\n{}",
        undocumented.join("\n")
    );
}

/// Every `///` block of `source`, as a byte range. A `(* … *)` is no
/// member's documentation: the house style is the line comment, whose
/// text a reader such as a hover takes.
fn comment_spans(source: &str) -> Vec<(usize, usize)> {
    ns_syntax::lexer::Lexer::new(source)
        .run_keeping_comments()
        .comments
        .iter()
        .filter(|comment| comment.kind == ns_syntax::CommentKind::Line)
        .map(|comment| (comment.span.start as usize, comment.span.end as usize))
        .collect()
}

/// Each class, slot and method of `module`, the module's own class
/// included, with a printable name and the offset its declaration starts
/// at.
fn member_starts(module: &Module) -> Vec<(String, usize)> {
    let mut members = Vec::new();
    let root = &module.root_class;
    members.push((format!("{} (class)", root.name.name), class_start(root)));
    let mut pending = vec![(root.name.name.clone(), root)];
    while let Some((path, class)) = pending.pop() {
        let mut sides = vec![(path.clone(), &class.instance_side)];
        if let Some(class_side) = &class.class_side {
            sides.push((format!("{path} class"), class_side));
        }
        for (side, body) in sides {
            collect_body(&side, body, &mut members);
            for nested in body.nested_classes() {
                let nested_path = format!("{side}>>{}", nested.name.name);
                members.push((format!("{nested_path} (class)"), class_start(nested)));
                pending.push((nested_path, nested));
            }
        }
    }
    members
}

/// The slots and methods of one side of a class.
fn collect_body(side: &str, body: &ClassBody, members: &mut Vec<(String, usize)>) {
    for slot in body.slots() {
        let start = (slot.span.start.min(slot.name.span.start)) as usize;
        members.push((format!("{side}>>{} (slot)", slot.name.name), start));
    }
    for method in body.methods() {
        let (selector, selector_start) = selector_of(&method.pattern);
        let start = method.span.start.min(selector_start) as usize;
        members.push((format!("{side}>>{selector}"), start));
    }
}

/// Where a class declaration starts: its access modifier, else `class`.
fn class_start(class: &ClassDeclaration) -> usize {
    class.span.start.min(class.name.span.start) as usize
}

/// A pattern's selector and the offset of its first token.
fn selector_of(pattern: &MessagePattern) -> (String, u32) {
    match pattern {
        MessagePattern::Unary { selector, .. } => (selector.name.clone(), selector.span.start),
        MessagePattern::Binary { selector, .. } => (selector.name.clone(), selector.span.start),
        MessagePattern::Keyword { parts } => {
            let selector = parts
                .iter()
                .map(|part| format!("{}:", part.keyword.name))
                .collect::<String>();
            let start = parts.first().map_or(0, |part| part.keyword.span.start);
            (selector, start)
        }
    }
}

/// Whether a comment ends at or before `start` with only whitespace
/// between it and `start`.
fn directly_after_a_comment(source: &str, comments: &[(usize, usize)], start: usize) -> bool {
    comments
        .iter()
        .any(|&(_, end)| end <= start && source[end..start].chars().all(char::is_whitespace))
}
