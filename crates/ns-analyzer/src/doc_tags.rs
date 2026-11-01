//! Warnings about a doc comment's tags: a tag the grammar does not know, a
//! tag without the argument it needs, a `@factory` in a comment that is no
//! class's, and a `@param` naming no parameter of what the comment
//! documents. The rule for which declaration a comment documents is the
//! hover's (`hover::doc_comment`, `hover::directly_above`), so a comment
//! warns about exactly the declaration it renders over.

use std::collections::HashMap;

use ns_syntax::ast::{self, Member};
use ns_syntax::doc_comment::{DocComment, DocTag, DocTagKind};
use ns_syntax::{Comment, CommentKind, Severity, Span};

use crate::analysis::Finding;
use crate::hover;

/// What a doc comment documents, and so what its `@param` and `@factory`
/// tags can be checked against.
enum Owner {
    Method {
        selector: String,
        parameters: Vec<String>,
    },
    Slot,
    /// A class, and its factory's selector and parameters when its header
    /// writes a factory.
    Class {
        factory: Option<(String, Vec<String>)>,
    },
}

impl Owner {
    /// The words that finish "the comment documents ...".
    fn description(&self) -> &'static str {
        match self {
            Owner::Method { .. } => "a method",
            Owner::Slot => "a slot",
            Owner::Class { factory: Some(_) } => "a class",
            Owner::Class { factory: None } => "a class without a factory",
        }
    }
}

/// The warnings for every doc comment of the module: a `Line` comment of two
/// or more lines, whichever declaration, if any, it documents.
pub fn findings(module: &ast::Module, comments: &[Comment], text: &str) -> Vec<Finding> {
    let owners = owners(module, text);
    let mut findings = Vec::new();
    for comment in comments {
        if comment.kind != CommentKind::Line || comment.text(text).lines().count() < 2 {
            continue;
        }
        let owner = owners.get(&comment.span);
        let documentation = DocComment::parse(*comment, text);
        for tag in &documentation.tags {
            check(tag, owner, &mut findings);
        }
    }
    findings
}

fn check(tag: &DocTag, owner: Option<&Owner>, findings: &mut Vec<Finding>) {
    let mut warn = |span: Span, message: String| {
        findings.push(Finding {
            span,
            message,
            severity: Severity::Warning,
        });
    };
    match &tag.kind {
        DocTagKind::Unknown(name) => warn(
            tag.name_span,
            format!(
                "unknown doc comment tag `@{name}`; the tags are @param, @answers, @signals, @see and @factory"
            ),
        ),
        DocTagKind::Factory => {
            if !matches!(owner, Some(Owner::Class { .. })) {
                let documents = owner.map_or("nothing", Owner::description);
                warn(
                    tag.name_span,
                    format!("`@factory` describes a class's factory: the comment documents {documents}"),
                );
            }
        }
        DocTagKind::Signals => {
            if tag.argument.is_none() {
                warn(
                    tag.name_span,
                    "`@signals` needs the class of the Error signalled".to_string(),
                );
            }
        }
        DocTagKind::Param => match (&tag.argument, owner) {
            (None, _) => warn(
                tag.name_span,
                "`@param` needs the name of a parameter".to_string(),
            ),
            (Some((name, span)), Some(owner)) => {
                let named = match owner {
                    Owner::Method {
                        selector,
                        parameters,
                    } => Some((selector, parameters)),
                    Owner::Class {
                        factory: Some((selector, parameters)),
                    } => Some((selector, parameters)),
                    Owner::Slot | Owner::Class { factory: None } => None,
                };
                match named {
                    Some((_, parameters)) if parameters.contains(name) => {}
                    Some((selector, _)) => warn(
                        *span,
                        format!("`{name}` is no parameter of `{selector}`"),
                    ),
                    None => warn(
                        *span,
                        format!(
                            "`{name}` is no parameter: the comment documents {}",
                            owner.description()
                        ),
                    ),
                }
            }
            (Some(_), None) => {}
        },
        DocTagKind::Answers | DocTagKind::See => {}
    }
}

/// Each doc comment (by its span) that documents a declaration of the
/// module, with that declaration's parameters. Nesting is walked with a
/// stack, so a deep nest costs no recursion.
fn owners(module: &ast::Module, text: &str) -> HashMap<Span, Owner> {
    let mut owners = HashMap::new();
    let mut add = |comments: &[ast::AttachedComment], start: u32, owner: Owner| {
        if let Some(span) = hover::doc_comment(comments)
            .filter(|comment| hover::directly_above(text, *comment, start))
        {
            owners.insert(span, owner);
        }
    };
    let mut classes = vec![&module.root_class];
    while let Some(class) = classes.pop() {
        add(
            &class.comments,
            class.span.start,
            Owner::Class {
                factory: class
                    .factory
                    .as_ref()
                    .map(|pattern| (selector_of(pattern), parameters_of(pattern))),
            },
        );
        for slot in &class.instance_side.header_slots {
            add(&slot.comments, slot.span.start, Owner::Slot);
        }
        let sides = std::iter::once(&class.instance_side).chain(class.class_side.as_ref());
        for side in sides {
            for member in &side.members {
                match member {
                    Member::Class(nested) => classes.push(nested),
                    Member::LazySlot(slot) => add(&slot.comments, slot.span.start, Owner::Slot),
                    Member::Method(method) => add(
                        &method.comments,
                        method.span.start,
                        Owner::Method {
                            selector: selector_of(&method.pattern),
                            parameters: parameters_of(&method.pattern),
                        },
                    ),
                }
            }
        }
    }
    owners
}

/// A pattern's full selector: `grow:by:` for a keyword pattern.
fn selector_of(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector, .. }
        | ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| format!("{}:", part.keyword.name))
            .collect(),
    }
}

fn parameters_of(pattern: &ast::MessagePattern) -> Vec<String> {
    match pattern {
        ast::MessagePattern::Unary { .. } => Vec::new(),
        ast::MessagePattern::Binary { parameter, .. } => vec![parameter.name.name.clone()],
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| part.parameter.name.name.clone())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn warnings(text: &str) -> Vec<(String, String)> {
        let parse = ns_syntax::parse_for_editor(text);
        findings(&parse.module, &parse.comments, text)
            .into_iter()
            .map(|finding| {
                assert_eq!(finding.severity, Severity::Warning);
                (finding.span.text(text).to_string(), finding.message)
            })
            .collect()
    }

    #[test]
    fn an_unknown_tag_is_a_warning() {
        let text = indoc! {"
            class Module = Object new () (
              /// Answers one.
              /// @retrun the one.
              public one = ( ^1 )
            ) : ()
        "};
        assert_eq!(
            warnings(text),
            [(
                "@retrun".to_string(),
                "unknown doc comment tag `@retrun`; the tags are @param, @answers, @signals, @see and @factory".to_string()
            )]
        );
    }

    #[test]
    fn a_factory_tag_outside_a_class_is_a_warning() {
        let text = indoc! {"
            class Module = Object new (
              |
              /// A slot.
              /// @factory No.
              public two = 2.
              |
            ) (
              /// A method.
              /// @factory No.
              public one = ( ^1 )
            ) : ()
            /// Nothing.
            /// @factory No.
        "};
        let found = warnings(text);
        let messages: Vec<&str> = found.iter().map(|(_, message)| message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "`@factory` describes a class's factory: the comment documents a slot",
                "`@factory` describes a class's factory: the comment documents a method",
                "`@factory` describes a class's factory: the comment documents nothing",
            ]
        );
        assert!(found.iter().all(|(span, _)| span == "@factory"));
    }

    #[test]
    fn a_tag_without_its_argument_is_a_warning() {
        let text = indoc! {"
            class Module = Object new () (
              /// Answers one.
              /// @param
              /// @signals
              public one = ( ^1 )
            ) : ()
        "};
        assert_eq!(
            warnings(text),
            [
                (
                    "@param".to_string(),
                    "`@param` needs the name of a parameter".to_string()
                ),
                (
                    "@signals".to_string(),
                    "`@signals` needs the class of the Error signalled".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_param_naming_no_parameter_is_a_warning() {
        let text = indoc! {"
            /// A module.
            /// @param platform fine.
            /// @param platfrom misspelt.
            class Module usingPlatform: platform = Object new () (
              /// Grows.
              /// @param amount fine.
              /// @param amout misspelt.
              public grow: amount by: step = ( ^amount )
              /// A class.
              /// @param any none.
              class Plain = Object new () () : ()
            ) : ()
        "};
        assert_eq!(
            warnings(text),
            [
                (
                    "platfrom".to_string(),
                    "`platfrom` is no parameter of `usingPlatform:`".to_string()
                ),
                (
                    "amout".to_string(),
                    "`amout` is no parameter of `grow:by:`".to_string()
                ),
                (
                    "any".to_string(),
                    "`any` is no parameter: the comment documents a class without a factory"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_param_above_a_slot_names_no_parameter() {
        let text = indoc! {"
            class Module = Object new (
              |
              /// A slot.
              /// @param amout none.
              public two = 2.
              |
            ) () : ()
        "};
        assert_eq!(
            warnings(text),
            [(
                "amout".to_string(),
                "`amout` is no parameter: the comment documents a slot".to_string()
            )]
        );
    }

    #[test]
    fn a_single_line_comment_is_not_checked() {
        let text = indoc! {"
            class Module = Object new () (
              /// @retrun nothing.
              public one = ( ^1 )
            ) : ()
        "};
        assert!(warnings(text).is_empty());
    }

    #[test]
    fn a_warning_is_placed_on_crlf_lines() {
        let text = "class Module = Object new () (\r\n  /// Grows.\r\n  /// @param amout x.\r\n  /// @retrun y.\r\n  public grow: amount = ( ^amount )\r\n) : ()\r\n";
        assert_eq!(
            warnings(text)
                .into_iter()
                .map(|(span, _)| span)
                .collect::<Vec<_>>(),
            ["amout", "@retrun"]
        );
    }

    #[test]
    fn a_correct_comment_has_none() {
        let text = indoc! {"
            /// A module.
            /// @factory Makes one.
            /// @param platform the platform.
            class Module usingPlatform: platform = Object new () (
              /// Grows.
              /// @param amount how much.
              /// @answers the total.
              /// @signals Error when negative.
              /// @see `other`
              public grow: amount = ( ^amount )
            ) : ()
        "};
        assert!(warnings(text).is_empty(), "{:?}", warnings(text));
    }
}
