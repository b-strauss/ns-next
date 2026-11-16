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

use ns_syntax::ast::{self, Expression, Statement, StatementKind};
use ns_syntax::Span;

use crate::bytecode::{BodyLimit, CodeKind, CompiledCode, Instruction, Literal};
use crate::debugger::metadata::MetadataCollector;
use crate::mixin::Mixin;
use crate::types::Binding;
use crate::Symbol;

use capture::{BodyKey, RootBody};
use lower::{Collecting, Enclosing, Lowerer};

/// A method's parameter count, `instantiate::pattern_parameter_count`'s one
/// definition, shared with the interpreter's factory-arity guard. Left
/// unnarrowed: a pattern can declare more parameters than a `u16` counts,
/// and each lowering judges that against the register ceiling and the
/// argument limit itself.
fn argument_count_of(pattern: &ast::MessagePattern) -> usize {
    crate::interpreter::instantiate::pattern_parameter_count(pattern)
}

/// A mixin's primary factory parameter count; `0` for a mixin with no
/// factory of its own -- a builtin's, a class side's. Unnarrowed, as
/// `argument_count_of` is.
fn factory_argument_count(mixin: &Mixin) -> usize {
    mixin.factory.map_or(0, |factory| factory.argument_count)
}

/// What a root body's lowerer collects into, when `collector` is given:
/// the body's parameters named `parameters()`.
fn collecting(
    collector: Option<&MetadataCollector>,
    parameters: impl FnOnce() -> Vec<Symbol>,
) -> Option<Collecting<'_>> {
    collector.map(|collector| Collecting {
        collector,
        parameters: parameters(),
    })
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

/// The mixin-relative index of the slot at `index` among a mixin's slots,
/// as `InitializeSlot` and its kin store it. Each slot is declared in the
/// mixin's source by at least one byte, and the frontend refuses a source
/// past `u32::MAX` bytes (`ns_syntax::source_too_long`).
fn own_slot_index(index: usize) -> u32 {
    u32::try_from(index)
        .expect("internal: a mixin declares fewer slots than its source's bytes, which fit a u32")
}

/// The resolver index of a body's `offset`-th named temporary, which
/// follows its parameters; `local_index`'s precondition holds.
fn temporary_index(argument_count: u16, offset: usize) -> u16 {
    local_index(usize::from(argument_count) + offset)
}

/// The limit a body taking `argument_count` parameters passes by them
/// alone: `ARGUMENT_LIMIT` (`docs/design/compiler.md`, "Limits"). Both
/// lowerings ask it once the body's named locals are known to fit the
/// registers, which a body past the register ceiling fails first.
fn parameter_limit_passed(argument_count: usize) -> Option<BodyLimit> {
    (argument_count > usize::from(crate::bytecode::ARGUMENT_LIMIT)).then_some(BodyLimit::Arguments)
}

/// The limit a body passes by emitting `instruction` after `emitted`
/// others: `INSTRUCTION_LIMIT`, or `ARGUMENT_LIMIT` for a send's or a
/// clause message's arguments (`Instruction::argument_slots`). Both
/// lowerings ask it of every instruction before they emit it, and emit
/// nothing once a limit is passed, so that both judge the same body alike
/// and no body grows past the instruction limit.
fn emitting_passes_a_limit(emitted: usize, instruction: &Instruction) -> Option<BodyLimit> {
    if emitted >= crate::bytecode::INSTRUCTION_LIMIT as usize {
        return Some(BodyLimit::Instructions);
    }
    instruction
        .argument_slots()
        .filter(|(_, sources)| sources.len() > usize::from(crate::bytecode::ARGUMENT_LIMIT))
        .map(|_| BodyLimit::Arguments)
}

/// How many steps up an initializer's superclass clause names the class it
/// initializes: one per `<:` clause, `clause_count`, and one for the
/// declaration's own level when it has a body, `own_level`. It fits a `u16`
/// for every resolved declaration: the resolver refuses more than
/// `CLAUSE_INDEX_LIMIT` clauses, and the image decoder a row of more, and
/// that limit plus one is far below `u16::MAX`.
fn superclass_steps(clause_count: usize, own_level: usize) -> u16 {
    const _: () = assert!(crate::mixin::CLAUSE_INDEX_LIMIT < u16::MAX as usize);
    debug_assert!(clause_count <= crate::mixin::CLAUSE_INDEX_LIMIT && own_level <= 1);
    u16::try_from(clause_count + own_level)
        .expect("internal: more `<:` clauses than CLAUSE_INDEX_LIMIT reached the compiler")
}

/// What a body past one of the compiler's limits compiles to, `limit`
/// naming the one it passed: the smallest
/// well-formed body of its kind and arity, which raises the too-large error
/// before it does anything else -- the deferral `Literal::UnresolvedIdentifier`
/// uses, so that compiling never fails and a body never run never fails
/// (`docs/design/compiler.md`, "Compilation cannot fail"). It takes no
/// copied values: a too-large block's creator hands it none. Its one
/// register is the first above the header's numbers
/// (`bytecode::HEADER_REGISTERS`); when the arguments leave no number for
/// one there, the last argument's serves, since the literal raises before
/// anything is written to it, and the body's registers are its arguments
/// alone. Both lowerings build it here, so the two cannot disagree on its
/// shape.
fn body_too_large(
    kind: CodeKind,
    argument_count: u16,
    source: Span,
    limit: BodyLimit,
) -> CompiledCode {
    let first_above_header =
        u32::from(argument_count) + u32::from(crate::bytecode::HEADER_REGISTERS);
    let (destination, register_count) = match u16::try_from(first_above_header + 1) {
        Ok(register_count) => (register_count - 1, register_count),
        // Past the ceiling the arguments reach above `u16::MAX -
        // HEADER_REGISTERS`, so there is a last one.
        Err(_) => (argument_count - 1, argument_count),
    };
    CompiledCode::new(
        kind,
        vec![
            Instruction::LoadLiteral {
                destination,
                literal: Literal::BodyTooLarge(limit),
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

/// A factory's body (`newspeak-spec.pdf` §6.3: a factory message sent to a
/// class object makes an instance and invokes the instance initializer on
/// it): `AllocateInstance` of the class object the factory was sent to,
/// `InitializeInstance` of it with the factory's arguments, which are this
/// body's own, and `Return` of the instance -- never of what the initializer
/// answers. The first two registers above the header's numbers
/// (`bytecode::HEADER_REGISTERS`) hold the instance and the initializer's
/// answer. Three instructions whatever the arity:
/// `InitializeInstance` names no argument list, so an arity read off an
/// image's two bytes cannot make the decoder build a body that grows with
/// it.
///
/// Synthesized rather than lowered: it has no source and no resolver
/// bindings, and depends on nothing but the arity, so every factory of one
/// arity runs the same body. Both lowerings, the embedded shipped code and
/// the image decoder reach it through `mixin::factory_entry`, and an image
/// never carries one, so none can be forged. A factory whose two registers
/// do not fit after its arguments, or whose arguments are past
/// `ARGUMENT_LIMIT`, gets `body_too_large`'s body, which raises the
/// too-large error when the factory is sent.
pub fn factory_body(argument_count: u16) -> CompiledCode {
    let source = Span::new(0, 0);
    let Some(register_count) = argument_count.checked_add(crate::bytecode::HEADER_REGISTERS + 2)
    else {
        return body_too_large(
            CodeKind::Method,
            argument_count,
            source,
            BodyLimit::Registers,
        );
    };
    if let Some(limit) = parameter_limit_passed(usize::from(argument_count)) {
        return body_too_large(CodeKind::Method, argument_count, source, limit);
    }
    let instance = argument_count + crate::bytecode::HEADER_REGISTERS;
    let answer = instance + 1;
    CompiledCode::new(
        CodeKind::Method,
        vec![
            Instruction::AllocateInstance {
                destination: instance,
            },
            Instruction::InitializeInstance {
                instance,
                destination: answer,
            },
            Instruction::Return { register: instance },
        ],
        argument_count,
        register_count,
        0,
        vec![(0, 0..3)],
        source,
    )
}

/// The clauses an initializer body evaluates before its own part: the
/// superclass clause, which stands for `Object new` when a declaration
/// writes none (§6.1), and the `<:` clauses whose levels the body
/// initializes -- all of them for the full body, none for the standalone
/// one.
#[derive(Clone, Copy)]
pub(super) struct Prologue<'a> {
    pub(super) superclass: Option<&'a ast::SuperclassClause>,
    pub(super) applied: &'a [ast::SuperclassClause],
    /// Whether the declaration adds a level of its own, which the body runs
    /// at: then its clauses' levels start one above it. A body-less
    /// declaration's body runs at its class object, the level its last
    /// clause adds, so they are counted from 0 (`Mixin::has_body`).
    pub(super) has_body: bool,
    /// The declaration's name, the span the debugger's metadata gives the
    /// superclass's initialization where no superclass clause is written.
    pub(super) declaration_name: Span,
}

impl<'a> Prologue<'a> {
    /// The prologue of `mixin`'s full body, or of its standalone body when
    /// `with_applied_clauses` is false; `None` for a mixin no declaration
    /// was read into -- a class side -- whose full body is its own part.
    pub(super) fn of_mixin(mixin: &'a Mixin, with_applied_clauses: bool) -> Option<Prologue<'a>> {
        let declaration = mixin.declaration.as_deref()?;
        Some(Prologue {
            superclass: declaration.superclass.as_ref(),
            applied: if with_applied_clauses {
                &declaration.mixin_applications
            } else {
                &[]
            },
            has_body: mixin.has_body,
            declaration_name: declaration.name.span,
        })
    }

    /// Every clause argument, in the order the prologue evaluates them:
    /// the superclass clause's, then each `<:` clause's from the lowest
    /// level up.
    pub(super) fn arguments(self) -> impl Iterator<Item = &'a Expression> {
        clause_arguments(self.superclass).iter().chain(
            self.applied
                .iter()
                .flat_map(|clause| clause_arguments(Some(clause))),
        )
    }

    /// `source` merged with the span of every clause.
    pub(super) fn merge_spans(self, source: Option<Span>) -> Option<Span> {
        self.superclass
            .into_iter()
            .chain(self.applied)
            .fold(source, |source, clause| {
                Some(merge_span(source, clause.span))
            })
    }
}

/// A clause's message's arguments, which an initializer evaluates in its
/// factory's scope (§6.3); none for a clause without a message.
pub(super) fn clause_arguments(clause: Option<&ast::SuperclassClause>) -> &[Expression] {
    clause
        .and_then(crate::interpreter::nested::superclass_clause_message)
        .map_or(&[], |message| &message.arguments[..])
}

/// A clause's message's selector; `new` for a clause without a message, and
/// for no clause at all (§6.1: "if the message is omitted, it defaults to
/// `new`").
pub(super) fn clause_selector(clause: Option<&ast::SuperclassClause>) -> Symbol {
    clause
        .and_then(crate::interpreter::nested::superclass_clause_message)
        .map_or(Symbol::NEW, lower::selector_of)
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

/// `collector`, when given, collects the debugger's metadata of the method
/// and every block in it (`docs/design/debugger.md`, "Compiling the
/// metadata again"); the code is the same either way.
pub(crate) fn compile_method(
    method: &ast::MethodDeclaration,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
    collector: Option<&MetadataCollector>,
) -> CompiledCode {
    if lowering == Lowering::Reference {
        return reference::compile_method(method, bindings, collector);
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
        collecting(collector, || {
            crate::resolver::pattern_parameter_names(&method.pattern)
        }),
    );
    lowerer.lower_local_initializers(&method.body.temporaries);
    lowerer.lower_body(&method.body.statements);
    lowerer.finish(method.span)
}

/// Compiles `mixin`'s per-mixin bodies onto it: the full instance
/// initializer and its own part always, the standalone initializer when the
/// declaration writes `<:` clauses and a body, the superclass clause and its message's
/// arguments when the declaration has them, and each `<:` clause's message
/// arguments. Called while the mixin is still a value, before it is wrapped
/// in the `Rc` every application shares -- the one moment it can be
/// written. `collector`, when given, collects every body's debugger
/// metadata, as `compile_method`'s does.
pub(crate) fn compile_mixin_bodies(
    mixin: &mut Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
    collector: Option<&MetadataCollector>,
) {
    if lowering == Lowering::Reference {
        reference::compile_mixin_bodies(mixin, bindings, collector);
    } else {
        compile_mixin_bodies_optimized(mixin, bindings, collector);
    }
    mixin.mark_declaration_code();
}

/// `compile_mixin_bodies` under the optimized lowering.
fn compile_mixin_bodies_optimized(
    mixin: &mut Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
    collector: Option<&MetadataCollector>,
) {
    let own = Rc::new(compile_mixin_initializer(mixin, bindings, collector));
    mixin.initializer_code = Some(
        compile_mixin_full_initializer(mixin, bindings, true, collector)
            .map_or_else(|| Rc::clone(&own), Rc::new),
    );
    // A body-less declaration's class object is never reached outside it,
    // so it has no standalone body (`Mixin::standalone_initializer_code`).
    mixin.standalone_initializer_code = if mixin.mixin_applications.is_empty() || !mixin.has_body {
        None
    } else {
        compile_mixin_full_initializer(mixin, bindings, false, collector).map(Rc::new)
    };
    mixin.own_initializer_code = Some(own);
    mixin.superclass_clause_code =
        compile_superclass_clause(mixin, bindings, collector).map(Rc::new);
}

/// A mixin's full instance initializer, or with `with_applied_clauses`
/// false its standalone one: the prologue
/// (`Lowerer::lower_instance_initialization_prologue`), then the own part
/// `compile_mixin_initializer` compiles alone. `None` for a mixin no
/// declaration was read into, whose full body is its own part.
fn compile_mixin_full_initializer(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
    with_applied_clauses: bool,
    collector: Option<&MetadataCollector>,
) -> Option<CompiledCode> {
    let prologue = Prologue::of_mixin(mixin, with_applied_clauses)?;
    Some(compile_initializer_body(
        mixin,
        bindings,
        Some(prologue),
        collector,
    ))
}

/// A mixin's own part of the instance initializer: each of `mixin`'s own
/// slots with an initializer (declaration order) as
/// `<expression>; InitializeSlot`, then each `initExpr` (a top-level `^` is
/// evaluated and discarded), then `LoadSelf; ImplicitReturn` --
/// `docs/design/object-model.md`, "Instantiation".
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
fn compile_mixin_initializer(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
    collector: Option<&MetadataCollector>,
) -> CompiledCode {
    compile_initializer_body(mixin, bindings, None, collector)
}

/// An initializer body: `prologue`'s clauses, when there is one, then the
/// own part. Its variables are the factory's parameters.
fn compile_initializer_body(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
    prologue: Option<Prologue<'_>>,
    collector: Option<&MetadataCollector>,
) -> CompiledCode {
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
    // the prologue's clause arguments, then the slots' -- a lazy slot's
    // initializer is its getter's (`compile_lazy_slot_getter`), never this
    // body's, and a simultaneous slot's is its future's block.
    let slot_initializers: Vec<&Expression> = prologue
        .into_iter()
        .flat_map(Prologue::arguments)
        .chain(
            mixin
                .slots
                .iter()
                .zip(&future_blocks)
                .filter(|(slot, _)| !slot.lazy)
                .filter_map(|(slot, future_block)| {
                    future_block
                        .as_ref()
                        .or_else(|| slot.declaration.as_ref()?.initializer.as_ref())
                }),
        )
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
        collecting(collector, || {
            mixin
                .declaration
                .as_ref()
                .and_then(|declaration| declaration.factory.as_ref())
                .map(crate::resolver::pattern_parameter_names)
                .unwrap_or_default()
        }),
    );
    let mut source: Option<Span> = None;
    if let Some(prologue) = prologue {
        lowerer.lower_instance_initialization_prologue(prologue);
        source = prologue.merge_spans(source);
    }
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        if slot.lazy {
            continue;
        }
        let Some(declaration) = &slot.declaration else {
            continue;
        };
        if let Some(block) = &future_blocks[own_index] {
            lowerer.lower_simultaneous_slot_initializer(
                block,
                own_slot_index(own_index),
                declaration.span,
            );
            source = Some(merge_span(source, declaration.span));
        } else if let Some(initializer) = &declaration.initializer {
            lowerer.lower_slot_initializer(
                initializer,
                own_slot_index(own_index),
                declaration.span,
            );
            source = Some(merge_span(source, declaration.span));
        }
    }
    // §6.3.2: "All these futures are resolved once the last slot declaration
    // in the simultaneous slot clause has been executed" -- each simultaneous
    // slot read back and stored resolved, in declaration order.
    for (own_index, future_block) in future_blocks.iter().enumerate() {
        let slot = &mixin.slots[own_index];
        if let (Some(_), false, Some(declaration)) = (future_block, slot.lazy, &slot.declaration) {
            lowerer.lower_simultaneous_slot_end(own_slot_index(own_index), declaration.span);
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
/// one. `span` is the slot declaration's. `collector`, when given, collects
/// the getter's debugger metadata, as `compile_method`'s does.
pub(crate) fn compile_lazy_slot_getter(
    initializer: &ast::Expression,
    own_index: u32,
    span: Span,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
    collector: Option<&MetadataCollector>,
) -> CompiledCode {
    if lowering == Lowering::Reference {
        return reference::compile_lazy_slot_getter(
            initializer,
            own_index,
            span,
            bindings,
            collector,
        );
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
        collecting(collector, Vec::new),
    );
    lowerer.lower_lazy_slot_getter(initializer, own_index, span);
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
    collector: Option<&MetadataCollector>,
) -> Option<CompiledCode> {
    let declaration = mixin.declaration.as_ref()?;
    let clause = declaration.superclass.as_ref()?;
    let statements = vec![Statement::of_return(
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
        collecting(collector, Vec::new),
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
    match &statement.kind {
        StatementKind::Expression(expression) | StatementKind::Return(expression) => {
            expression.kind.span()
        }
    }
}
