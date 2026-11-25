//! An implicit-receiver keyword send `id: e` to a mutable local: a write
//! of the local, rewritten into the assignment `id:: e` is.
//!
//! §3.6 makes a local a slot of its activation and §5.7 sends an
//! implicit-receiver message to the current activation first, so `id: e`
//! sends the local's setter as `id:: e` does (§5.4.3). Both assign `e`,
//! and both answer `e`: a setter answers its receiver (§6.3.2), which for
//! a local would be the activation itself, an object no program holds but
//! through a mirror (§3.5), so the activation is never made and the send
//! is the assignment. A parameter and an immutable local declare no
//! setter, so the send passes them by, as `id:: e` does
//! (`ScopeStack::resolve_setter`). `docs/design/dispatch.md`, "Setter
//! sends", has the decision; `docs/spec/`'s appendix the difference from
//! §6.3.2.
//!
//! The resolver decides which sends are writes (`target`, then
//! `Resolver::resolve_local_setter`), and `rewrite` turns each into an
//! `ExpressionKind::Assign` before anything compiles the module, so every
//! reader of a resolved module -- both lowerings, the capture analysis,
//! the debugger's metadata -- sees the assignment it is. The editor's
//! resolution keeps the source's own shape (`resolve_for_editor`).

use std::collections::HashSet;

use ns_syntax::Span;
use ns_syntax::ast::{
    ClassBody, ClassDeclaration, CodeBody, Expression, ExpressionKind, Identifier, Member,
    MessageSend, Module, Selector, StatementKind,
};

/// The local an implicit-receiver send of `message` would write, named
/// and placed as the assignment's target: the keyword without its colon.
/// `None` for a message of another selector than a single keyword, and
/// for one the compiler built, which has no selector token to place the
/// target at -- no such message is sent to the implicit receiver.
pub fn target(message: &MessageSend) -> Option<Identifier> {
    let Selector::Keyword(selector) = &message.selector else {
        return None;
    };
    let name = selector.strip_suffix(':')?;
    if name.contains(':') || message.arguments.len() != 1 {
        return None;
    }
    let &[keyword] = message.selector_spans.as_slice() else {
        return None;
    };
    Some(Identifier {
        name: name.to_owned(),
        span: Span::new(keyword.start, keyword.end.saturating_sub(1)),
    })
}

/// A node still to be walked for writes.
enum Pending<'tree> {
    Class(&'tree mut ClassDeclaration),
    Side(&'tree mut ClassBody),
    Body(&'tree mut CodeBody),
    Expression(&'tree mut Expression),
    Message(&'tree mut MessageSend),
}

/// `module` with every send `writes` names -- by its span -- an
/// assignment.
pub(super) fn rewrite_module(module: &mut Module, writes: &HashSet<Span>) {
    if !writes.is_empty() {
        rewrite(Pending::Class(&mut module.root_class), writes);
    }
}

/// `body`, an evaluated expression's, with every send `writes` names an
/// assignment.
pub(super) fn rewrite_body(body: &mut CodeBody, writes: &HashSet<Span>) {
    if !writes.is_empty() {
        rewrite(Pending::Body(body), writes);
    }
}

/// One walk from a stack rather than by recursion: a chain of messages
/// nests as deep as the source is long.
fn rewrite(root: Pending<'_>, writes: &HashSet<Span>) {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        match node {
            Pending::Class(class) => {
                for clause in class
                    .superclass
                    .iter_mut()
                    .chain(&mut class.mixin_applications)
                {
                    pending.extend(clause.messages.iter_mut().map(Pending::Message));
                }
                pending.push(Pending::Side(&mut class.instance_side));
                pending.extend(class.class_side.as_mut().map(Pending::Side));
            }
            Pending::Side(side) => {
                for slot in &mut side.header_slots {
                    pending.extend(slot.initializer.as_mut().map(Pending::Expression));
                }
                for statement in &mut side.initializer_expressions {
                    let (StatementKind::Expression(expression) | StatementKind::Return(expression)) =
                        &mut statement.kind;
                    pending.push(Pending::Expression(expression));
                }
                for member in &mut side.members {
                    match member {
                        Member::Class(class) => pending.push(Pending::Class(class)),
                        Member::LazySlot(slot) => {
                            pending.extend(slot.initializer.as_mut().map(Pending::Expression));
                        }
                        Member::Method(method) => pending.push(Pending::Body(&mut method.body)),
                    }
                }
            }
            Pending::Body(body) => {
                for temporary in &mut body.temporaries {
                    pending.extend(temporary.initializer.as_mut().map(Pending::Expression));
                }
                for statement in &mut body.statements {
                    let (StatementKind::Expression(expression) | StatementKind::Return(expression)) =
                        &mut statement.kind;
                    pending.push(Pending::Expression(expression));
                }
            }
            Pending::Expression(expression) => {
                assign_if_a_write(expression, writes);
                match &mut expression.kind {
                    ExpressionKind::Identifier(_) | ExpressionKind::Literal(_) => {}
                    ExpressionKind::Send(send) => {
                        pending.push(Pending::Expression(&mut send.receiver));
                        pending.push(Pending::Message(&mut send.message));
                    }
                    ExpressionKind::Cascade(cascade) => {
                        pending.push(Pending::Expression(&mut cascade.receiver));
                        pending.push(Pending::Message(&mut cascade.first));
                        pending.extend(cascade.rest.iter_mut().map(Pending::Message));
                    }
                    ExpressionKind::Assign { value, .. } => {
                        pending.push(Pending::Expression(value));
                    }
                    ExpressionKind::Block(block) => pending.push(Pending::Body(&mut block.body)),
                    ExpressionKind::DynamicArray { elements, .. } => {
                        pending.extend(elements.iter_mut().map(Pending::Expression));
                    }
                }
            }
            Pending::Message(message) => {
                pending.extend(message.arguments.iter_mut().map(Pending::Expression));
            }
        }
    }
}

/// Makes `expression` the assignment it is when it is a send `writes`
/// names, its message's comments the expression's.
fn assign_if_a_write(expression: &mut Expression, writes: &HashSet<Span>) {
    let ExpressionKind::Send(send) = &expression.kind else {
        return;
    };
    if !writes.contains(&send.span) {
        return;
    }
    let target = target(&send.message)
        .expect("internal: the resolver records only a send of one keyword as a write");
    let span = send.span;
    let placeholder = ExpressionKind::DynamicArray {
        elements: Vec::new(),
        span,
    };
    let ExpressionKind::Send(mut send) = std::mem::replace(&mut expression.kind, placeholder)
    else {
        unreachable!("internal: matched as a send above");
    };
    // `Send` drops itself link by link, so its parts are taken, not moved.
    let value = send
        .message
        .arguments
        .pop()
        .expect("internal: a send of one keyword has one argument");
    expression.comments.append(&mut send.message.comments);
    expression.kind = ExpressionKind::Assign {
        target,
        value: Box::new(value),
        span,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one statement of method `run`'s body in `source`'s root.
    fn statement_of_run(module: &Module) -> &Expression {
        let Some(Member::Method(method)) = module.root_class.instance_side.members.first() else {
            panic!("the fixture's first member is `run`");
        };
        let (StatementKind::Expression(expression) | StatementKind::Return(expression)) =
            &method.body.statements[0].kind;
        expression
    }

    #[test]
    fn a_write_becomes_the_assignment_its_target_named_by_the_keyword() {
        let source = indoc::indoc! {"
            class Module = Object new (
            ) (
              run = ( | value | value: 3 + 4 )
            ) : ()
        "};
        let mut module = ns_syntax::parse(source).expect("the fixture parses");
        let ExpressionKind::Send(send) = &statement_of_run(&module).kind else {
            panic!("the statement is a send");
        };
        let written = target(&send.message).expect("a send of one keyword");
        let writes = HashSet::from([send.span]);
        rewrite_module(&mut module, &writes);
        let ExpressionKind::Assign { target, value, .. } = &statement_of_run(&module).kind else {
            panic!("the statement is an assignment");
        };
        assert_eq!(target, &written);
        assert_eq!(
            &source[target.span.start as usize..target.span.end as usize],
            "value"
        );
        assert!(
            matches!(value.kind, ExpressionKind::Send(_)),
            "`3 + 4` is the value"
        );
    }

    #[test]
    fn only_a_send_of_one_keyword_names_a_target() {
        let source = indoc::indoc! {"
            class Module = Object new (
            ) (
              run = ( at: 1 put: 2 )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let ExpressionKind::Send(send) = &statement_of_run(&module).kind else {
            panic!("the statement is a send");
        };
        assert_eq!(target(&send.message), None);
    }
}
