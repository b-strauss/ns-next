//! An expression the debugger evaluates in a stopped frame, compiled as the
//! body of a block whose outer frame is that frame
//! (`docs/design/debugger.md`, decision 8): its marked labels are the
//! block's parameters, and a variable of the frame, which the resolver
//! binds as `Binding::FrameVariable`, is read and written where the stopped
//! frame keeps it, through the instructions a closure reaches its
//! creator's registers with. The variable is never copied, so an
//! assignment reaches the frame and a block the expression makes reads the
//! frame's current value whenever it runs.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::{Severity, Span};

use super::Lowering;
use super::capture::{self, BodyKey, RootBody};
use super::lower::{Enclosing, Lowerer, ScopeKind};
use crate::Symbol;
use crate::bytecode::{CodeKind, CompiledCode};
use crate::debugger::metadata::VariablePlace;
use crate::resolver::FrameVariable;
use crate::types::Binding;

/// `text` parsed as a code body standing alone -- temporaries, then
/// statements -- or the parser's first error.
pub(crate) fn parse_expression(text: &str) -> Result<ast::CodeBody, String> {
    ns_syntax::parse_code_body(text).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .find(|diagnostic| diagnostic.severity == Severity::Error)
            .map_or_else(
                || "the expression does not parse".to_string(),
                |diagnostic| diagnostic.message,
            )
    })
}

/// `expression`, resolved by `resolver::resolve_evaluation` into
/// `bindings`, compiled under `lowering` as a block of `parameters.len()`
/// arguments -- the marked labels it names, in order -- that answers its
/// last statement's value. `variables` is the table the bindings index:
/// each `Binding::FrameVariable` is reached where its place says the
/// stopped frame reads it, one hop further out, since the stopped frame is
/// the block's outer frame; a block the expression makes reaches it one
/// hop further for each frame between. So the body's `outer_reach` is at
/// least 1 whenever it names a frame variable, and a block it makes that
/// names one reaches 2 or more, for which the frame keeps its registers
/// (`debugger::evaluation::begin_in`).
pub(crate) fn compile_evaluation(
    expression: &ast::CodeBody,
    bindings: &Rc<HashMap<Span, Binding>>,
    variables: &[FrameVariable],
    parameters: &[Symbol],
    lowering: Lowering,
) -> Rc<CompiledCode> {
    let places: Rc<[VariablePlace]> = variables.iter().map(|variable| variable.place).collect();
    let code = match lowering {
        Lowering::Reference => {
            super::reference::compile_evaluation(expression, bindings, places, parameters.len())
        }
        Lowering::Optimized => compile_optimized(expression, bindings, places, parameters.len()),
    };
    Rc::new(code)
}

/// `compile_evaluation` under the optimized lowering: the expression is the
/// root of its code family, analysed for closure conversion as a method's
/// body is -- a frame variable is no local of the family, so nothing
/// copies it -- and lowered as a block's body below one by-reference frame
/// scope that stands for the stopped frame. That scope is the outermost,
/// at position 0, which `Lowerer::frame_variable_access` counts its hops
/// to; no binding names a local of it, so its key is never looked up.
fn compile_optimized(
    expression: &ast::CodeBody,
    bindings: &Rc<HashMap<Span, Binding>>,
    places: Rc<[VariablePlace]>,
    argument_count: usize,
) -> CompiledCode {
    let captures = Rc::new(capture::analyze(
        RootBody {
            argument_count,
            temporaries: &expression.temporaries,
            slot_initializers: Vec::new(),
            statements: &expression.statements,
        },
        bindings,
    ));
    let mut lowerer = Lowerer::new(
        CodeKind::Block,
        BodyKey::Root,
        argument_count,
        &expression.temporaries,
        bindings,
        captures,
        false,
        Enclosing {
            scopes: vec![ScopeKind::Frame {
                key: BodyKey::Root,
                by_reference: true,
            }],
            frame_variables: Some(places),
            ..Enclosing::default()
        },
        0,
        None,
    );
    lowerer.lower_local_initializers(&expression.temporaries);
    lowerer.lower_body(&expression.statements);
    lowerer.finish(expression.span)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::rc::Rc;

    use indoc::indoc;
    use ns_syntax::ast::{self, ExpressionKind, StatementKind};

    use super::{compile_evaluation, parse_expression};
    use crate::Symbol;
    use crate::bytecode::{CompiledCode, Instruction};
    use crate::compiler::Lowering;
    use crate::debugger::metadata::{MetadataCollector, VariablePlace, VectorHolder};
    use crate::resolver::sends::EnclosingClass;
    use crate::resolver::{
        Assignability, EvaluationScope, FrameVariable, own_member_names, resolve_evaluation,
    };
    use crate::types::Binding;

    /// A frame stopped in a method: the variables its metadata names, and
    /// the member names of the classes around the method, innermost first.
    struct Stopped {
        variables: Vec<FrameVariable>,
        classes: Vec<(Symbol, HashSet<Symbol>)>,
    }

    /// The first method of the class `path` names in `source` -- the root
    /// class's name first, then each nested class's -- compiled under
    /// `lowering` with a collector, as the debugger compiles a stopped
    /// body again. A parameter is not assignable; every temporary is.
    fn stopped(source: &str, path: &[&str], lowering: Lowering) -> Stopped {
        let module = ns_syntax::parse(source).expect("the module parses");
        let resolved = match crate::resolve(
            module,
            Rc::from(source),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the module does not resolve"),
        };
        let mut chain: Vec<&ast::ClassDeclaration> = vec![&resolved.module.root_class];
        for name in &path[1..] {
            let outer = chain.last().expect("the root class");
            let nested = outer
                .instance_side
                .nested_classes()
                .find(|declaration| declaration.name.name == *name)
                .expect("the nested class");
            chain.push(nested);
        }
        let declaration = chain.last().expect("a class");
        let method = declaration
            .instance_side
            .methods()
            .next()
            .expect("a method");
        let collector = MetadataCollector::new();
        let code =
            crate::compiler::compile_method(method, &resolved.bindings, lowering, Some(&collector));
        let metadata = collector.take(&code).expect("the method's metadata");
        let parameters = crate::resolver::pattern_parameter_names(&method.pattern);
        let variables = metadata
            .variables
            .iter()
            .map(|entry| FrameVariable {
                name: entry.name,
                place: entry.place,
                assignability: if parameters.contains(&entry.name) {
                    Assignability::NotAssignable
                } else {
                    Assignability::Assignable
                },
                available: true,
            })
            .collect();
        let classes = chain
            .iter()
            .rev()
            .map(|declaration| {
                (
                    Symbol::intern(&declaration.name.name),
                    own_member_names(declaration),
                )
            })
            .collect();
        Stopped { variables, classes }
    }

    /// `expression` resolved in `stop`'s scope with `labels`, and its
    /// bindings.
    fn resolved(
        stop: &Stopped,
        expression: &str,
        labels: &[Symbol],
    ) -> Result<
        (
            ast::CodeBody,
            Rc<std::collections::HashMap<ns_syntax::Span, Binding>>,
        ),
        String,
    > {
        let mut body = parse_expression(expression)?;
        let classes: Vec<EnclosingClass<'_>> = stop
            .classes
            .iter()
            .map(|(name, members)| EnclosingClass {
                name: *name,
                members,
            })
            .collect();
        let bindings = resolve_evaluation(
            &mut body,
            &EvaluationScope {
                classes: &classes,
                variables: &stop.variables,
                labels,
            },
        )?;
        Ok((body, bindings))
    }

    /// `expression` compiled in `stop`'s scope with `labels`.
    fn compiled(
        stop: &Stopped,
        expression: &str,
        labels: &[Symbol],
        lowering: Lowering,
    ) -> Rc<CompiledCode> {
        let (body, bindings) = resolved(stop, expression, labels).expect("the expression resolves");
        compile_evaluation(&body, &bindings, &stop.variables, labels, lowering)
    }

    /// The register `name` sits in, in the stopped frame itself.
    fn register_of(stop: &Stopped, name: &str) -> u16 {
        let entry = stop
            .variables
            .iter()
            .find(|variable| variable.name == Symbol::intern(name))
            .expect("the variable");
        match entry.place {
            VariablePlace::Register(register) => register,
            VariablePlace::Outer { .. } | VariablePlace::Vector { .. } => {
                panic!("{name} is not in a register of the frame")
            }
        }
    }

    /// The binding of the first statement's expression, an identifier.
    fn binding_of_the_identifier(
        body: &ast::CodeBody,
        bindings: &std::collections::HashMap<ns_syntax::Span, Binding>,
    ) -> Option<Binding> {
        let StatementKind::Expression(expression) = &body.statements[0].kind else {
            panic!("an expression statement");
        };
        let ExpressionKind::Identifier(identifier) = &expression.kind else {
            panic!("an identifier");
        };
        bindings.get(&identifier.span).copied()
    }

    const COUNTER: &str = indoc! {"
        class Counter = Object new (
        ) (
          step: argument = ( | count ::= 0. | count:: count + argument. ^count )
        ) : ()
    "};

    #[test]
    fn a_frame_variable_reads_through_the_outer_frame() {
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            let stop = stopped(COUNTER, &["Counter"], lowering);
            let code = compiled(&stop, "count + 1", &[], lowering);
            let count = register_of(&stop, "count");
            assert!(
                code.instructions().iter().any(|instruction| matches!(
                    instruction,
                    Instruction::LoadOuterLocal { depth: 1, index, .. } if *index == count
                )),
                "{lowering:?}: {:?}",
                code.instructions()
            );
            assert_eq!(code.outer_reach, 1, "{lowering:?}");
            assert_eq!(code.argument_count, 0);
        }
    }

    #[test]
    fn an_assignment_writes_the_frame_variable() {
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            let stop = stopped(COUNTER, &["Counter"], lowering);
            let code = compiled(&stop, "count:: 0", &[], lowering);
            let count = register_of(&stop, "count");
            assert!(
                code.instructions().iter().any(|instruction| matches!(
                    instruction,
                    Instruction::StoreOuterLocal { depth: 1, index, .. } if *index == count
                )),
                "{lowering:?}: {:?}",
                code.instructions()
            );
        }
    }

    // spec: semantics.send.local-setter
    #[test]
    fn a_keyword_send_writes_the_frame_variable_as_its_assignment_does() {
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            let stop = stopped(COUNTER, &["Counter"], lowering);
            let code = compiled(&stop, "count: 0", &[], lowering);
            let count = register_of(&stop, "count");
            assert!(
                code.instructions().iter().any(|instruction| matches!(
                    instruction,
                    Instruction::StoreOuterLocal { depth: 1, index, .. } if *index == count
                )),
                "{lowering:?}: {:?}",
                code.instructions()
            );
        }
    }

    #[test]
    fn a_vector_variable_is_read_through_its_vector() {
        let source = indoc! {"
            class Counter = Object new (
            ) (
              run: holder = ( | count ::= 0. | holder value: [count:: count + 1]. ^count )
            ) : ()
        "};
        let stop = stopped(source, &["Counter"], Lowering::Optimized);
        let count = stop
            .variables
            .iter()
            .find(|variable| variable.name == Symbol::intern("count"))
            .expect("count");
        let VariablePlace::Vector {
            vector: VectorHolder::Register(vector_register),
            slot,
        } = count.place
        else {
            panic!("count lives in the frame's vector");
        };
        let code = compiled(&stop, "count", &[], Lowering::Optimized);
        let instructions = code.instructions();
        let held = instructions
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::LoadOuterLocal {
                    destination,
                    depth: 1,
                    index,
                } if *index == vector_register => Some(*destination),
                _ => None,
            })
            .expect("the vector read through the outer frame");
        assert!(
            instructions.iter().any(|instruction| matches!(
                instruction,
                Instruction::LoadVector { vector, index, .. } if *vector == held && *index == slot
            )),
            "{instructions:?}"
        );
        assert_eq!(code.outer_reach, 1);
    }

    #[test]
    fn a_block_in_the_expression_reaches_the_frame_one_hop_further() {
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            let stop = stopped(COUNTER, &["Counter"], lowering);
            let code = compiled(&stop, "[count:: count + 1] value", &[], lowering);
            let count = register_of(&stop, "count");
            let block = code
                .instructions()
                .into_iter()
                .find_map(|instruction| match instruction {
                    Instruction::MakeClosure { code, .. } => Some(code),
                    _ => None,
                })
                .expect("the block");
            let instructions = block.instructions();
            assert!(
                instructions.iter().any(|instruction| matches!(
                    instruction,
                    Instruction::LoadOuterLocal { depth: 2, index, .. } if *index == count
                )) && instructions.iter().any(|instruction| matches!(
                    instruction,
                    Instruction::StoreOuterLocal { depth: 2, index, .. } if *index == count
                )),
                "{lowering:?}: {instructions:?}"
            );
            assert_eq!(block.outer_reach, 2, "{lowering:?}");
            assert_eq!(code.outer_reach, 1, "{lowering:?}");
        }
    }

    #[test]
    fn a_label_is_not_assignable() {
        let stop = stopped(COUNTER, &["Counter"], Lowering::Optimized);
        let label = Symbol::intern("first_DebugLabel");
        assert_eq!(
            resolved(&stop, "first_DebugLabel:: 1", &[label]).map(|_| ()),
            Err("first_DebugLabel is not assignable".to_string())
        );
    }

    #[test]
    fn a_parameter_is_not_assignable() {
        let stop = stopped(COUNTER, &["Counter"], Lowering::Optimized);
        assert_eq!(
            resolved(&stop, "argument:: 1", &[]).map(|_| ()),
            Err("argument is not assignable".to_string())
        );
    }

    // spec: semantics.send.local-setter
    #[test]
    fn a_keyword_send_to_a_parameter_is_refused_as_its_assignment_is() {
        let stop = stopped(COUNTER, &["Counter"], Lowering::Optimized);
        assert_eq!(
            resolved(&stop, "argument: 1", &[]).map(|_| ()),
            Err("argument is not assignable".to_string())
        );
    }

    #[test]
    fn an_unknown_name_is_an_implicit_receiver_send() {
        let stop = stopped(COUNTER, &["Counter"], Lowering::Optimized);
        let (body, bindings) = resolved(&stop, "frobnicate", &[]).expect("resolves");
        assert_eq!(
            binding_of_the_identifier(&body, &bindings),
            Some(Binding::LexicalSend { depth: 0 })
        );
    }

    #[test]
    fn a_slot_of_an_enclosing_class_is_a_lexical_send_at_its_depth() {
        let source = indoc! {"
            class Ledger = Object new (
              | total ::= 0. |
            ) (
              class Entry = Object new (
              ) (
                amount = ( ^1 )
              ) : ()
            ) : ()
        "};
        let stop = stopped(source, &["Ledger", "Entry"], Lowering::Optimized);
        let (body, bindings) = resolved(&stop, "total", &[]).expect("resolves");
        assert_eq!(
            binding_of_the_identifier(&body, &bindings),
            Some(Binding::LexicalSend { depth: 1 })
        );
    }

    #[test]
    fn a_label_is_a_parameter() {
        let stop = stopped(COUNTER, &["Counter"], Lowering::Optimized);
        let label = Symbol::intern("first_DebugLabel");
        let (body, bindings) =
            resolved(&stop, "first_DebugLabel size", &[label]).expect("resolves");
        let StatementKind::Expression(expression) = &body.statements[0].kind else {
            panic!("an expression statement");
        };
        let ExpressionKind::Send(send) = &expression.kind else {
            panic!("a send");
        };
        let ExpressionKind::Identifier(receiver) = &send.receiver.kind else {
            panic!("an identifier receiver");
        };
        assert_eq!(bindings.get(&receiver.span), Some(&Binding::Local(0)));
        let code = compiled(
            &stop,
            "first_DebugLabel size",
            &[label],
            Lowering::Optimized,
        );
        assert_eq!(code.argument_count, 1);
        assert!(
            code.instructions().iter().any(|instruction| matches!(
                instruction,
                Instruction::SendPublic {
                    receiver_register: 0,
                    ..
                }
            )),
            "{:?}",
            code.instructions()
        );
    }

    #[test]
    fn a_caret_is_refused() {
        let stop = stopped(COUNTER, &["Counter"], Lowering::Optimized);
        assert_eq!(
            resolved(&stop, "^count", &[]).map(|_| ()),
            Err("a ^ cannot leave an evaluation".to_string())
        );
    }

    #[test]
    fn a_caret_in_a_nested_block_is_refused() {
        let stop = stopped(COUNTER, &["Counter"], Lowering::Optimized);
        assert_eq!(
            resolved(&stop, "[^count]", &[]).map(|_| ()),
            Err("a ^ cannot leave an evaluation".to_string())
        );
    }
}
