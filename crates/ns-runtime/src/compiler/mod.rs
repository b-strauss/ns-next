//! Bytecode compiler entry points, and the choice between the optimized
//! lowering (`lower.rs`) and the reference lowering it is checked against
//! (`reference.rs`).
//! Compiling an already-resolved module is a total, deterministic lowering
//! that can never raise a `NewspeakError`; a failure here is an `ns-runtime`
//! bug and panics.

pub(crate) mod capture;
pub(crate) mod control_flow;
pub(crate) mod lower;
pub(crate) mod primitive_method;
pub(crate) mod reference;
pub(crate) mod simultaneous;
pub(crate) mod unwind_marker;

/// Which lowering compiles a module: the optimized one every program runs
/// under, or the deliberately naive reference lowering it is checked against
/// (`docs/design/compiler.md`, "The reference lowering"). Chosen once per
/// `Interpreter`, so one interpreter never mixes the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lowering {
    Optimized,
    Reference,
}

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::Span;

use crate::bytecode::{CodeKind, CompiledCode, Instruction, Literal};
use crate::mixin::Mixin;
use crate::types::Binding;
use crate::Symbol;

use capture::{BodyKey, RootBody};
use lower::{Enclosing, Lowerer};

/// A method's parameter count, `instantiate::pattern_parameter_count`'s one
/// definition, shared with the interpreter's factory-arity guard. Left
/// unnarrowed: a pattern can declare more parameters than a `u16` counts,
/// and each lowering judges that against the register ceiling itself.
fn argument_count_of(pattern: &ast::MessagePattern) -> usize {
    crate::interpreter::instantiate::pattern_parameter_count(pattern)
}

/// A mixin's primary factory parameter count; `0` for a mixin with no
/// factory of its own -- a builtin's, a class side's. Unnarrowed, as
/// `argument_count_of` is.
fn factory_argument_count(mixin: &Mixin) -> usize {
    mixin.factory.map_or(0, |factory| factory.argument_count)
}

/// `count` as the `u16` a `CompiledCode` stores an arity in, or `u16::MAX`
/// when it does not fit -- which only a body past the register ceiling
/// meets, and that body is `body_too_large`'s, whose arity is never read
/// for a register.
fn saturated_count(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

/// Whether a body's parameters and named temporaries fit the registers a
/// `u16` numbers, so that each has a resolver index and a register of its
/// own. The capture analysis and both lowerings ask it before they lay a
/// body out; one that fails it is never laid out, and compiles to
/// `body_too_large`.
fn named_locals_fit(argument_count: usize, temporary_count: usize) -> bool {
    argument_count
        .checked_add(temporary_count)
        .is_some_and(|count| count <= usize::from(u16::MAX))
}

/// A local's resolver index -- or the register it sits in -- in a body
/// that `named_locals_fit` admitted, where every index fits a `u16`; one
/// past `u16::MAX` is a compiler defect, never a program's.
fn local_index(index: usize) -> u16 {
    u16::try_from(index)
        .expect("internal: a local's index past u16::MAX in a body whose locals were checked")
}

/// The resolver index of a body's `offset`-th named temporary, which
/// follows its parameters; `local_index`'s precondition holds.
fn temporary_index(argument_count: u16, offset: usize) -> u16 {
    local_index(usize::from(argument_count) + offset)
}

/// What a body past the register ceiling compiles to: the smallest
/// well-formed body of its kind and arity, which raises the too-large error
/// before it does anything else -- the deferral `Literal::UnresolvedIdentifier`
/// uses, so that compiling never fails and a body never run never fails
/// (`docs/design/compiler.md`, "Compilation cannot fail"). It takes no
/// copied values: a too-large block's creator hands it none. Its one
/// register follows the arguments; when every register is an argument the
/// last argument's serves, since the literal raises before anything is
/// written to it. Both lowerings build it here, so the two cannot disagree
/// on its shape.
fn body_too_large(kind: CodeKind, argument_count: u16, source: Span) -> CompiledCode {
    let destination = argument_count.min(u16::MAX - 1);
    let register_count = argument_count.max(destination + 1);
    CompiledCode::new(
        kind,
        vec![
            Instruction::LoadLiteral {
                destination,
                literal: Literal::BodyTooLarge,
            },
            Instruction::ImplicitReturn {
                register: destination,
            },
        ],
        argument_count,
        register_count,
        0,
        vec![(0, 0..2)],
        source,
    )
}

/// A block's parameter names as its `MakeClosure` carries them: all of
/// them, except for a block with more parameters than a `u16` counts, whose
/// `body_too_large` body takes `u16::MAX`. Its `MakeClosure` names as many,
/// so that the two agree, as the image decoder checks; a call with more
/// arguments meets the block's arity check, which raises.
fn parameters_of(mut parameters: Vec<Symbol>, code: &CompiledCode) -> Vec<Symbol> {
    parameters.truncate(usize::from(code.argument_count));
    parameters
}

pub fn compile_method(
    method: &ast::MethodDeclaration,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
) -> CompiledCode {
    if lowering == Lowering::Reference {
        return reference::compile_method(method, bindings);
    }
    let argument_count = argument_count_of(&method.pattern);
    let captures = Rc::new(capture::analyze(
        RootBody {
            argument_count,
            temporaries: &method.body.temporaries,
            slot_initializers: Vec::new(),
            statements: &method.body.statements,
        },
        bindings,
    ));
    let mut lowerer = Lowerer::new(
        CodeKind::Method,
        BodyKey::Root,
        argument_count,
        &method.body.temporaries,
        bindings,
        captures,
        false,
        Enclosing::default(),
        0,
    );
    lowerer.lower_local_initializers(&method.body.temporaries);
    lowerer.lower_body(&method.body.statements);
    lowerer.finish(method.span)
}

/// Compiles `mixin`'s per-mixin bodies onto it: the instance initializer
/// always, the superclass clause and its message's arguments when the
/// declaration has them, and each `<:` clause's message arguments. Called
/// while the mixin is still a value, before it is wrapped in the `Rc` every
/// application shares -- the one moment it can be written.
pub(crate) fn compile_mixin_bodies(
    mixin: &mut Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
) {
    if lowering == Lowering::Reference {
        reference::compile_mixin_bodies(mixin, bindings);
        return;
    }
    mixin.initializer_code = Some(Rc::new(compile_mixin_initializer(mixin, bindings)));
    mixin.superclass_clause_code = compile_superclass_clause(mixin, bindings).map(Rc::new);
    mixin.superclass_message_code = compile_superclass_message(mixin, bindings).map(Rc::new);
    fill_mixin_application_messages(mixin, |mixin, clause| {
        compile_clause_message(mixin, clause, bindings)
    });
}

/// Fills each `<:` clause's `message_code` from the declaration it was read
/// off, with `compile` -- one lowering's clause-message compiler. Shared by
/// both lowerings because it only pairs a clause with its body; the bodies
/// themselves are each lowering's own.
pub(super) fn fill_mixin_application_messages(
    mixin: &mut Mixin,
    compile: impl Fn(&Mixin, &ast::SuperclassClause) -> Option<CompiledCode>,
) {
    let Some(declaration) = mixin.declaration.clone() else {
        return;
    };
    debug_assert_eq!(
        declaration.mixin_applications.len(),
        mixin.mixin_applications.len(),
        "internal: build_mixin reads one clause per `<:`"
    );
    for (index, clause) in declaration.mixin_applications.iter().enumerate() {
        let code = compile(mixin, clause).map(Rc::new);
        mixin.mixin_applications[index].message_code = code;
    }
}

/// A mixin's instance initializer: each of `mixin`'s own slots with an
/// initializer (declaration order) as `<expression>; InitializeSlot`, then each
/// `initExpr` (a top-level `^` is evaluated and discarded), then
/// `LoadSelf; ImplicitReturn` -- `docs/design/object-model.md`, "Instantiation".
/// `argument_count` is `mixin`'s own factory parameter count -- the resolver
/// scoped this code with exactly those parameters as `Binding::Local`.
///
/// Needs no absolute field index: each `InitializeSlot` carries `own_index`, the slot's
/// position among `mixin`'s *own* slots only (plain declaration-order
/// `enumerate()`, independent of any superclass) -- never an absolute field
/// index, because this one compiled body runs for every application of
/// `mixin` (it lives on the mixin, not on a class object, per §6.2), and
/// different applications can legitimately disagree about where the mixin's
/// own slots start. The VM adds the executing activation's slot base --
/// its `current_class`'s, the level it initializes, so that level's own
/// real `MixinApplication.own_slot_base` (`Heap::activation_slot_base`) --
/// to turn `own_index` into the absolute index the instance array actually
/// needs.
fn compile_mixin_initializer(mixin: &Mixin, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledCode {
    let argument_count = factory_argument_count(mixin);
    // Each own slot's future block (§6.3.2), by own index, for a simultaneous
    // slot with an initializer: built once, so the capture analysis and the
    // lowering below walk the very same block literal.
    let future_blocks: Vec<Option<Expression>> = mixin
        .slots
        .iter()
        .map(|slot| simultaneous::future_block(slot.declaration.as_ref()?))
        .collect();
    // The same expressions, in the same order, this function lowers below:
    // a lazy slot's initializer is its getter's (`compile_lazy_slot_getter`),
    // never this body's, and a simultaneous slot's is its future's block.
    let slot_initializers: Vec<&Expression> = mixin
        .slots
        .iter()
        .zip(&future_blocks)
        .filter(|(slot, _)| !slot.lazy)
        .filter_map(|(slot, future_block)| {
            future_block
                .as_ref()
                .or_else(|| slot.declaration.as_ref()?.initializer.as_ref())
        })
        .collect();
    let captures = Rc::new(capture::analyze(
        RootBody {
            argument_count,
            temporaries: &[],
            slot_initializers,
            statements: &mixin.initializer_expressions,
        },
        bindings,
    ));
    let mut lowerer = Lowerer::new(
        CodeKind::Initializer,
        BodyKey::Root,
        argument_count,
        &[],
        bindings,
        captures,
        false,
        Enclosing::default(),
        0,
    );
    let mut source: Option<Span> = None;
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        if slot.lazy {
            continue;
        }
        let Some(declaration) = &slot.declaration else {
            continue;
        };
        if let Some(block) = &future_blocks[own_index] {
            lowerer.lower_simultaneous_slot_initializer(block, own_index as u32);
            source = Some(merge_span(source, declaration.span));
        } else if let Some(initializer) = &declaration.initializer {
            lowerer.lower_slot_initializer(initializer, own_index as u32);
            source = Some(merge_span(source, declaration.span));
        }
    }
    // §6.3.2: "All these futures are resolved once the last slot declaration
    // in the simultaneous slot clause has been executed" -- each simultaneous
    // slot read back and stored resolved, in declaration order.
    for (own_index, future_block) in future_blocks.iter().enumerate() {
        if future_block.is_some() && !mixin.slots[own_index].lazy {
            lowerer.lower_simultaneous_slot_end(own_index as u32);
        }
    }
    for statement in &mixin.initializer_expressions {
        source = Some(merge_span(source, statement_span(statement)));
    }
    lowerer.lower_body(&mixin.initializer_expressions);
    lowerer.finish(source.unwrap_or(Span::new(0, 0)))
}

/// A lazy slot's getter (§6.3.3), a zero-argument method that answers the
/// slot at `own_index` once it is filled (`ReturnSlotIfFilled`) and
/// otherwise evaluates the slot's initializer, stores it with
/// `InitializeSlot` at `own_index` and answers it. The resolver scoped `initializer` as a unary method's body --
/// no factory parameter is in scope (§6: they are "in scope in the instance
/// initializer but not in the rest of the class body") -- so it lowers as
/// one. `span` is the slot declaration's.
pub(crate) fn compile_lazy_slot_getter(
    initializer: &ast::Expression,
    own_index: u32,
    span: Span,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
) -> CompiledCode {
    if lowering == Lowering::Reference {
        return reference::compile_lazy_slot_getter(initializer, own_index, span, bindings);
    }
    let captures = Rc::new(capture::analyze(
        RootBody {
            argument_count: 0,
            temporaries: &[],
            slot_initializers: vec![initializer],
            statements: &[],
        },
        bindings,
    ));
    let mut lowerer = Lowerer::new(
        CodeKind::Method,
        BodyKey::Root,
        0,
        &[],
        bindings,
        captures,
        false,
        Enclosing::default(),
        0,
    );
    lowerer.lower_lazy_slot_getter(initializer, own_index);
    lowerer.finish(span)
}

/// A mixin's superclass clause as executable code: the expression
/// `newspeak-spec.pdf` §6.1 calls `ec`, lowered as a single synthetic `^ec`
/// so the body ends in the `Return` that hands the class object back.
///
/// The expression itself comes from `interpreter::nested`'s
/// `superclass_clause_expression`, the *one* definition of what a superclass
/// clause evaluates.
///
/// Returns `None` for a mixin with no clause (a class side, a builtin, or
/// `class C = (...)`), which the accessor answers with `Object` without
/// running any code at all.
fn compile_superclass_clause(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let declaration = mixin.declaration.as_ref()?;
    let clause = declaration.superclass.as_ref()?;
    let statements = vec![Statement::Return(
        crate::interpreter::nested::superclass_clause_expression(
            clause,
            &declaration.mixin_applications,
        ),
    )];
    let captures = Rc::new(capture::analyze(
        RootBody {
            argument_count: 0,
            temporaries: &[],
            slot_initializers: Vec::new(),
            statements: &statements,
        },
        bindings,
    ));
    let mut lowerer = Lowerer::new(
        CodeKind::SuperclassClause,
        BodyKey::Root,
        0,
        &[],
        bindings,
        captures,
        false,
        Enclosing::default(),
        0,
    );
    lowerer.lower_body(&statements);
    Some(lowerer.finish(clause.span))
}

/// A mixin's superclass-clause *message arguments* as executable code: the
/// arguments of `newspeak-spec.pdf` §6.1's deferred `message`, lowered as a
/// single synthetic `^{argument. argument}` so the body ends in the `Return`
/// that hands the whole argument list back at once.
///
/// `argument_count` is this mixin's own factory parameter count, not zero as in
/// `compile_superclass_clause`: §6.2 evaluates the message in the
/// instantiation's own initializer context, where the resolver scoped these
/// expressions with exactly those parameters as `Binding::Local` (the
/// `class M usingPlatform: p = Super usingPlatform: p (...)` idiom).
///
/// The expression comes from `interpreter::nested`'s
/// `superclass_clause_message_arguments_expression`, the one definition of what
/// the clause's arguments are -- `SuperclassClause::has_arguments` is read off
/// it too. `None` when the clause has no message, or a message with no
/// arguments: then there is nothing to evaluate.
fn compile_superclass_message(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let clause = mixin.declaration.as_ref()?.superclass.as_ref()?;
    compile_clause_message(mixin, clause, bindings)
}

/// One inheritance clause's message arguments, compiled for `mixin`'s
/// declaration: its superclass clause's, or one of its `<:` clauses', which
/// §6.2 evaluates in the same instance initializer and the resolver
/// therefore scopes the same way.
fn compile_clause_message(
    mixin: &Mixin,
    clause: &ast::SuperclassClause,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let arguments =
        crate::interpreter::nested::superclass_clause_message_arguments_expression(clause)?;
    let argument_count = factory_argument_count(mixin);
    let statements = vec![Statement::Return(arguments)];
    let captures = Rc::new(capture::analyze(
        RootBody {
            argument_count,
            temporaries: &[],
            slot_initializers: Vec::new(),
            statements: &statements,
        },
        bindings,
    ));
    let mut lowerer = Lowerer::new(
        CodeKind::SuperclassMessage,
        BodyKey::Root,
        argument_count,
        &[],
        bindings,
        captures,
        false,
        Enclosing::default(),
        0,
    );
    lowerer.lower_body(&statements);
    Some(lowerer.finish(clause.span))
}

fn merge_span(accumulator: Option<Span>, next: Span) -> Span {
    match accumulator {
        Some(span) => span.merge(next),
        None => next,
    }
}

fn statement_span(statement: &Statement) -> Span {
    match statement {
        Statement::Expression(expression) | Statement::Return(expression) => {
            expression_span(expression)
        }
    }
}

fn expression_span(expression: &Expression) -> Span {
    match expression {
        Expression::Identifier(id) => id.span,
        Expression::Literal(literal) => literal_span(literal),
        Expression::Send(send) => send.span,
        Expression::Cascade(cascade) => cascade.span,
        Expression::Assign { span, .. } => *span,
        Expression::Block(block) => block.span,
        Expression::DynamicArray { span, .. } => *span,
    }
}

fn literal_span(literal: &ast::Literal) -> Span {
    match literal {
        ast::Literal::Integer { span, .. }
        | ast::Literal::Float { span, .. }
        | ast::Literal::String { span, .. }
        | ast::Literal::Symbol { span, .. }
        | ast::Literal::Boolean { span, .. }
        | ast::Literal::Nil { span }
        | ast::Literal::Pattern { span, .. } => *span,
    }
}
