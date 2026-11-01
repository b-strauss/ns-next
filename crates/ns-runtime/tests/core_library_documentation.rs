//! Every member of a shipped core-library module is documented
//! (CLAUDE.md, "Conventions"): a `///` comment directly above every class,
//! slot and method, whatever its access, of two or more lines, with a
//! `@param` for each parameter in the pattern's order, one `@answers` for
//! a method, and one `@factory` for a class whose header names its factory.

use std::path::Path;

use indoc::indoc;
use ns_syntax::ast::{ClassBody, ClassDeclaration, MessagePattern, Module};
use ns_syntax::doc_comment::DocTagKind;
use ns_syntax::{Comment, CommentKind, DocComment};

/// The modules the rule holds today.
const DOCUMENTED_MODULES: [&str; 8] = [
    "Kernel.ns",
    "Manifest.ns",
    "PlatformFactory.ns",
    "platform/TranscriptFactory.ns",
    "platform/TimeFactory.ns",
    "platform/CollectionsFactory.ns",
    "platform/TypedArraysFactory.ns",
    "platform/MirrorsFactory.ns",
];

/// Reads a core-library module and parses it.
fn read_module(name: &str) -> (String, Module) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("core-library")
        .join(name);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let module = match ns_syntax::parse(&source) {
        Ok(module) => module,
        Err(_) => panic!("{name} does not parse"),
    };
    (source, module)
}

#[test]
fn every_member_of_a_documented_module_has_a_comment() {
    let mut undocumented = Vec::new();
    for name in DOCUMENTED_MODULES {
        let (source, module) = read_module(name);
        let comments = comments_of(&source);
        for member in members_of(&module) {
            if comment_above(&source, &comments, member.start).is_none() {
                undocumented.push(format!("{name}: {}", member.name));
            }
        }
    }
    assert!(
        undocumented.is_empty(),
        "members without a `///` comment directly above them:\n{}",
        undocumented.join("\n")
    );
}

#[test]
fn every_member_of_a_documented_module_has_its_tags() {
    let mut faults = Vec::new();
    for name in DOCUMENTED_MODULES {
        let (source, module) = read_module(name);
        faults.extend(tag_faults(name, &source, &module));
    }
    assert!(
        faults.is_empty(),
        "members whose doc tags are wrong:\n{}",
        faults.join("\n")
    );
}

/// The checker is tested on a module of its own, so that a rule it
/// silently stopped enforcing shows here and not as a module that passes.
#[test]
fn the_tag_check_finds_each_fault() {
    let source = indoc! {"
        /// The sample root.
        ///
        /// @factory makes a sample.
        /// @param first the first.
        /// @param second the second.
        class Sample first: first second: second = Value (
          |
            /// A slot.
            private note = 1.
          |
        ) (
          /// Leaves one parameter out.
          ///
          /// @param one the one.
          /// @answers nil.
          public missing: one with: two = ( ^nil )

          /// Names a parameter that is none.
          ///
          /// @param one the one.
          /// @param three the three.
          /// @answers nil.
          public extra: one = ( ^nil )

          /// Names them in the wrong order.
          ///
          /// @param two the two.
          /// @param one the one.
          /// @answers nil.
          public ordered: one and: two = ( ^nil )

          /// Names one twice.
          ///
          /// @param one the one.
          /// @param one the one again.
          /// @answers nil.
          public twice: one = ( ^nil )

          /// Says nothing about its answer.
          ///
          /// It has a description and no tag.
          public silent = ( ^nil )

          /// Answers twice.
          ///
          /// @answers nil.
          /// @answers nothing.
          public doubled = ( ^nil )

          /// Carries a factory tag.
          ///
          /// @factory makes nothing.
          /// @answers nil.
          public misplaced = ( ^nil )

          /// Stands a blank line above its method.
          ///
          /// @answers nil.

          public detached = ( ^nil )

          /// A nested class.
          ///
          /// @param value the value.
          public class Inner with: value = Value () ()
        )
    "};
    let module = ns_syntax::parse(source).unwrap_or_else(|_| panic!("the module parses"));
    assert_eq!(
        tag_faults("Sample.ns", source, &module),
        [
            "Sample.ns: Sample>>note (slot): the comment has fewer than two lines",
            "Sample.ns: Sample>>missing:with:: no `@param` for `two`",
            "Sample.ns: Sample>>extra:: `@param three` names no parameter",
            "Sample.ns: Sample>>ordered:and:: the `@param` tags are not in the pattern's order, `one`, `two`",
            "Sample.ns: Sample>>twice:: `@param one` is given more than once",
            "Sample.ns: Sample>>silent: a method has exactly one `@answers`, this one has 0",
            "Sample.ns: Sample>>doubled: a method has exactly one `@answers`, this one has 2",
            "Sample.ns: Sample>>misplaced: only a class has a `@factory`",
            "Sample.ns: Sample>>Inner (class): a class whose header names its factory has exactly one `@factory`, this one has 0",
        ]
    );
    let comments = comments_of(source);
    let undocumented: Vec<String> = members_of(&module)
        .into_iter()
        .filter(|member| comment_above(source, &comments, member.start).is_none())
        .map(|member| member.name)
        .collect();
    assert_eq!(undocumented, ["Sample>>detached"]);
    // A line break is one whichever way it is written, and a second one
    // detaches the comment whichever way either is written.
    for (text, attached) in [
        ("/// A.\nx", true),
        ("/// A.\r\nx", true),
        ("/// A.\rx", true),
        ("/// A.\n\nx", false),
        ("/// A.\r\n\r\nx", false),
        ("/// A.\r\rx", false),
        ("/// A.\n\r\nx", false),
    ] {
        let start = text.len() - 1;
        assert_eq!(
            comment_above(text, &comments_of(text), start).is_some(),
            attached,
            "{text:?}"
        );
    }
}

/// Everything the tags of each member of `module`, which `source` holds,
/// get wrong, one line each as `module: member: what is wrong`. A member
/// without a comment is the other test's business.
fn tag_faults(name: &str, source: &str, module: &Module) -> Vec<String> {
    let comments = comments_of(source);
    let mut faults = Vec::new();
    for member in members_of(module) {
        let Some(comment) = comment_above(source, &comments, member.start) else {
            continue;
        };
        let mut wrong = |what: String| faults.push(format!("{name}: {}: {what}", member.name));
        let line_count = source[comment.span.start as usize..comment.span.end as usize]
            .lines()
            .count();
        if line_count < 2 {
            wrong("the comment has fewer than two lines".to_string());
        }
        let doc = DocComment::parse(comment, source);
        let count_of = |kind: DocTagKind| doc.tags.iter().filter(|tag| tag.kind == kind).count();
        let described: Vec<&str> = doc
            .tags
            .iter()
            .filter(|tag| tag.kind == DocTagKind::Param)
            .map(|tag| tag.argument.as_ref().map_or("", |(word, _)| word.as_str()))
            .collect();
        if described != member.parameters {
            for parameter in &member.parameters {
                if !described.contains(&parameter.as_str()) {
                    wrong(format!("no `@param` for `{parameter}`"));
                }
            }
            for (position, word) in described.iter().enumerate() {
                if !member.parameters.iter().any(|parameter| parameter == word) {
                    wrong(format!("`@param {word}` names no parameter"));
                } else if described[..position].contains(word) {
                    wrong(format!("`@param {word}` is given more than once"));
                }
            }
            // Order is judged over the names that are parameters, each once.
            let mut seen: Vec<&str> = Vec::new();
            for word in &described {
                if member.parameters.iter().any(|parameter| parameter == word)
                    && !seen.contains(word)
                {
                    seen.push(word);
                }
            }
            let expected: Vec<&str> = member
                .parameters
                .iter()
                .map(String::as_str)
                .filter(|parameter| seen.contains(parameter))
                .collect();
            if seen != expected {
                wrong(format!(
                    "the `@param` tags are not in the pattern's order, `{}`",
                    member.parameters.join("`, `")
                ));
            }
        }
        let answers = count_of(DocTagKind::Answers);
        if member.kind == MemberKind::Method && answers != 1 {
            wrong(format!(
                "a method has exactly one `@answers`, this one has {answers}"
            ));
        }
        let factories = count_of(DocTagKind::Factory);
        match member.kind {
            MemberKind::Class { names_factory } => {
                if names_factory && factories != 1 {
                    wrong(format!(
                        "a class whose header names its factory has exactly one `@factory`, \
                         this one has {factories}"
                    ));
                } else if factories > 1 {
                    wrong(format!(
                        "a class has at most one `@factory`, this one has {factories}"
                    ));
                }
            }
            MemberKind::Slot | MemberKind::Method => {
                if factories > 0 {
                    wrong("only a class has a `@factory`".to_string());
                }
            }
        }
    }
    faults
}

/// What a member is, as far as its tags are concerned.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberKind {
    /// A class; `names_factory` when its header names a factory pattern.
    Class {
        names_factory: bool,
    },
    Slot,
    Method,
}

/// A class, slot or method of a module.
struct Member {
    /// A printable name.
    name: String,
    /// The offset its declaration starts at.
    start: usize,
    kind: MemberKind,
    /// The names its comment must describe, in order: a method's pattern
    /// parameters, a class's factory parameters, none for a slot.
    parameters: Vec<String>,
}

/// A pattern's parameter names, in order.
fn parameters_of(pattern: &MessagePattern) -> Vec<String> {
    match pattern {
        MessagePattern::Unary { .. } => Vec::new(),
        MessagePattern::Binary { parameter, .. } => vec![parameter.name.name.clone()],
        MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| part.parameter.name.name.clone())
            .collect(),
    }
}

/// Every `///` block of `source`. A `(* … *)` is no member's
/// documentation: the house style is the line comment, whose text a reader
/// such as a hover takes.
fn comments_of(source: &str) -> Vec<Comment> {
    ns_syntax::lexer::Lexer::new(source)
        .run_keeping_comments()
        .comments
        .iter()
        .filter(|comment| comment.kind == CommentKind::Line)
        .copied()
        .collect()
}

/// Each class, slot and method of `module`, the module's own class
/// included.
fn members_of(module: &Module) -> Vec<Member> {
    let mut members = Vec::new();
    let root = &module.root_class;
    members.push(class_member(format!("{} (class)", root.name.name), root));
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
                members.push(class_member(format!("{nested_path} (class)"), nested));
                pending.push((nested_path, nested));
            }
        }
    }
    members
}

/// The slots and methods of one side of a class.
fn collect_body(side: &str, body: &ClassBody, members: &mut Vec<Member>) {
    for slot in body.slots() {
        members.push(Member {
            name: format!("{side}>>{} (slot)", slot.name.name),
            start: (slot.span.start.min(slot.name.span.start)) as usize,
            kind: MemberKind::Slot,
            parameters: Vec::new(),
        });
    }
    for method in body.methods() {
        let (selector, selector_start) = selector_of(&method.pattern);
        members.push(Member {
            name: format!("{side}>>{selector}"),
            start: method.span.start.min(selector_start) as usize,
            kind: MemberKind::Method,
            parameters: parameters_of(&method.pattern),
        });
    }
}

/// A class as a member: its factory parameters are what its comment
/// describes.
fn class_member(name: String, class: &ClassDeclaration) -> Member {
    Member {
        name,
        start: class_start(class),
        kind: MemberKind::Class {
            names_factory: class.factory.is_some(),
        },
        parameters: class.factory.as_ref().map_or_else(Vec::new, parameters_of),
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

/// The comment that ends at or before `start` with only whitespace and
/// exactly one line break between it and `start` -- `\r\n` counted once, a
/// lone `\r` as one -- as the hover and the rendered view require: a comment
/// a blank line above a member is not the member's documentation there, so
/// it is not here either.
fn comment_above(source: &str, comments: &[Comment], start: usize) -> Option<Comment> {
    comments.iter().copied().find(|comment| {
        let end = comment.span.end as usize;
        end <= start && {
            let gap = &source[end..start];
            gap.chars().all(char::is_whitespace)
                && gap.matches('\n').count() + gap.matches('\r').count()
                    - gap.matches("\r\n").count()
                    == 1
        }
    })
}
