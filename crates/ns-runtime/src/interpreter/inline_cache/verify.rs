//! The install stress mode's check (`NS_NEXT_STRESS_INSTALL`): every
//! entry the caches hold -- each site's, each polymorphic slot's, each
//! global entry and the verified slot level -- compared with what a fresh
//! lookup decides for its key. An install vacates only the entries naming
//! what it re-applied (`InlineCaches::forget_stale`), so an entry it
//! should have vacated and kept would be run at its next hit; the check
//! finds it at the commit instead (`docs/design/dispatch.md`,
//! "Invalidation").
//!
//! An entry's key names an application by its dispatch id, never a
//! handle, so the check finds one object or class object of each id in one
//! walk of the space and looks up from it; an entry whose guard no live
//! object has can never hit, and is skipped. It calls the lookups alone,
//! never `act`, so that it changes no cache state it inspects.

use std::rc::Rc;

use super::{
    CacheEntry, CacheRule, InlineCaches, SendKey, Target, chain_start, levels_between, nested_field,
};
use crate::Symbol;
use crate::bytecode::{CallSite, CompiledCode, Instruction};
use crate::hash::IdentityMap;
use crate::interpreter::Mutator;
use crate::interpreter::resolve::{Chain, Resolved};
use crate::mixin::DispatchId;
use crate::types::*;

/// The kind of send a site's instruction makes, which decides the rule
/// its entries were recorded under.
#[derive(Clone, Copy)]
enum SendKind {
    Public,
    Lexical,
    Super,
}

impl SendKind {
    fn name(self) -> &'static str {
        match self {
            SendKind::Public => "public",
            SendKind::Lexical => "lexical",
            SendKind::Super => "super",
        }
    }
}

/// One filled entry, copied out of the caches so that the lookups may
/// borrow the mutator: where it stands, its key and its target.
struct Recorded {
    place: String,
    source: Source,
    guard: DispatchId,
    rule: CacheRule,
    selector: Symbol,
    target: Target,
}

/// Where an entry stands.
#[derive(Clone, Copy)]
enum Source {
    Site,
    PolymorphicSlot,
    Global,
}

/// How many entries the check compared with a lookup, by where they stand
/// and by the rule they were recorded under: what a test reads to see that
/// no kind of entry is always skipped.
#[derive(Default, Debug)]
pub(super) struct Compared {
    pub(super) sites: usize,
    pub(super) polymorphic_slots: usize,
    pub(super) global_entries: usize,
    pub(super) public: usize,
    pub(super) lexical: usize,
    pub(super) private: usize,
    pub(super) super_sends: usize,
}

impl Compared {
    fn count(&mut self, recorded: &Recorded) {
        match recorded.source {
            Source::Site => self.sites += 1,
            Source::PolymorphicSlot => self.polymorphic_slots += 1,
            Source::Global => self.global_entries += 1,
        }
        match recorded.rule {
            CacheRule::Public => self.public += 1,
            CacheRule::Lexical(_) => self.lexical += 1,
            CacheRule::LexicalPrivate => self.private += 1,
            CacheRule::Super => self.super_sends += 1,
        }
    }
}

/// One live receiver of each dispatch id and one class object of each
/// application's instances id, found in one walk of the space: what a
/// lookup for an entry's key starts from.
struct Receivers<'gc> {
    by_guard: IdentityMap<DispatchId, Value<'gc>>,
    classes: IdentityMap<DispatchId, ClassReference<'gc>>,
}

impl<'gc> Receivers<'gc> {
    fn walk(mutator: &Mutator<'gc>) -> Receivers<'gc> {
        let heap = &mutator.heap;
        let (classes, objects) = heap.classes_and_instances_over(|_| true);
        let mut by_guard: IdentityMap<DispatchId, Value<'gc>> = IdentityMap::default();
        // The immediates are in no space; their ids are in a table.
        let immediates = [
            Some(Value::nil()),
            Some(Value::boolean(true)),
            Some(Value::boolean(false)),
            Some(Value::small_integer(0)),
            crate::heap::Immediate::small_float(1.0).map(Value::from_immediate),
        ];
        for value in immediates
            .into_iter()
            .flatten()
            .chain(objects.into_iter().map(Value::object))
        {
            let guard = heap.dispatch_id(value);
            if guard != DispatchId::NONE {
                by_guard.entry(guard).or_insert(value);
            }
        }
        let mut by_instances: IdentityMap<DispatchId, ClassReference<'gc>> = IdentityMap::default();
        for class in classes {
            let instances = heap.class_dispatch_ids(class).instances;
            if instances != DispatchId::NONE {
                by_instances.entry(instances).or_insert(class);
            }
        }
        Receivers {
            by_guard,
            classes: by_instances,
        }
    }
}

impl InlineCaches {
    /// Each site of every family that has run, by its index in the table,
    /// with the selector and the kind of send of its instruction. A site of
    /// a loose range, a closure's an image decoded, has no family whose
    /// body would say, and is not answered.
    fn site_sends(&self) -> IdentityMap<usize, (Symbol, SendKind)> {
        let mut sends = IdentityMap::default();
        for family in self.families.values() {
            let mut pending: Vec<Rc<CompiledCode>> = vec![Rc::clone(&family.code)];
            while let Some(body) = pending.pop() {
                for instruction in &body.instructions() {
                    // No `_` arm, so that a new cached send must say which
                    // rule its entries are recorded under.
                    let kind = match instruction {
                        Instruction::SendPublic { .. } | Instruction::SendInlinable { .. } => {
                            SendKind::Public
                        }
                        Instruction::SendLexical { .. } => SendKind::Lexical,
                        Instruction::SendSuper { .. } => SendKind::Super,
                        Instruction::MakeClosure { code: block, .. } => {
                            pending.push(Rc::clone(block));
                            continue;
                        }
                        Instruction::SendMirror { .. }
                        | Instruction::InitializeSuperclass { .. }
                        | Instruction::InitializeAppliedLevel { .. }
                        | Instruction::CheckAppliedClause { .. }
                        | Instruction::LoadLiteral { .. }
                        | Instruction::LoadSelf { .. }
                        | Instruction::LoadEnclosingObject { .. }
                        | Instruction::Move { .. }
                        | Instruction::StoreLocal { .. }
                        | Instruction::LoadOuterLocal { .. }
                        | Instruction::StoreOuterLocal { .. }
                        | Instruction::MakeArray { .. }
                        | Instruction::MakeVector { .. }
                        | Instruction::LoadVector { .. }
                        | Instruction::StoreVector { .. }
                        | Instruction::InitializeSlot { .. }
                        | Instruction::ReturnSlotIfFilled { .. }
                        | Instruction::MakeFuture { .. }
                        | Instruction::ResolveFuture { .. }
                        | Instruction::LoadOwnSlot { .. }
                        | Instruction::InitializeSlotProvisionally { .. }
                        | Instruction::Return { .. }
                        | Instruction::ImplicitReturn { .. }
                        | Instruction::NonLocalReturn { .. }
                        | Instruction::SignalNonBoolean { .. }
                        | Instruction::Jump { .. }
                        | Instruction::JumpIfNotBoolean { .. }
                        | Instruction::BranchIfFalse { .. }
                        | Instruction::BranchIfTrue { .. }
                        | Instruction::IntegerBinary { .. }
                        | Instruction::AllocateInstance { .. }
                        | Instruction::InitializeInstance { .. } => continue,
                    };
                    let (Some(site), Some((selector, _))) =
                        (instruction.call_site(), instruction.sent_message())
                    else {
                        continue;
                    };
                    if site == CallSite::UNCACHED {
                        continue;
                    }
                    if let Some(index) =
                        super::site_index(family.base, family.code.family_size, site)
                    {
                        sends.insert(index, (selector, kind));
                    }
                }
            }
        }
        sends
    }

    /// Every filled entry, copied out with its place and its key.
    fn recorded_entries(&self) -> Vec<Recorded> {
        let sends = self.site_sends();
        let mut recorded = Vec::new();
        // The rule an entry of a site was recorded under: a lexical site's
        // private entry is keyed by N alone, any other by its receiver. A
        // site's key does not carry a non-private lexical send's N, so its
        // entry is looked up without N's private member, which the global
        // entry of the same send, whose rule names N, is checked with.
        let rule_of = |kind: SendKind, target: &Target| match kind {
            SendKind::Public => CacheRule::Public,
            SendKind::Super => CacheRule::Super,
            SendKind::Lexical if target.site_privacy() == Some(true) => CacheRule::LexicalPrivate,
            SendKind::Lexical => CacheRule::Lexical(DispatchId::NONE),
        };
        let mut push = |place: String,
                        source: Source,
                        entry: &CacheEntry,
                        selector: Symbol,
                        kind: SendKind| {
            recorded.push(Recorded {
                place,
                source,
                guard: entry.guard,
                rule: rule_of(kind, &entry.target),
                selector,
                target: entry.target.clone(),
            });
        };
        for (index, entry) in self.sites.iter().enumerate() {
            let Some(&(selector, kind)) = sends.get(&index) else {
                continue;
            };
            let site = |slot: Option<usize>| {
                let send = format!("a {} send of #{selector}", kind.name());
                match slot {
                    Some(position) => format!("slot {position} of site {index}, {send},"),
                    None => format!("site {index}, {send},"),
                }
            };
            match entry.target {
                Target::Vacant | Target::Megamorphic { .. } => {}
                Target::Polymorphic { block, .. } => {
                    for (position, slot) in self
                        .pool
                        .get(block as usize)
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        if !matches!(slot.target, Target::Vacant) {
                            push(
                                site(Some(position)),
                                Source::PolymorphicSlot,
                                slot,
                                selector,
                                kind,
                            );
                        }
                    }
                }
                _ => push(site(None), Source::Site, entry, selector, kind),
            }
        }
        for (index, entry) in self.global.entries.iter().enumerate() {
            if entry.is_vacant() {
                continue;
            }
            recorded.push(Recorded {
                place: format!(
                    "global entry {index}, #{} under {:?},",
                    entry.selector, entry.rule
                ),
                source: Source::Global,
                guard: entry.guard,
                rule: entry.rule,
                selector: entry.selector,
                target: entry.target.clone(),
            });
        }
        recorded
    }
}

impl<'gc> Mutator<'gc> {
    /// Whether every entry the caches hold is what a fresh lookup decides
    /// for its key now -- each site's, each polymorphic slot's, each
    /// global entry and the verified slot level -- answering the first that
    /// is not, described. The install stress mode runs it after every
    /// commit (`Mutator::commit_install`). It calls the lookups alone and
    /// leaves every cache as it found it.
    pub(crate) fn verify_caches_against_lookups(&mut self) -> Result<(), String> {
        self.compare_caches_with_lookups().map(|_| ())
    }

    /// `verify_caches_against_lookups`, answering how many entries of
    /// each kind it compared.
    pub(super) fn compare_caches_with_lookups(&mut self) -> Result<Compared, String> {
        let receivers = Receivers::walk(self);
        let mut compared = Compared::default();
        for recorded in self.caches.recorded_entries() {
            if self.verify_entry(&receivers, &recorded)? {
                compared.count(&recorded);
            }
        }
        if let Some((guard, level)) = self.caches.verified_slot_level
            && let Some(receiver) = receivers.by_guard.get(&guard)
            && !self.heap.receiver_has_level(*receiver, level)
        {
            return Err(format!(
                "the verified slot level caches level {} for the guard {} where the lookup \
                 decides a receiver with that guard lacks the level",
                level.index(),
                guard.bits()
            ));
        }
        Ok(compared)
    }

    /// `recorded` against a fresh lookup for its key, answering whether it
    /// was compared: not for a key no live object has, which no send can
    /// present.
    fn verify_entry(
        &mut self,
        receivers: &Receivers<'gc>,
        recorded: &Recorded,
    ) -> Result<bool, String> {
        let guard = recorded.guard;
        let class_of_id = |id: DispatchId| receivers.classes.get(&id).copied();
        let (receiver, lexical_class, superclass) = match recorded.rule {
            CacheRule::Public => {
                let Some(receiver) = receivers.by_guard.get(&guard) else {
                    return Ok(false);
                };
                (*receiver, None, None)
            }
            CacheRule::Lexical(lexical_id) => {
                let Some(receiver) = receivers.by_guard.get(&guard) else {
                    return Ok(false);
                };
                let lexical_class = if lexical_id == DispatchId::NONE {
                    None
                } else {
                    let Some(lexical_class) = class_of_id(lexical_id) else {
                        return Ok(false);
                    };
                    Some(lexical_class)
                };
                (*receiver, lexical_class, None)
            }
            // §5.9's private rule reads N alone; a receiver is needed only
            // where N no longer has the member, which the chain then tells.
            CacheRule::LexicalPrivate => {
                let Some(lexical_class) = class_of_id(guard) else {
                    return Ok(false);
                };
                (Value::object(lexical_class), Some(lexical_class), None)
            }
            CacheRule::Super => {
                let Some(superclass) = class_of_id(guard) else {
                    return Ok(false);
                };
                (Value::object(superclass), None, Some(superclass))
            }
        };
        let selector = recorded.selector;
        let resolved = match recorded.rule {
            CacheRule::Public => self.resolve_public(receiver, selector),
            CacheRule::Lexical(_) | CacheRule::LexicalPrivate => {
                self.resolve_lexical(receiver, lexical_class, selector)
            }
            CacheRule::Super => match superclass {
                Some(superclass) => self.resolve_super(superclass, selector),
                None => Resolved::Unhandled,
            },
        };
        let key = SendKey {
            guard,
            rule: recorded.rule,
            receiver: &receiver,
            lexical_class,
            superclass,
            current_class: None,
        };
        if self.target_agrees(&recorded.target, &resolved, key)
            && self.recorded_guard_holds(receivers, &recorded.target)
        {
            return Ok(true);
        }
        Err(format!(
            "{} caches {} where the lookup decides {}",
            recorded.place,
            describe_target(&recorded.target),
            self.describe_resolved(&resolved, key)
        ))
    }

    /// Whether `target` is what an entry records for `resolved` under
    /// `key` (`inline_cache::record`): the same member, found on the same
    /// chain the same number of levels up from where the chain starts.
    fn target_agrees(
        &mut self,
        target: &Target,
        resolved: &Resolved<'gc>,
        key: SendKey<'_, 'gc>,
    ) -> bool {
        match (target, resolved) {
            (
                Target::Method {
                    method,
                    level,
                    chain,
                    ..
                },
                Resolved::Method {
                    method: found,
                    defining_class,
                    chain: found_chain,
                },
            ) => {
                chain == found_chain
                    && Rc::ptr_eq(method, found)
                    && self.levels_up(*chain, key, *defining_class) == Some(*level)
            }
            (
                Target::Slot {
                    field,
                    kind,
                    chain,
                    level,
                    ..
                },
                Resolved::Slot {
                    kind: found_kind,
                    field: found_field,
                    chain: found_chain,
                    level: found_level,
                    ..
                },
            ) => {
                chain == found_chain
                    && kind == found_kind
                    && *field as usize == *found_field
                    && level == found_level
            }
            (
                Target::NestedClass {
                    mixin,
                    field,
                    level,
                    chain,
                },
                Resolved::NestedClass {
                    mixin: found,
                    declaring_class,
                    chain: found_chain,
                    ..
                },
            ) => {
                chain == found_chain
                    && Rc::ptr_eq(mixin, found)
                    && self.levels_up(*chain, key, *declaring_class) == Some(*level)
                    && *field == nested_field(self, *chain, key, mixin)
            }
            (
                Target::Primitive {
                    primitive,
                    level,
                    chain,
                },
                Resolved::Primitive {
                    primitive: found,
                    level: found_level,
                    chain: found_chain,
                },
            ) => {
                chain == found_chain
                    && primitive == found
                    && self.levels_up(*chain, key, *found_level) == Some(*level)
            }
            (
                Target::Factory { code, chain },
                Resolved::Factory {
                    code: found,
                    chain: found_chain,
                    ..
                },
            ) => chain == found_chain && Rc::ptr_eq(code, found),
            _ => false,
        }
    }

    /// How many superclass steps from where `chain` starts for `key`
    /// reach `level`.
    fn levels_up(
        &mut self,
        chain: Chain,
        key: SendKey<'_, 'gc>,
        level: ClassReference<'gc>,
    ) -> Option<u16> {
        let start = chain_start(self, chain, key)?;
        levels_between(&self.heap, start, level)
    }

    /// Whether a slot target's recorded guard still names receivers with
    /// its level: the one compare a hit's slot check passes on
    /// (`Mutator::slot_level_holds`). A guard no live object has passes,
    /// since no receiver presents it.
    fn recorded_guard_holds(&self, receivers: &Receivers<'gc>, target: &Target) -> bool {
        let Target::Slot {
            level,
            recorded_guard,
            ..
        } = target
        else {
            return true;
        };
        *recorded_guard == DispatchId::NONE
            || receivers
                .by_guard
                .get(recorded_guard)
                .is_none_or(|receiver| self.heap.receiver_has_level(*receiver, *level))
    }

    /// `resolved` described as `describe_target` describes an entry.
    fn describe_resolved(&mut self, resolved: &Resolved<'gc>, key: SendKey<'_, 'gc>) -> String {
        let levels = |levels: Option<u16>| {
            levels.map_or_else(|| "no level".to_owned(), |levels| format!("level {levels}"))
        };
        match resolved {
            Resolved::Method {
                method,
                defining_class,
                chain,
            } => format!(
                "the method #{} at {} of the {chain:?} chain",
                method.selector,
                levels(self.levels_up(*chain, key, *defining_class))
            ),
            Resolved::Slot {
                kind,
                field,
                chain,
                level,
                ..
            } => format!(
                "the {kind:?} of field {field} of application {} on the {chain:?} chain",
                level.index()
            ),
            Resolved::NestedClass {
                mixin,
                declaring_class,
                chain,
                ..
            } => format!(
                "the nested class {} at {} of the {chain:?} chain, hidden slot {}",
                mixin.reflected_name(),
                levels(self.levels_up(*chain, key, *declaring_class)),
                nested_field(self, *chain, key, mixin)
            ),
            Resolved::Primitive {
                primitive,
                level,
                chain,
            } => format!(
                "the primitive {primitive:?} at {} of the {chain:?} chain",
                levels(self.levels_up(*chain, key, *level))
            ),
            Resolved::Factory { chain, .. } => format!("a factory on the {chain:?} chain"),
            Resolved::Unhandled => "nothing".to_owned(),
        }
    }
}

/// An entry's target, for the check's report.
fn describe_target(target: &Target) -> String {
    match target {
        Target::Method {
            method,
            level,
            chain,
            ..
        } => format!(
            "the method #{} at level {level} of the {chain:?} chain",
            method.selector
        ),
        Target::Slot {
            field,
            kind,
            chain,
            level,
            recorded_guard,
        } => format!(
            "the {kind:?} of field {field} of application {} on the {chain:?} chain, \
             recorded for the guard {}",
            level.index(),
            recorded_guard.bits()
        ),
        Target::NestedClass {
            mixin,
            field,
            level,
            chain,
        } => format!(
            "the nested class {} at level {level} of the {chain:?} chain, hidden slot {field}",
            mixin.reflected_name()
        ),
        Target::Primitive {
            primitive,
            level,
            chain,
        } => format!("the primitive {primitive:?} at level {level} of the {chain:?} chain"),
        Target::Factory { chain, .. } => format!("a factory on the {chain:?} chain"),
        Target::Vacant => "nothing".to_owned(),
        Target::Polymorphic { .. } | Target::Megamorphic { .. } => "a site state".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::edit::{Edit, MemberEdit};

    fn expect_ok(result: crate::HostResult, what: &str) -> crate::Root {
        match result {
            Ok(root) => root,
            Err(crate::HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(crate::HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// `Edited` and `Untouched`, each sent `value` by `warm:and:`, and
    /// both by the one site of `poly:`, which so goes polymorphic; a
    /// subclass and a `<:` level over `Edited`, each sent `value` by
    /// `warmLevels`, which also sends `new` to every class; a lexical send
    /// of `Edited`'s `helper` from its nested `Deep`, a `super` send from
    /// `Sub`, and a private lexical send in `Untouched`.
    const PROGRAM: &str = indoc::indoc! {"
        class Program = Object new () (
          public class Edited = Object new () (
            public value = ( ^1 )
            public helper = ( ^2 )
            public class Deep = Object new () (
              public run = ( ^helper )
            )
          )
          public class Untouched = Object new () (
            public value = ( ^hidden )
            private hidden = ( ^3 )
          )
          public class Sub = Edited () (
            public value = ( ^super value + 10 )
          )
          public class Mixed = Object new <: Edited () ()
          public warm: edited and: untouched = (
            edited value.
            untouched value.
            self poly: edited.
            self poly: untouched.
            ^0
          )
          public poly: receiver = ( ^receiver value )
          public warmLevels = (
            Sub new value.
            Mixed new value.
            Edited new.
            Untouched new.
            ^0
          )
          public classes = (
            Edited.
            Untouched.
            ^0
          )
        ) : ()
    "};

    /// The program loaded and instantiated with its sends warmed twice, no
    /// stress mode vacating the caches between the warming and the check;
    /// the program and the two instances `warm:and:` is sent.
    fn warmed() -> (crate::Interpreter, crate::Root, crate::Root, crate::Root) {
        let module = ns_syntax::parse(PROGRAM).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(PROGRAM),
            crate::types::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.set_stress_caches(false);
        interpreter.set_stress_breakpoints(false);
        interpreter.set_stress_step(false);
        interpreter.set_jit_stress(true);
        let class = interpreter.load_module(&resolved);
        let program = expect_ok(interpreter.instantiate(&class, &[]), "a Program");
        let instance_of = |interpreter: &mut crate::Interpreter, name: &str| {
            let class = expect_ok(interpreter.send(&program, Symbol::intern(name), &[]), name);
            expect_ok(interpreter.send(&class, Symbol::NEW, &[]), name)
        };
        let edited = instance_of(&mut interpreter, "Edited");
        let untouched = instance_of(&mut interpreter, "Untouched");
        let deep_class = expect_ok(
            interpreter.send(&edited, Symbol::intern("Deep"), &[]),
            "Deep",
        );
        let deep = expect_ok(interpreter.send(&deep_class, Symbol::NEW, &[]), "a Deep");
        warm(&mut interpreter, &program, &edited, &untouched);
        expect_ok(interpreter.send(&deep, Symbol::intern("run"), &[]), "run");
        (interpreter, program, edited, untouched)
    }

    /// Runs every send of the program twice.
    fn warm(
        interpreter: &mut crate::Interpreter,
        program: &crate::Root,
        edited: &crate::Root,
        untouched: &crate::Root,
    ) {
        for _ in 0..2 {
            expect_ok(
                interpreter.send(
                    program,
                    Symbol::intern("warm:and:"),
                    &[edited.clone(), untouched.clone()],
                ),
                "warm:and:",
            );
            for selector in ["warmLevels", "classes"] {
                expect_ok(
                    interpreter.send(program, Symbol::intern(selector), &[]),
                    selector,
                );
            }
        }
    }

    /// The sites of `Program`'s `method` sending `value`, in emission
    /// order.
    fn value_sites(mutator: &Mutator<'_>, program: &crate::Root, method: &str) -> Vec<usize> {
        let program_class = mutator.class_of(mutator.fetch(program));
        let code = mutator
            .heap
            .class_application(program_class)
            .expect("applied")
            .mixin
            .compiled_method(Symbol::intern(method))
            .expect("Program has the method")
            .code
            .clone();
        mutator
            .caches
            .site_indices_of(&code, Symbol::intern("value"))
            .into_iter()
            .map(|site| site.expect("warmed"))
            .collect()
    }

    /// The check's report, which must find a stale entry.
    fn problem_found(mutator: &mut Mutator<'_>) -> String {
        let problem = mutator
            .verify_caches_against_lookups()
            .expect_err("the stale entry is found");
        assert!(problem.contains("where the lookup decides"), "{problem}");
        problem
    }

    /// An entry that is not what the lookup decides for its key -- here
    /// `Untouched>>value` under `Edited`'s guard, written in turn into a
    /// monomorphic site, a polymorphic slot and a global entry -- is
    /// reported with its place, where the warm caches it was written into
    /// passed.
    // spec: vm.dispatch.cache-records-lookup
    #[test]
    fn a_stale_entry_is_reported() {
        let (mut interpreter, program, edited, _) = warmed();
        interpreter.enter(|mutator| {
            assert_eq!(mutator.verify_caches_against_lookups(), Ok(()));
            let warm_sites = value_sites(mutator, &program, "warm:and:");
            let (edited_site, untouched_site) = (warm_sites[0], warm_sites[1]);
            let wrong = mutator.caches.sites[untouched_site].target.clone();

            let kept =
                std::mem::replace(&mut mutator.caches.sites[edited_site].target, wrong.clone());
            let problem = problem_found(mutator);
            assert!(
                problem.starts_with(&format!("site {edited_site}, a public send of #value,")),
                "{problem}"
            );
            mutator.caches.sites[edited_site].target = kept;
            assert_eq!(mutator.verify_caches_against_lookups(), Ok(()));

            let poly_site = value_sites(mutator, &program, "poly:")[0];
            let Target::Polymorphic { block, .. } = mutator.caches.sites[poly_site].target else {
                panic!("poly:'s site is polymorphic");
            };
            let slots = &mut mutator.caches.pool[block as usize];
            assert!(matches!(slots[0].target, Target::Method { .. }));
            let other = slots[1].target.clone();
            let kept = std::mem::replace(&mut slots[0].target, other);
            let problem = problem_found(mutator);
            assert!(
                problem.starts_with(&format!(
                    "slot 0 of site {poly_site}, a public send of #value,"
                )),
                "{problem}"
            );
            mutator.caches.pool[block as usize][0].target = kept;
            assert_eq!(mutator.verify_caches_against_lookups(), Ok(()));

            let edited_guard = mutator.heap.dispatch_id(mutator.fetch(&edited));
            let global = mutator
                .caches
                .global
                .entries
                .iter()
                .position(|entry| {
                    !entry.is_vacant()
                        && entry.guard == edited_guard
                        && entry.selector == Symbol::intern("value")
                        && entry.rule == CacheRule::Public
                })
                .expect("Edited's value is in the global cache");
            mutator.caches.global.entries[global].target = wrong;
            let problem = problem_found(mutator);
            assert!(
                problem.starts_with(&format!("global entry {global}, #value under Public,")),
                "{problem}"
            );
        });
    }

    /// After an install the caches kept, and those the program fills
    /// again, are what the lookups decide -- the install stress mode's
    /// own check at the commit among them -- every kind of entry compared,
    /// and the check changes no cache.
    // spec: vm.dispatch.cache-records-lookup
    // spec: vm.dispatch.invalidation-after-change
    #[test]
    fn a_warm_cache_after_an_install_passes() {
        let (mut interpreter, program, edited, untouched) = warmed();
        interpreter.set_stress_install(true);
        interpreter.enter(|mutator| {
            let edited_class = mutator.class_of(mutator.fetch(&edited));
            let edited_id = mutator
                .heap
                .known_mixin_id(
                    &mutator
                        .heap
                        .class_application(edited_class)
                        .expect("applied")
                        .mixin,
                )
                .expect("registered");
            let edit = Edit {
                header: None,
                instance_side: vec![MemberEdit::AddMethod("public value = ( ^5 )".to_string())],
                class_side: Vec::new(),
                new_name: None,
            };
            if let Err(diagnostic) = mutator.install_one(edited_id, &edit) {
                panic!("the install was refused: {diagnostic}");
            }
        });
        warm(&mut interpreter, &program, &edited, &untouched);
        interpreter.enter(|mutator| {
            let before = mutator.caches.statistics();
            assert!(before.filled_sites > 0, "the caches are warm");
            let compared = match mutator.compare_caches_with_lookups() {
                Ok(compared) => compared,
                Err(problem) => panic!("a stale entry after the install: {problem}"),
            };
            // Every kind of entry was compared, none always skipped.
            for (kind, count) in [
                ("site", compared.sites),
                ("polymorphic slot", compared.polymorphic_slots),
                ("global entry", compared.global_entries),
                ("public", compared.public),
                ("lexical", compared.lexical),
                ("private lexical", compared.private),
                ("super", compared.super_sends),
            ] {
                assert!(count > 0, "no {kind} entry was compared: {compared:?}");
            }
            assert_eq!(
                mutator.caches.statistics(),
                before,
                "the check changes no cache"
            );
        });
    }
}
