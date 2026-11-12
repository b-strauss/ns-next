//! The oracle over every body (`docs/design/debugger.md`, decision 14):
//! every body of every loaded declaration compiled again, held to its
//! running counterpart, and its tables held to their invariants. A test's
//! tool, never run by the debugger itself.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ns_syntax::Span;

use super::again::{compile_level_again, BodyOrigin, Member};
use super::{address, BodyMetadata, Cache, VariablePlace, VectorHolder};
use crate::bytecode::{CompiledCode, Instruction, Literal};
use crate::compiler::Lowering;
use crate::heap::MixinId;
use crate::interpreter::core_library::{shipped_module_text, LAZY_SHIPPED_MODULES};
use crate::interpreter::instantiate::InitializerBody;
use crate::interpreter::Mutator;
use crate::mixin::{MethodEntry, Mixin};
use crate::{Interpreter, Symbol};

impl Interpreter {
    /// Compiles every body of every mixin of the modules this interpreter
    /// loaded -- the shipped ones too when `shipped`, each lazy shipped
    /// module built first where this interpreter has not built it yet --
    /// again and checks each is alike and its tables hold their
    /// invariants: every statement range within the body and every
    /// `statement_program_counter` range starting at a statement start;
    /// every send entry's index a send's of exactly that call site (a
    /// `SendMirror` for None), and every send named by one; every
    /// variable's registers below the reading body's `local_count` and
    /// every `Outer` hop within its lexical depth; every span within the
    /// module's text. Answers how many bodies it checked, or the first
    /// failure.
    ///
    /// A level is compiled again under the interpreter's lowering, and
    /// under the other one only when the first leaves one of its bodies
    /// unpaired -- an image's code, which either may have compiled
    /// (decision 5) -- and one of the two must pair every body of the
    /// level. Each body's tables are then read through `Cache::metadata`,
    /// the path a stopped frame takes, so that the check holds what the
    /// debugger reads. Only a body past a compiler limit, which compiles
    /// to a body that raises `Literal::BodyTooLarge` first and records no
    /// tables, may have none.
    #[doc(hidden)]
    pub fn check_debug_metadata(&mut self, shipped: bool) -> Result<usize, String> {
        if shipped {
            for name in LAZY_SHIPPED_MODULES {
                let built = self.enter(|mutator| {
                    let symbol = Symbol::intern(name);
                    mutator.shipped_world.module_instance(symbol).is_some()
                        || mutator.shipped_world.pending_class(symbol).is_some()
                });
                if !built {
                    self.build_shipped_module(name, None);
                }
            }
        }
        self.enter(|mutator| check_every_level(mutator, shipped))
    }
}

/// `check_debug_metadata`'s walk over the heap's mixins, inside an entry.
fn check_every_level(mutator: &mut Mutator<'_>, shipped: bool) -> Result<usize, String> {
    let mut count = 0;
    for id in mutator.heap.mixin_ids() {
        let Some(mixin) = mutator.heap.mixin(id) else {
            continue;
        };
        // A class side is checked with its instance side, whose
        // declaration it is compiled again from.
        if mixin.is_class_side() {
            continue;
        }
        let (module_root, _) = mutator.path_from_module_root(id);
        let Some(root) = mutator.heap.mixin(module_root) else {
            return Err(format!("{}'s module has no root", mixin.name));
        };
        if root.shipped && !shipped {
            continue;
        }
        let bodies = level_bodies(mutator, id, &mixin);
        // The placeholder a builtin class is applied over before the
        // kernel fills it, and the mirror's class, are declared in no
        // module: they hold no text and no body but the factory a class
        // side synthesizes from its arity. A declaration that lost its
        // text while it holds a body of its own is a failure.
        let synthesized_only = bodies
            .iter()
            .all(|body| matches!(body.member, Member::Factory));
        if mixin.text.is_none() && synthesized_only {
            continue;
        }
        let text: Rc<str> = if root.shipped {
            shipped_module_text(root.name.as_str())
        } else {
            match mutator.heap.module_source(module_root) {
                Some(text) => Rc::from(text),
                None => return Err(format!("{}'s module has no text", mixin.name)),
            }
        };
        count += check_level(mutator, &mixin, &bodies, &text)?;
    }
    Ok(count)
}

/// One body to check: the side that holds it, the member it belongs to,
/// and the `local_count` of each body lexically around it, innermost
/// first, which its `Outer` places reach.
struct Checked {
    code: Rc<CompiledCode>,
    side: Rc<Mixin>,
    member: Member,
    enclosing: Rc<[u16]>,
}

/// Every body of the level of the instance side `mixin`, the mixin `id`
/// names, and of its class side, declared or synthesized.
fn level_bodies(mutator: &Mutator<'_>, id: MixinId, mixin: &Rc<Mixin>) -> Vec<Checked> {
    let class_side = mixin
        .class_side
        .clone()
        .or_else(|| mutator.metaclass_mixins.get(&id).cloned());
    let mut bodies = Vec::new();
    let mut seen = HashSet::new();
    collect_side(mixin, &mut bodies, &mut seen);
    if let Some(class_side) = &class_side {
        collect_side(class_side, &mut bodies, &mut seen);
    }
    bodies
}

/// Checks `bodies`, every body of the level of the instance side `mixin`
/// and of its class side; answers how many it checked.
fn check_level(
    mutator: &mut Mutator<'_>,
    mixin: &Rc<Mixin>,
    bodies: &[Checked],
    text: &str,
) -> Result<usize, String> {
    let level = format!("{}", mixin.name);
    let first = mutator.lowering;
    let other = match first {
        Lowering::Optimized => Lowering::Reference,
        Lowering::Reference => Lowering::Optimized,
    };
    let mut unpaired = None;
    for lowering in [first, other] {
        let Some(pairs) = compile_level_again(mutator, mixin, lowering) else {
            return Err(format!(
                "{level}: its declaration is not found in its module's text"
            ));
        };
        let paired: HashMap<usize, bool> = pairs
            .iter()
            .map(|(code, metadata)| (address(code), metadata.is_some()))
            .collect();
        unpaired = bodies
            .iter()
            .find(|body| !too_large(&body.code) && paired.get(&address(&body.code)) != Some(&true))
            .map(|body| describe(&level, body));
        if unpaired.is_none() {
            break;
        }
    }
    if let Some(body) = unpaired {
        return Err(format!(
            "{body} does not compile again alike under either lowering"
        ));
    }
    for body in bodies {
        let origin = BodyOrigin {
            mixin: Rc::clone(&body.side),
            member: body.member.clone(),
        };
        let metadata = Cache::metadata(mutator, &body.code, &origin);
        match metadata {
            Some(metadata) => check_tables(body, &metadata, text)
                .map_err(|failure| format!("{}: {failure}", describe(&level, body)))?,
            None if too_large(&body.code) => {}
            None => {
                return Err(format!(
                    "{} has no metadata through the debugger's cache",
                    describe(&level, body)
                ))
            }
        }
    }
    Ok(bodies.len())
}

/// Every body of `side` -- its methods, its factory, its three
/// initializer bodies, its superclass clause -- and the blocks each
/// creates, transitively, each once.
fn collect_side(side: &Rc<Mixin>, bodies: &mut Vec<Checked>, seen: &mut HashSet<usize>) {
    let mut roots: Vec<(Rc<CompiledCode>, Member)> = Vec::new();
    for (selector, entry) in &side.methods {
        match entry {
            MethodEntry::Compiled(method) => {
                roots.push((Rc::clone(&method.code), Member::Method(*selector)));
            }
            MethodEntry::Factory(factory) => {
                roots.push((Rc::clone(&factory.code), Member::Factory));
            }
            MethodEntry::SlotAccessor(_) | MethodEntry::Primitive(_) => {}
        }
    }
    for kind in [
        InitializerBody::Full,
        InitializerBody::OwnPart,
        InitializerBody::Standalone,
    ] {
        if let Some(code) = kind.code_of(side) {
            roots.push((code, Member::Initializer(kind)));
        }
    }
    if let Some(code) = &side.superclass_clause_code {
        roots.push((Rc::clone(code), Member::SuperclassClause));
    }
    for (root, member) in roots {
        // A worklist: an image's body chooses how deep its blocks nest.
        let mut pending = vec![(root, Rc::<[u16]>::from([]))];
        while let Some((code, enclosing)) = pending.pop() {
            if !seen.insert(address(&code)) {
                continue;
            }
            let inside: Rc<[u16]> = std::iter::once(code.local_count)
                .chain(enclosing.iter().copied())
                .collect();
            pending.extend(
                code.body()
                    .created_blocks()
                    .map(|block| (Rc::clone(block.code), Rc::clone(&inside))),
            );
            bodies.push(Checked {
                code,
                side: Rc::clone(side),
                member: member.clone(),
                enclosing,
            });
        }
    }
}

/// Whether `code` is the body a body past a compiler limit compiled to,
/// which raises `Literal::BodyTooLarge` first and records no tables
/// (decision 14).
fn too_large(code: &CompiledCode) -> bool {
    code.instruction_count() > 0
        && matches!(
            code.instruction(0),
            Instruction::LoadLiteral {
                literal: Literal::BodyTooLarge(_),
                ..
            }
        )
}

/// `body` named for a failure.
fn describe(level: &str, body: &Checked) -> String {
    let member = match &body.member {
        Member::Method(selector) => format!(">>{selector}"),
        Member::Initializer(_) => " initializer".to_string(),
        Member::SuperclassClause => " superclass clause".to_string(),
        Member::Factory => " factory".to_string(),
    };
    let side = if body.side.is_class_side() {
        " class"
    } else {
        ""
    };
    let block = if body.enclosing.is_empty() {
        String::new()
    } else {
        format!(" (a block at depth {})", body.enclosing.len())
    };
    format!("{level}{side}{member}{block}")
}

/// Holds `metadata`, the tables of `body`, to their invariants.
fn check_tables(body: &Checked, metadata: &BodyMetadata, text: &str) -> Result<(), String> {
    let code = &body.code;
    let length = u32::try_from(code.instruction_count()).map_err(|_| "too many instructions")?;
    let within_text = |span: Span, what: &str| -> Result<(), String> {
        let start = usize::try_from(span.start).map_err(|_| format!("{what} past the text"))?;
        let end = usize::try_from(span.end).map_err(|_| format!("{what} past the text"))?;
        if start <= end && text.get(start..end).is_some() {
            Ok(())
        } else {
            Err(format!(
                "{what} {}..{} is not within the module's text of {} bytes",
                span.start,
                span.end,
                text.len()
            ))
        }
    };

    for statement in &metadata.statements {
        if statement.instructions.start > statement.instructions.end
            || statement.instructions.end > length
        {
            return Err(format!(
                "the statement range {:?} is not within the body's {length} instructions",
                statement.instructions
            ));
        }
        within_text(statement.span, "a statement's span")?;
    }
    // A factory's tables come from its class header, not from its body,
    // which the compiler synthesizes from its arity: no statement start,
    // so that no step stops in it, and no send (decision 12).
    let factory = matches!(body.member, Member::Factory);
    let lowered_ranges = if factory {
        &[][..]
    } else {
        &code.statement_program_counter[..]
    };
    for (_, range) in lowered_ranges {
        let starts = metadata
            .statements
            .iter()
            .any(|statement| usize::try_from(statement.instructions.start) == Ok(range.start));
        if !starts {
            return Err(format!(
                "the statement_program_counter range {range:?} starts at no statement start"
            ));
        }
    }

    let mut previous: Option<u32> = None;
    for send in &metadata.sends {
        if previous.is_some_and(|previous| previous >= send.index) {
            return Err(format!(
                "the send entries are not ascending at {}",
                send.index
            ));
        }
        previous = Some(send.index);
        if send.index >= length {
            return Err(format!("the send entry {} is past the body", send.index));
        }
        let instruction = code.instruction(send.index as usize);
        let matches = match send.call_site {
            Some(call_site) => instruction.call_site() == Some(call_site),
            None => matches!(instruction, Instruction::SendMirror { .. }),
        };
        if !matches {
            return Err(format!(
                "the send entry {} names {:?}, but the instruction is {instruction:?}",
                send.index, send.call_site
            ));
        }
        if let Some(receiver) = send.receiver {
            within_text(receiver, "a send's receiver span")?;
        }
        if send.selector.is_empty() {
            return Err(format!(
                "the send entry {} has no selector span",
                send.index
            ));
        }
        for selector in &send.selector {
            within_text(*selector, "a send's selector span")?;
        }
    }
    for index in 0..length {
        let instruction = code.instruction(index as usize);
        let is_send = instruction.call_site().is_some()
            || matches!(instruction, Instruction::SendMirror { .. });
        if is_send && !factory && !metadata.sends.iter().any(|send| send.index == index) {
            return Err(format!("the send at {index} has no entry"));
        }
    }

    let depth = body.enclosing.len();
    let outer_local_count = |hops: u16| -> Result<u16, String> {
        let position = usize::from(hops)
            .checked_sub(1)
            .ok_or_else(|| "an outer place of no hops".to_string())?;
        body.enclosing.get(position).copied().ok_or_else(|| {
            format!("an outer place {hops} hops out, past the body's lexical depth {depth}")
        })
    };
    let register_below = |register: u16, local_count: u16, what: &str| -> Result<(), String> {
        if register < local_count {
            Ok(())
        } else {
            Err(format!(
                "{what} register {register} is not below the local count {local_count}"
            ))
        }
    };
    for variable in &metadata.variables {
        if usize::from(variable.depth) > depth {
            return Err(format!(
                "the variable {} is declared {} scopes out, past the lexical depth {depth}",
                variable.name, variable.depth
            ));
        }
        let what = format!("the variable {}'s", variable.name);
        match variable.place {
            VariablePlace::Register(register) => {
                register_below(register, code.local_count, &what)?;
            }
            VariablePlace::Outer { hops, register } => {
                register_below(register, outer_local_count(hops)?, &what)?;
            }
            VariablePlace::Vector { vector, .. } => match vector {
                VectorHolder::Register(register) => {
                    register_below(register, code.local_count, &what)?;
                }
                VectorHolder::Outer { hops, register } => {
                    register_below(register, outer_local_count(hops)?, &what)?;
                }
            },
        }
    }

    match (&body.member, metadata.factory_pattern) {
        (Member::Factory, Some(pattern)) => within_text(pattern, "the factory's pattern")?,
        (Member::Factory, None) => return Err("the factory stands nowhere".to_string()),
        (_, Some(_)) => return Err("a body the lowerers compile has a factory pattern".into()),
        (_, None) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use indoc::indoc;

    use super::*;
    use crate::bytecode::CallSite;

    const SOURCE: &str = indoc! {"
        class Root = Object new (
        ) (
          public m: x = ( | total ::= 0. | total:: x. ^[:y | total + y foo ] )
        ) : ()
    "};

    fn loaded(lowering: Lowering) -> Interpreter {
        let module = ns_syntax::parse(SOURCE).expect("parse");
        let resolved = match crate::resolve(
            module,
            Rc::from(SOURCE),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("does not resolve"),
        };
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        interpreter.load_module(&resolved);
        interpreter
    }

    /// The block `Root>>m:` creates, as the check walks it, and its tables
    /// through the debugger's cache.
    fn the_block(mutator: &mut Mutator<'_>) -> (Checked, BodyMetadata) {
        let root = mutator
            .heap
            .mixin_ids()
            .into_iter()
            .filter_map(|id| Some((id, mutator.heap.mixin(id)?)))
            .find(|(_, mixin)| mixin.name.as_str() == "Root" && !mixin.shipped)
            .expect("the module's root");
        let block = level_bodies(mutator, root.0, &root.1)
            .into_iter()
            .find(|body| !body.enclosing.is_empty())
            .expect("the method's block");
        let origin = BodyOrigin {
            mixin: Rc::clone(&block.side),
            member: block.member.clone(),
        };
        let metadata = Cache::metadata(mutator, &block.code, &origin).expect("the block's tables");
        (block, (*metadata).clone())
    }

    crate::for_each_lowering! {
        fn a_program_s_bodies_are_checked(lowering) {
            let mut interpreter = loaded(lowering);
            match interpreter.check_debug_metadata(false) {
                // The method, its block, the factory and the initializer
                // bodies.
                Ok(count) => assert!(count >= 4, "{count} bodies"),
                Err(failure) => panic!("{failure}"),
            }
        }
    }

    crate::for_each_lowering! {
        fn a_broken_table_fails_the_check(lowering) {
            let mut interpreter = loaded(lowering);
            interpreter.enter(|mutator| {
                let (block, metadata) = the_block(mutator);
                assert_eq!(check_tables(&block, &metadata, SOURCE), Ok(()));
                let length = u32::try_from(block.code.instruction_count()).expect("small");
                let past_the_text = u32::try_from(SOURCE.len() + 1).expect("small");
                let mut broken: Vec<(&str, BodyMetadata)> = Vec::new();

                let mut statement_past_the_body = metadata.clone();
                statement_past_the_body.statements[0].instructions.end = length + 1;
                broken.push(("a statement past the body", statement_past_the_body));

                let mut no_statement_start = metadata.clone();
                no_statement_start.statements = Box::new([]);
                broken.push(("a range at no statement start", no_statement_start));

                let mut span_past_the_text = metadata.clone();
                span_past_the_text.sends[0].selector = Box::new([Span::new(0, past_the_text)]);
                broken.push(("a span past the text", span_past_the_text));

                let mut other_call_site = metadata.clone();
                other_call_site.sends[0].call_site = Some(CallSite { number: 1_000 });
                broken.push(("a send entry of another call site", other_call_site));

                let mut send_without_entry = metadata.clone();
                send_without_entry.sends = Box::new([]);
                broken.push(("a send without an entry", send_without_entry));

                let mut register_past_the_locals = metadata.clone();
                register_past_the_locals.variables[0].place =
                    VariablePlace::Register(block.code.local_count);
                broken.push(("a register past the locals", register_past_the_locals));

                let mut hops_past_the_depth = metadata.clone();
                hops_past_the_depth.variables[0].place =
                    VariablePlace::Outer { hops: 2, register: 0 };
                broken.push(("an outer place past the lexical depth", hops_past_the_depth));

                let mut a_factory_pattern = metadata;
                a_factory_pattern.factory_pattern = Some(Span::new(0, 1));
                broken.push(("a lowered body with a factory pattern", a_factory_pattern));

                for (what, metadata) in broken {
                    assert!(check_tables(&block, &metadata, SOURCE).is_err(), "{what}");
                }
            });
        }
    }
}
