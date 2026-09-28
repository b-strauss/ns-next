//! The capture analysis of closure conversion (`docs/design/execution.md`,
//! "Closure conversion"): for one code family -- a root body and
//! every block literal in it -- which of a body's locals live in registers
//! and which in its indirection vector, and what each real block is handed
//! when it is created.
//!
//! A pre-pass of the optimized lowering, not of the resolver: only the
//! compiler knows which blocks `control_flow::recognize` inlines, and a
//! variable read only inside an inlined block is not captured at all. The
//! reference lowering reaches every variable by reference and never calls
//! this.

use std::collections::HashMap;

use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::Span;

use super::control_flow::{self, InlineForm};
use super::simultaneous;
use crate::types::Binding;

/// A body of one code family: its root, or a block literal named by its span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BodyKey {
    Root,
    Block(Span),
}

/// Where one of a body's own locals lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Register,
    /// Slot `0..vector_size` of the body's indirection vector.
    Vector(u16),
}

/// One entry of a block's copied values: a variable's value, or the whole
/// indirection vector of the body that declares the variables it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Captured {
    Variable { body: BodyKey, index: u16 },
    Vector { body: BodyKey },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyCaptures {
    /// One entry per local, by resolver index: the parameters, then the
    /// temporaries, in declaration order.
    pub(crate) places: Vec<Place>,
    pub(crate) vector_size: u16,
    /// One entry per local: whether a block of an inlined conditional in this
    /// body assigns it, and so whether the fallback closure of that
    /// conditional can write its register at any later time ("Fallback
    /// blocks"). What the protective copies read
    /// (`docs/design/execution.md`, "Fewer protective copies"): a send is a
    /// write of a local's register only for a local this names.
    pub(crate) branch_assigned: Vec<bool>,
    /// What this block is handed when it is created, in order; its registers
    /// are `argument_count + position`. Empty for `BodyKey::Root`.
    pub(crate) copied: Vec<Captured>,
}

#[derive(Debug, Default)]
pub(crate) struct Captures {
    bodies: HashMap<BodyKey, BodyCaptures>,
}

impl Captures {
    pub(crate) fn body(&self, key: BodyKey) -> &BodyCaptures {
        self.bodies
            .get(&key)
            .unwrap_or_else(|| panic!("internal: no capture analysis for body {key:?}"))
    }

    #[cfg(test)]
    pub(crate) fn keys(&self) -> impl Iterator<Item = BodyKey> + '_ {
        self.bodies.keys().copied()
    }
}

/// A root body, in the order the optimized lowering lowers it: the local
/// initializers, then a slot initializer per initialized slot (an instance
/// initializer only), then the statements.
pub(crate) struct RootBody<'a> {
    pub(crate) argument_count: u16,
    pub(crate) temporaries: &'a [ast::SlotDeclaration],
    pub(crate) slot_initializers: Vec<&'a Expression>,
    pub(crate) statements: &'a [Statement],
}

pub(crate) fn analyze(root: RootBody<'_>, bindings: &HashMap<Span, Binding>) -> Captures {
    let mut walker = Walker {
        bindings,
        scopes: Vec::new(),
        position: 0,
        next_loop: 0,
        creation: HashMap::new(),
        facts: HashMap::new(),
        locals: HashMap::new(),
        needs: HashMap::new(),
    };
    walker.declare(BodyKey::Root, root.argument_count, root.temporaries);
    walker.scopes.push(Scope::Frame { key: BodyKey::Root });
    walker.local_initializers(BodyKey::Root, root.argument_count, root.temporaries);
    for initializer in &root.slot_initializers {
        walker.expression(initializer);
    }
    walker.statements(root.statements);
    walker.scopes.pop();
    walker.finish()
}

#[derive(Clone, Copy)]
enum Scope {
    Frame {
        key: BodyKey,
    },
    Inlined {
        /// A block of an inlined `ifTrue:`/`ifFalse:`/`and:`/`or:`, whose
        /// fallback path builds a real closure out of the same block.
        conditional: bool,
        /// The inlined loop this block belongs to, if any.
        loop_number: Option<u32>,
    },
}

/// A local, named by the body that declares it and its resolver index.
type LocalKey = (BodyKey, u16);

#[derive(Default)]
struct Facts {
    /// A block other than the declaring body assigns it: always shared.
    written_by_block: bool,
    /// The earliest position at which a real block that reads it is created.
    first_capture: Option<u32>,
    /// The inlined loops of the declaring body that were open at each such
    /// creation.
    capture_loops: Vec<u32>,
    writes: Vec<Write>,
}

struct Write {
    position: u32,
    loops: Vec<u32>,
    /// Inside a block of an inlined conditional, whose fallback closure can
    /// run at any later time.
    conditional: bool,
}

struct Walker<'a> {
    bindings: &'a HashMap<Span, Binding>,
    /// Mirrors the resolver's scope chain: every block literal is a scope,
    /// inlined or not, so a `Binding`'s depth indexes it directly.
    scopes: Vec<Scope>,
    position: u32,
    next_loop: u32,
    /// Per real block: where its `MakeClosure` stands, and which inlined
    /// loops of its creating frame were open there.
    creation: HashMap<BodyKey, (u32, Vec<u32>)>,
    facts: HashMap<LocalKey, Facts>,
    /// Per body: whether each local is mutable, by resolver index.
    locals: HashMap<BodyKey, Vec<bool>>,
    /// Per real block: the variables of enclosing bodies it needs, in first
    /// reference order, including everything its own nested blocks need.
    needs: HashMap<BodyKey, Vec<LocalKey>>,
}

impl<'a> Walker<'a> {
    fn declare(&mut self, key: BodyKey, argument_count: u16, temporaries: &[ast::SlotDeclaration]) {
        // A parameter is immutable (§3.6), and so is a local slot declared
        // with `=`; only those can never need the vector. A simultaneous
        // temporary (§6.3.2) is written twice whatever its declaration says
        // -- its future, then its value at the clause's end -- and a
        // sibling's future block may have read it in between.
        let mutable = std::iter::repeat_n(false, argument_count as usize)
            .chain(temporaries.iter().map(|temporary| {
                temporary.mutability != ast::SlotMutability::Immutable || temporary.simultaneous
            }))
            .collect();
        self.locals.insert(key, mutable);
        self.needs.insert(key, Vec::new());
    }

    /// The innermost frame's key, and its position in `scopes`.
    fn current_frame(&self) -> (BodyKey, usize) {
        for (position, scope) in self.scopes.iter().enumerate().rev() {
            if let Scope::Frame { key } = scope {
                return (*key, position);
            }
        }
        unreachable!("internal: a capture walk outside every frame")
    }

    /// The loops of the frame at `frame_position` that are open here.
    fn open_loops(&self, frame_position: usize) -> Vec<u32> {
        self.scopes[frame_position + 1..]
            .iter()
            .filter_map(|scope| match scope {
                Scope::Inlined { loop_number, .. } => *loop_number,
                Scope::Frame { .. } => None,
            })
            .collect()
    }

    /// Whether anything between the frame at `frame_position` and here is a
    /// block of an inlined conditional.
    fn inside_conditional(&self, frame_position: usize) -> bool {
        self.scopes[frame_position + 1..].iter().any(|scope| {
            matches!(
                scope,
                Scope::Inlined {
                    conditional: true,
                    ..
                }
            )
        })
    }

    /// The scope that declares the local a binding at `depth` names.
    fn declaring(&self, depth: u16) -> (BodyKey, usize) {
        let position = self.scopes.len() - 1 - depth as usize;
        match self.scopes[position] {
            Scope::Frame { key } => (key, position),
            Scope::Inlined { .. } => unreachable!(
                "internal: a local declared by an inlined block (the inlining gate admits none)"
            ),
        }
    }

    fn binding_of(&self, span: Span) -> Option<(u16, u16)> {
        match self.bindings.get(&span) {
            Some(Binding::Local(index)) => Some((0, *index)),
            Some(Binding::OuterLocal { depth, index }) => Some((*depth, *index)),
            _ => None,
        }
    }

    /// Every real block between the frame that declares a local and here
    /// needs that local -- a middle block carries what an inner one needs --
    /// and the outermost of those blocks is where it is captured. Answers
    /// that outermost block, or `None` when the reference stands in the
    /// declaring frame itself.
    fn note_need(
        &mut self,
        declaring_key: BodyKey,
        index: u16,
        declaring_position: usize,
    ) -> Option<BodyKey> {
        let blocks: Vec<BodyKey> = self.scopes[declaring_position + 1..]
            .iter()
            .filter_map(|scope| match scope {
                Scope::Frame { key } => Some(*key),
                Scope::Inlined { .. } => None,
            })
            .collect();
        let outermost = blocks.first().copied()?;
        for block in &blocks {
            let needs = self
                .needs
                .get_mut(block)
                .expect("internal: undeclared block");
            if !needs.contains(&(declaring_key, index)) {
                needs.push((declaring_key, index));
            }
        }
        Some(outermost)
    }

    /// A read of a local: every real block between its declaring frame and
    /// here needs it, and the outermost of those blocks is where it is
    /// captured.
    fn reference(&mut self, identifier: &ast::Identifier) {
        let Some((depth, index)) = self.binding_of(identifier.span) else {
            return;
        };
        let (declaring_key, declaring_position) = self.declaring(depth);
        let Some(outermost) = self.note_need(declaring_key, index, declaring_position) else {
            return;
        };
        let (position, loops) = self
            .creation
            .get(&outermost)
            .cloned()
            .expect("internal: a block referenced before it was entered");
        let facts = self.facts.entry((declaring_key, index)).or_default();
        facts.first_capture = Some(match facts.first_capture {
            Some(earlier) => earlier.min(position),
            None => position,
        });
        for number in loops {
            if !facts.capture_loops.contains(&number) {
                facts.capture_loops.push(number);
            }
        }
    }

    /// A write of a local, from wherever the walk currently stands.
    fn write(&mut self, declaring_key: BodyKey, index: u16, declaring_position: usize) {
        let (current_key, current_position) = self.current_frame();
        self.position += 1;
        if current_key != declaring_key {
            // A block that only ever *writes* an enclosing local still needs
            // to reach it, so it is handed the vector exactly as a reading
            // block would be -- and every block between it and the declaring
            // frame has to carry that vector to it.
            self.note_need(declaring_key, index, declaring_position);
            self.facts
                .entry((declaring_key, index))
                .or_default()
                .written_by_block = true;
            return;
        }
        let write = Write {
            position: self.position,
            loops: self.open_loops(current_position),
            conditional: self.inside_conditional(declaring_position),
        };
        self.facts
            .entry((declaring_key, index))
            .or_default()
            .writes
            .push(write);
    }

    fn assign(&mut self, target: &ast::Identifier) {
        let Some((depth, index)) = self.binding_of(target.span) else {
            return;
        };
        let (declaring_key, declaring_position) = self.declaring(depth);
        self.write(declaring_key, index, declaring_position);
    }

    fn local_initializers(
        &mut self,
        key: BodyKey,
        argument_count: u16,
        temporaries: &[ast::SlotDeclaration],
    ) {
        let frame_position = self.scopes.len() - 1;
        for (offset, temporary) in temporaries.iter().enumerate() {
            let Some(initializer) = &temporary.initializer else {
                continue;
            };
            // A simultaneous initializer is its future's block, walked as the
            // real block the lowering makes of it, so what it reads is
            // captured there (`simultaneous::future_block`).
            match simultaneous::future_block(temporary) {
                Some(block) => self.expression(&block),
                None => self.expression(initializer),
            }
            self.write(key, argument_count + offset as u16, frame_position);
        }
        // The clause's end: each simultaneous temporary written again with
        // its resolved value, in declaration order, after every future's
        // block was created.
        for (offset, temporary) in temporaries.iter().enumerate() {
            if temporary.simultaneous && temporary.initializer.is_some() {
                self.write(key, argument_count + offset as u16, frame_position);
            }
        }
    }

    fn statements(&mut self, statements: &[Statement]) {
        for statement in statements {
            match statement {
                Statement::Expression(expression) | Statement::Return(expression) => {
                    self.expression(expression)
                }
            }
        }
    }

    fn expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Identifier(identifier) => self.reference(identifier),
            Expression::Literal(_) => {}
            Expression::Assign { target, value, .. } => {
                self.expression(value);
                self.assign(target);
            }
            Expression::Send(send) => self.send(send),
            Expression::Cascade(cascade) => {
                self.expression(&cascade.receiver);
                for message in std::iter::once(&cascade.first).chain(cascade.rest.iter()) {
                    for argument in &message.arguments {
                        self.expression(argument);
                    }
                }
            }
            Expression::Block(block) => self.real_block(block),
            Expression::DynamicArray { elements, .. } => {
                for element in elements {
                    self.expression(element);
                }
            }
        }
    }

    /// Mirrors `lower_send` through the shared gate: only an ordinary
    /// explicit-receiver send is offered to `recognize`, and a lexical or
    /// `super` send has no receiver expression to walk.
    fn send(&mut self, send: &ast::Send) {
        if control_flow::is_ordinary_send(self.bindings, send) {
            // A mirror send's failure block is lowered inline
            // (`lower_mirror_send`), so it is no closure here either. It is
            // not `conditional`: that flag says a fallback closure built out
            // of the block can write this frame later, and a `SendMirror`
            // builds none -- a receiver that is not the mirror is an internal
            // error, not a send (`interpreter::mirror::send_inlined`).
            if let Some((_, block)) = control_flow::recognize_mirror_send(self.bindings, send) {
                self.expression(&send.receiver);
                let arguments = &send.message.arguments;
                for argument in &arguments[..arguments.len() - 1] {
                    self.expression(argument);
                }
                self.inlined_block(block, false, None);
                return;
            }
            if let Some(form) = control_flow::recognize(send) {
                self.inlined(send, form);
                return;
            }
            self.expression(&send.receiver);
        }
        for argument in &send.message.arguments {
            self.expression(argument);
        }
    }

    fn inlined(&mut self, send: &ast::Send, form: InlineForm<'_>) {
        match form {
            InlineForm::IfTrueIfFalse(first, second) => {
                self.expression(&send.receiver);
                self.inlined_block(first, true, None);
                self.inlined_block(second, true, None);
            }
            InlineForm::IfTrue(block)
            | InlineForm::IfFalse(block)
            | InlineForm::And(block)
            | InlineForm::Or(block) => {
                self.expression(&send.receiver);
                self.inlined_block(block, true, None);
            }
            InlineForm::WhileTrue { condition, body }
            | InlineForm::WhileFalse { condition, body } => {
                let number = self.next_loop;
                self.next_loop += 1;
                self.inlined_block(condition, false, Some(number));
                self.inlined_block(body, false, Some(number));
            }
        }
    }

    fn inlined_block(
        &mut self,
        block: &ast::BlockExpression,
        conditional: bool,
        loop_number: Option<u32>,
    ) {
        self.scopes.push(Scope::Inlined {
            conditional,
            loop_number,
        });
        self.statements(&block.body.statements);
        self.scopes.pop();
    }

    fn real_block(&mut self, block: &ast::BlockExpression) {
        let key = BodyKey::Block(block.span);
        let (_, creating_frame) = self.current_frame();
        self.position += 1;
        let loops = self.open_loops(creating_frame);
        self.creation.insert(key, (self.position, loops));
        let argument_count = block.parameters.len() as u16;
        self.declare(key, argument_count, &block.body.temporaries);
        self.scopes.push(Scope::Frame { key });
        self.local_initializers(key, argument_count, &block.body.temporaries);
        self.statements(&block.body.statements);
        self.scopes.pop();
    }

    fn finish(self) -> Captures {
        let mut bodies: HashMap<BodyKey, BodyCaptures> = HashMap::new();
        for (key, mutable) in &self.locals {
            let mut places = vec![Place::Register; mutable.len()];
            let mut branch_assigned = vec![false; mutable.len()];
            let mut vector_size = 0;
            for index in 0..mutable.len() {
                let Some(facts) = self.facts.get(&(*key, index as u16)) else {
                    continue;
                };
                branch_assigned[index] = facts.writes.iter().any(|write| write.conditional);
                if mutable[index] && needs_vector(facts) {
                    places[index] = Place::Vector(vector_size);
                    vector_size += 1;
                }
            }
            bodies.insert(
                *key,
                BodyCaptures {
                    places,
                    vector_size,
                    branch_assigned,
                    copied: Vec::new(),
                },
            );
        }
        for (block, needs) in &self.needs {
            let mut copied: Vec<Captured> = Vec::new();
            for (body, index) in needs {
                let entry = match bodies[body].places[*index as usize] {
                    Place::Register => Captured::Variable {
                        body: *body,
                        index: *index,
                    },
                    Place::Vector(_) => Captured::Vector { body: *body },
                };
                if !copied.contains(&entry) {
                    copied.push(entry);
                }
            }
            bodies
                .get_mut(block)
                .expect("internal: undeclared block")
                .copied = copied;
        }
        Captures { bodies }
    }
}

/// The rule (`docs/design/execution.md`, "Where the analysis lives"): a
/// captured mutable local is copied unless something can write it once a
/// block holds it.
fn needs_vector(facts: &Facts) -> bool {
    if facts.written_by_block {
        return true;
    }
    let Some(first_capture) = facts.first_capture else {
        return false;
    };
    facts.writes.iter().any(|write| {
        write.position > first_capture
            || write.conditional
            || write
                .loops
                .iter()
                .any(|number| facts.capture_loops.contains(number))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::formatdoc;
    use std::path::PathBuf;

    /// Analyses the first instance-side method of a one-method module.
    fn analyze_method(body: &str) -> Captures {
        let source = formatdoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              foo = ( {body} )
            ) : ()
        "};
        let module = ns_syntax::parse(&source).expect("parse");
        let resolved = crate::resolve(
            module,
            std::rc::Rc::from(source.as_str()),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let method = resolved.module.root_class.instance_side.methods[0].clone();
        analyze(
            RootBody {
                argument_count: 0,
                temporaries: &method.body.temporaries,
                slot_initializers: Vec::new(),
                statements: &method.body.statements,
            },
            &resolved.bindings,
        )
    }

    /// The root body's place for the local at `index`.
    fn place(captures: &Captures, index: usize) -> Place {
        captures.body(BodyKey::Root).places[index]
    }

    /// The one block of a body whose root has exactly one block literal.
    fn only_block(captures: &Captures) -> &BodyCaptures {
        let keys: Vec<BodyKey> = captures
            .keys()
            .filter(|key| *key != BodyKey::Root)
            .collect();
        assert_eq!(keys.len(), 1, "expected exactly one block literal");
        captures.body(keys[0])
    }

    #[test]
    fn a_variable_written_only_before_capture_is_copied() {
        let captures = analyze_method("| x ::= 0. | x:: 7. ^[x]");
        assert_eq!(place(&captures, 0), Place::Register);
        assert_eq!(
            only_block(&captures).copied,
            vec![Captured::Variable {
                body: BodyKey::Root,
                index: 0
            }]
        );
    }

    #[test]
    fn a_variable_written_after_capture_goes_into_the_vector() {
        let captures = analyze_method("| x ::= 0. reader ::= nil. | reader:: [x]. x:: 2. ^reader");
        assert_eq!(place(&captures, 0), Place::Vector(0));
        assert_eq!(captures.body(BodyKey::Root).vector_size, 1);
        assert_eq!(
            only_block(&captures).copied,
            vec![Captured::Vector {
                body: BodyKey::Root
            }]
        );
    }

    /// A simultaneous temporary is written twice -- its future, then its
    /// value at the clause's end -- so a sibling's future block that reads it
    /// must share it through the vector, not copy the nil its register held.
    #[test]
    fn a_simultaneous_temporary_a_sibling_reads_goes_into_the_vector() {
        let captures = analyze_method("|| first = [second]. second = 2. || ^first");
        assert_eq!(
            place(&captures, 1),
            Place::Vector(0),
            "second is read by first's future block and written at the clause's end"
        );
    }

    #[test]
    fn a_variable_a_block_writes_goes_into_the_vector() {
        let captures = analyze_method("| count ::= 0. | ^[count:: count + 1]");
        assert_eq!(place(&captures, 0), Place::Vector(0));
    }

    /// A block that never reads the variable still has to be handed its
    /// scope's vector: the write goes through it.
    #[test]
    fn a_block_that_only_writes_is_handed_the_vector() {
        let captures = analyze_method("| count ::= 0. | ^[count:: 1]");
        assert_eq!(place(&captures, 0), Place::Vector(0));
        assert_eq!(
            only_block(&captures).copied,
            vec![Captured::Vector {
                body: BodyKey::Root
            }]
        );
    }

    /// And so does every block between it and the declaring frame.
    #[test]
    fn a_middle_block_carries_a_vector_an_inner_block_only_writes() {
        let captures = analyze_method("| count ::= 0. | ^[[count:: 1]]");
        let blocks: Vec<&BodyCaptures> = captures
            .keys()
            .filter(|key| *key != BodyKey::Root)
            .map(|key| captures.body(key))
            .collect();
        assert_eq!(blocks.len(), 2);
        for block in blocks {
            assert_eq!(
                block.copied,
                vec![Captured::Vector {
                    body: BodyKey::Root
                }]
            );
        }
    }

    #[test]
    fn a_parameter_and_an_immutable_local_are_never_in_the_vector() {
        let captures = analyze_method("| five = 5. | ^[five]");
        assert_eq!(place(&captures, 0), Place::Register);
    }

    #[test]
    fn a_write_in_an_inlined_loop_that_also_captures_goes_into_the_vector() {
        let captures = analyze_method(
            "| index ::= 0. x ::= 0. blocks ::= nil. | \
             [index < 3] whileTrue: [x:: index. blocks add: [x]. index:: index + 1]. ^blocks",
        );
        assert_eq!(place(&captures, 1), Place::Vector(0));
    }

    #[test]
    fn a_write_in_an_inlined_loop_condition_that_also_captures_goes_into_the_vector() {
        let captures = analyze_method(
            "| index ::= 0. x ::= 0. blocks ::= nil. | \
             [x:: index. index < 3] whileTrue: [blocks add: [x]. index:: index + 1]. ^blocks",
        );
        assert_eq!(place(&captures, 1), Place::Vector(0));
    }

    #[test]
    fn a_write_in_an_inlined_branch_goes_into_the_vector_only_when_a_block_captures() {
        let captured = analyze_method(
            "| x ::= 1. reader ::= nil. | true ifTrue: [x:: 2]. reader:: [x]. ^reader",
        );
        assert_eq!(place(&captured, 0), Place::Vector(0));
        assert!(captured.body(BodyKey::Root).branch_assigned[0]);

        let uncaptured = analyze_method("| x ::= 1. | true ifTrue: [x:: 2]. ^x");
        assert_eq!(place(&uncaptured, 0), Place::Register);
        assert!(uncaptured.body(BodyKey::Root).branch_assigned[0]);
    }

    /// A mirror send's failure block is inlined but builds no fallback
    /// closure, so a local it assigns is not `branch_assigned`: no later
    /// send can write it, and it stays in its register.
    #[test]
    fn a_write_in_a_mirror_sends_failure_block_is_not_branch_assigned() {
        let source = indoc::indoc! {"
            Newspeak3
            'T'
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class C = Object new () (
                foo = ( | x ::= 1. | vmMirror stringSize: self ifFail: [x:: 2]. ^x )
              )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let Ok(resolved) = crate::resolver::resolve_kernel(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        ) else {
            panic!("the kernel-shaped module resolves");
        };
        let method = &resolved.module.root_class.instance_side.nested_classes[0]
            .instance_side
            .methods[0];
        let captures = analyze(
            RootBody {
                argument_count: 0,
                temporaries: &method.body.temporaries,
                slot_initializers: Vec::new(),
                statements: &method.body.statements,
            },
            &resolved.bindings,
        );
        assert_eq!(place(&captures, 0), Place::Register);
        assert!(!captures.body(BodyKey::Root).branch_assigned[0]);
        // The failure block is no closure: the root is the only body.
        assert_eq!(captures.bodies.len(), 1);
    }

    #[test]
    fn a_variable_read_only_in_an_inlined_block_is_not_captured() {
        let captures = analyze_method("| x ::= 4. | ^true ifTrue: [x + 1] ifFalse: [0]");
        assert_eq!(place(&captures, 0), Place::Register);
        assert_eq!(captures.keys().count(), 1);
    }

    #[test]
    fn a_middle_block_carries_what_an_inner_block_needs() {
        let captures = analyze_method("| x ::= 1. | ^[[x]]");
        let mut blocks: Vec<&BodyCaptures> = captures
            .keys()
            .filter(|key| *key != BodyKey::Root)
            .map(|key| captures.body(key))
            .collect();
        assert_eq!(blocks.len(), 2);
        blocks.sort_by_key(|block| block.copied.len());
        for block in blocks {
            assert_eq!(
                block.copied,
                vec![Captured::Variable {
                    body: BodyKey::Root,
                    index: 0
                }]
            );
        }
    }

    #[test]
    fn an_initializer_of_a_local_counts_as_a_write() {
        // The reader is created first, so the initializer of `x` -- which the
        // lowering runs as the body's first statements, in declaration order
        // -- writes after the capture.
        let captures = analyze_method("| reader ::= [x]. x ::= 1. | ^reader");
        assert_eq!(place(&captures, 1), Place::Vector(0));
    }
}
