//! The three lookup algorithms and the kth-enclosing-object computation.
//! See `docs/design/dispatch.md`; cites `newspeak-spec.pdf`
//! §3.3, §5.5, §5.9, §5.10, §6.3.1.

use std::rc::Rc;

use ns_syntax::ast;

use crate::builtins::primitive::Primitive;
use crate::heap::{DeclarationScope, Heap};
use crate::mixin::{MethodEntry, Mixin, MixinApplication, MixinMethod, SlotAccessor};
use crate::types::*;
use crate::Symbol;

pub enum Lookup<'gc> {
    Method(Rc<MixinMethod>, ClassReference<'gc>),
    /// One of the accessors a slot declaration defines (`newspeak-spec.pdf`
    /// §6.3.2), with the absolute field it reads or writes: the declaring
    /// application's `own_slot_base` plus the slot's mixin-relative index --
    /// and that application's index, the level whose slots the field is
    /// one of, which a private or super accessor's guard check asks the
    /// receiver's chain for (`interpreter::slots`, `slot_level_holds`).
    SlotAccessor(SlotAccessor, usize, crate::heap::ApplicationIndex),
    /// A nested class declaration, reached through the implicit accessor
    /// `newspeak-spec.pdf` §6 says every such declaration defines in its
    /// enclosing class. The `ClassReference` is the class object of the level
    /// that declares it -- the same "defining class" a `Lookup::Method`
    /// reports, so a caller can treat both arms uniformly; the nested class
    /// object itself does not exist until `interpreter::nested::nested_class`
    /// materializes it for a particular receiver.
    NestedClass(Rc<Mixin>, ClassReference<'gc>),
    /// A primitive entry -- a row of the VM mirror, the only builtin whose
    /// method table holds any -- with the class object of the level whose
    /// method table holds it, which an inline cache counts its level from.
    /// A row never passes its level over: its failure path is the failure
    /// block its send carries.
    Primitive(Primitive, ClassReference<'gc>),
    /// §6.3's primary factory, with the class object of the level whose
    /// method table holds it, as `Primitive` carries its level. §6.3
    /// instantiates the *receiver* of the factory message, not the level;
    /// the level is what lets the resolution honour the entry only when it
    /// was found on the receiver's own class, its metaclass
    /// (`interpreter::resolve`'s `resolved_for`). The body is the entry's,
    /// which the factory send activates.
    Factory(ClassReference<'gc>, Rc<crate::bytecode::CompiledCode>),
    NotFound,
}

pub fn effective_access(access: Option<ast::AccessModifier>) -> ast::AccessModifier {
    access.unwrap_or(ast::AccessModifier::Protected)
}

/// The accessibility of the implicit accessor a nested declaration defines: §6,
/// "implicitly defines a method with the same name *and accessibility* in OC".
/// Stored on the mixin, already defaulted, because the image's mixin table has
/// to carry it: a decoded mixin has no `ast::ClassDeclaration` to read it back
/// off, and a lookup that guessed would answer `Protected` for a declaration
/// written `public class Inner = ...`.
pub fn nested_access(mixin: &Mixin) -> ast::AccessModifier {
    mixin.access
}

/// Where a running activation's lexical walk starts: §5.9's C', "the
/// current class of the current activation", as the code that runs sees
/// it (`interpreter::sends::lexical_origin` decides which).
#[derive(Clone, Copy)]
pub enum LexicalOrigin<'gc> {
    /// The class that defines the running code, the activation's
    /// `current_class`, `None` for a top-level activation: depth 0's N,
    /// and the level whose enclosing object and declaring class the walk
    /// reads at depth 1.
    Class(Option<ClassReference<'gc>>),
    /// The own code of a body-less declaration -- its full initializer,
    /// which is its clause messages and their arguments, and the blocks in
    /// them -- running at its class object. That class object is the level
    /// the declaration's last clause added (§6.2), whose mixin, enclosing
    /// object and declaring class are the named class's; C' is the
    /// declaration, the class a body version of it would have been, which
    /// declares no member. So depth 0 has no N, and depth 1 is the
    /// declaration's own enclosing object and declaring class
    /// (`Heap::class_declaration_scope`).
    Declaration(DeclarationScope<'gc>),
}

/// §3.3: the enclosing object at lexical `depth` for an activation whose
/// walk starts at `origin` and running on `self_value`.
///
/// `depth` is *lexical*: the resolver fixed it against the chain of class
/// declarations open at the place the code was written, so it counts levels
/// out from the class that **defines** the running method -- which
/// `Frame.current_class` names -- and not from the class of whatever object
/// receives it. Those two are the same class only when the method is not
/// inherited across a nesting boundary; seeding the walk from the receiver
/// instead follows the receiver's own chain and lands on an object of an
/// unrelated class (or `nil`) for an inherited one.
///
/// So depth 0 is `self`, and depth k > 0 is reached by walking k levels of
/// the *declaration* chain out from the defining class, taking each level's
/// own enclosing object -- §3.3's "the object that received the message
/// that created the class object". `Heap::class_declaring` is that
/// chain; the receiver's class chain is not, for the same reason the seed
/// is not. A body-less declaration's own code takes its first step from
/// the declaration's scope instead (`LexicalOrigin::Declaration`).
///
/// A chain shorter than `depth` yields `nil` rather than the nearest
/// object it did reach, which would be dispatched against silently. Only
/// the outermost level ends a chain: a module root class, which no
/// declaration encloses, and a `None` defining class, a top-level
/// activation. Both have a lexical chain of exactly one level, so no
/// positive depth is resolvable in either.
pub fn kth_enclosing_object<'gc>(
    heap: &Heap<'gc>,
    self_value: Value<'gc>,
    origin: LexicalOrigin<'gc>,
    depth: u16,
) -> Value<'gc> {
    if depth == 0 {
        return self_value;
    }
    let (mut level, mut remaining) = match origin {
        LexicalOrigin::Class(Some(level)) => (level, depth),
        LexicalOrigin::Class(None) => return Value::nil(),
        LexicalOrigin::Declaration(scope) => {
            if depth == 1 {
                return scope.enclosing.map_or(Value::nil(), Value::object);
            }
            let Some(level) = scope.declaring_class else {
                return Value::nil();
            };
            (level, depth - 1)
        }
    };
    loop {
        let Some(enclosing) = heap.class_enclosing(level) else {
            return Value::nil();
        };
        remaining -= 1;
        if remaining == 0 {
            return Value::object(enclosing);
        }
        // One step out along the declaration chain -- the class object of
        // the level that lexically declares `level`.
        let Some(next) = heap.class_declaring(level) else {
            return Value::nil();
        };
        level = next;
    }
}

/// What a lookup does with a declaration of its selector found at one level,
/// decided by the declaration's accessibility.
enum Verdict {
    /// The declaration is the answer.
    Accept,
    /// The declaration is passed over; the search continues at the
    /// superclass.
    Skip,
    /// The search ends with no answer.
    Stop,
}

/// Which of the three lookups a walk is (`newspeak-spec.pdf` §5.5, §5.9,
/// §3.5) -- what it does with a declaration it finds at one level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// §5.5's lookupPublic: public accepted, protected stops, private skipped.
    Public,
    /// §5.9's lookupProtected: public and protected accepted, private skipped.
    Protected,
    /// §3.5's "defined for", `doesNotUnderstand:`'s: every declaration accepted.
    DefinedFor,
}

impl Rule {
    fn verdict(self, access: ast::AccessModifier) -> Verdict {
        match (self, access) {
            (Rule::DefinedFor, _)
            | (Rule::Public, ast::AccessModifier::Public)
            | (Rule::Protected, ast::AccessModifier::Public | ast::AccessModifier::Protected) => {
                Verdict::Accept
            }
            (Rule::Public, ast::AccessModifier::Protected) => Verdict::Stop,
            (Rule::Public | Rule::Protected, ast::AccessModifier::Private) => Verdict::Skip,
        }
    }
}

/// The class declaration at lexical depth `depth` from `origin` -- §5.9's
/// `N`, "a class whose declaration D lexically encloses the outer send", at
/// the depth the resolver fixed. Depth 0 is the defining class itself, the
/// class that declares the running method; each further level is one step
/// out along the declaration chain, the chain `kth_enclosing_object` walks.
/// `None` where the chain is shorter than `depth`, for a top-level
/// activation (no defining class), and at depth 0 for a body-less
/// declaration's own code: the declaration declares no member, so there is
/// no private one for §5.9 to find, where its class object's mixin, the
/// named class's, would offer that class's.
pub fn kth_lexical_class<'gc>(
    heap: &Heap<'gc>,
    origin: LexicalOrigin<'gc>,
    depth: u16,
) -> Option<ClassReference<'gc>> {
    let (mut level, steps) = match origin {
        LexicalOrigin::Class(defining_class) => (defining_class?, depth),
        LexicalOrigin::Declaration(scope) => (scope.declaring_class?, depth.checked_sub(1)?),
    };
    for _ in 0..steps {
        level = heap.class_declaring(level)?;
    }
    Some(level)
}

/// §5.9: "If N declares a private method f named s then the value of the
/// send is the result of invoking f". Asks `lexical_class`'s *own* mixin
/// only -- a private member of a subclass or a superclass of N is not N's
/// -- and answers `NotFound` unless that mixin declares `selector` private.
/// A private nested class is a private member on the same footing (§6: its
/// accessor has "the same name and accessibility"). Primordial Soup's
/// `LexicalSend` asks the same one table (`MethodAt` on N's application).
pub fn private_member<'gc>(
    heap: &Heap<'gc>,
    lexical_class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    let Some(application) = heap.class_application(lexical_class) else {
        return Lookup::NotFound;
    };
    if let Some(entry) = application.mixin.methods.get(&selector) {
        return if entry.access() == ast::AccessModifier::Private {
            found_entry(entry, &application, lexical_class)
        } else {
            Lookup::NotFound
        };
    }
    match nested_declaration(heap, &application, selector) {
        Some(nested) if nested_access(&nested) == ast::AccessModifier::Private => {
            Lookup::NestedClass(nested, lexical_class)
        }
        _ => Lookup::NotFound,
    }
}

/// §5.9's lookup for a lexical send `outer N s` to `o`: N's own private
/// member first (`private_member`; `lexical_class` is N, `None` when the
/// chain is shorter than the send's depth), then lookupProtected on
/// `enclosing_class`, the class of `o`. `NotFound` means the send is an
/// ordinary one to `o`. The one statement of the order for a reader that
/// only needs what the lookup finds. `Mutator::resolve_lexical` takes the
/// same two steps inline, because it also tells a private find from a
/// protected one, and is documented and tested against this function; the
/// two change together.
pub fn lexical_member<'gc>(
    heap: &Heap<'gc>,
    lexical_class: Option<ClassReference<'gc>>,
    enclosing_class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    if let Some(lexical_class) = lexical_class {
        match private_member(heap, lexical_class, selector) {
            Lookup::NotFound => {}
            found => return found,
        }
    }
    lookup_protected(heap, enclosing_class, selector)
}

/// §5.5, ordinary (explicit-receiver) sends: lookupPublic(n, C) is "m, if the
/// mixin of C defines a method m with selector n and public access.
/// Undefined, if the mixin of C defines a method m with selector n and
/// protected access. lookupPublic(n, superclass(C)) otherwise." So a
/// `protected` declaration shadows an inherited `public` one and ends the
/// search, while a `private` one falls to the "otherwise" and is passed over
/// -- Primordial Soup's `OrdinarySendMiss` does the same.
pub fn lookup_public<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    walk(heap, class, selector, Rule::Public)
}

/// §5.9's lookupProtected(n, C): "m, if the mixin of C defines a method m
/// with selector n and either public or protected access", and
/// lookupProtected(n, superclass(C)) otherwise -- a `private` declaration is
/// passed over. The lookup of a lexical send (§5.9) and of a super send
/// (§5.10).
pub fn lookup_protected<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    walk(heap, class, selector, Rule::Protected)
}

/// The plain, access-blind "defined for" walk (§3.5): used by
/// `doesNotUnderstand:` resolution, which is exempt from every access check
/// (§5.5: "the #doesNotUnderstand: method defined for (3.5) the class of o").
pub fn lookup_defined_for<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    walk(heap, class, selector, Rule::DefinedFor)
}

/// Walks `class`'s superclass chain for a declaration of `selector`, asking
/// `rule` what to do with each one it finds.
///
/// `#[inline(always)]`, and called with a literal `Rule` by each of the three
/// lookups above, so that each of them is its own specialized walk with
/// `Rule::verdict` folded into a constant test -- the shape the three
/// per-lookup verdict closures gave before the rule became a value. One
/// shared walk taking the rule at run time measured 1.7-2.0% slower on both
/// screening sources.
#[inline(always)]
fn walk<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
    rule: Rule,
) -> Lookup<'gc> {
    let mut level = level_of(heap, class);
    while let Some((level_class, application)) = level {
        // `level_of` established `level_class` names a class only for the first
        // iteration; every iteration after that reaches `level_class` through
        // `next_level`'s unverified class-object chain (see its doc), so a
        // hierarchy where that chain diverges from the application chain must
        // end the search here rather than read stale `data` off a `level_class`
        // that no longer names a class.
        let Some(superclass) = heap.class_lookup_step(level_class) else {
            return Lookup::NotFound;
        };
        // The step to the next level is taken where the search continues,
        // not once per level up front: `next_level` clones the superclass's
        // application `Rc`, and a lookup that answers at the level it is
        // already on -- a send to a slot accessor, a method the receiver's
        // own class declares, a builtin's own primitive, which is most of
        // them -- would otherwise pay that reference count for a level it
        // never reads. Reading the superclass handle here rather than in
        // `next_level` keeps the walk's loop state two words: measured 2.5%
        // (`bench_loop.ns`) and 1.6% (`bench_send.ns`) faster than building
        // it unconditionally at the top of the loop, on both screening
        // sources.
        if let Some(entry) = application.mixin.methods.get(&selector) {
            match rule.verdict(entry.access()) {
                Verdict::Accept => return found_entry(entry, &application, level_class),
                Verdict::Stop => return Lookup::NotFound,
                Verdict::Skip => {
                    level = next_level(superclass, &application);
                    continue;
                }
            }
        }
        // §6: a nested declaration is a member of its enclosing class on
        // exactly the same footing as a method, so it is searched at the
        // same level and under the same access rules -- after the method
        // dictionary, because a method is the only thing that can shadow
        // an accessor the compiler generated implicitly. A linear scan of
        // the nested declarations costs nothing on the hot path: it runs
        // only once the method dictionary has already missed, and
        // `nested_declaration` declines at once for every builtin and for
        // most user classes, whose chains declare none.
        if let Some(nested) = nested_declaration(heap, &application, selector) {
            match rule.verdict(nested_access(&nested)) {
                Verdict::Accept => return Lookup::NestedClass(nested, level_class),
                Verdict::Stop => return Lookup::NotFound,
                Verdict::Skip => {
                    level = next_level(superclass, &application);
                    continue;
                }
            }
        }
        level = next_level(superclass, &application);
    }
    Lookup::NotFound
}

/// The current version of the declaration named `selector` nested
/// directly in `application`'s mixin, read through the mixin's id, so that
/// an edit of the nested declaration is what the accessor reaches. An
/// application whose chain caches no nested class declares none, so it
/// declines without asking the heap for the id.
fn nested_declaration(
    heap: &Heap<'_>,
    application: &MixinApplication,
    selector: Symbol,
) -> Option<Rc<Mixin>> {
    if application.nested_slot.is_empty() {
        return None;
    }
    let id = heap.known_mixin_id(&application.mixin)?;
    heap.nested_of(id)
        .iter()
        .filter_map(|nested_id| heap.mixin(*nested_id))
        .find(|nested| nested.name == selector)
}

/// The answer a method-table entry found at `level_class`'s level gives: a
/// compiled method with that level as its defining class, a slot accessor
/// with its field resolved against that level's application -- the level
/// whose mixin declares the slot, so `own_slot_base` places it (§6.3.2, as
/// `InitializeSlot` places a slot's initial value) -- a primitive with
/// that level, which an inline cache counts from, or the primary factory with
/// that level, which the resolution checks is the receiver's own metaclass.
fn found_entry<'gc>(
    entry: &MethodEntry,
    application: &MixinApplication,
    level_class: ClassReference<'gc>,
) -> Lookup<'gc> {
    match entry {
        MethodEntry::Compiled(method) => Lookup::Method(method.clone(), level_class),
        MethodEntry::SlotAccessor(accessor) => Lookup::SlotAccessor(
            *accessor,
            application.own_slot_base + accessor.own_index as usize,
            application.index,
        ),
        MethodEntry::Primitive(method) => Lookup::Primitive(method.primitive, level_class),
        MethodEntry::Factory(factory) => Lookup::Factory(level_class, factory.code.clone()),
    }
}

/// One level of the two chains a lookup walks: the application supplies the
/// code, the class object supplies the identity `Frame.current_class` needs
/// for `super` and `outer`. Returns `None` for a non-class object, which is
/// what ends a walk that was handed one.
fn level_of<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
) -> Option<(ClassReference<'gc>, Rc<MixinApplication>)> {
    heap.class_application(class)
        .map(|application| (class, application))
}

/// Steps both chains together. For an ordinary class object, every
/// construction site (`load_module`, `build_builtin_classes`) links its
/// superclass exactly when it links its application's superclass, so the
/// two chains have the same length; requiring both halves here means a
/// hierarchy that somehow lost one of them ends the search instead of
/// reporting a `Lookup::Method` whose class object comes from a different
/// level than its code.
///
/// A metaclass keeps the invariant too (`Mutator::attach_metaclass`): its
/// class-object `superclass` is `Class` and its application is applied over
/// `Class`'s (`newspeak-spec.pdf` §6), so a lookup that misses on a
/// metaclass continues onto `Class`, `Behavior` and `Object` -- never onto
/// an ancestor's class side, which §3.2 keeps flat.
fn next_level<'gc>(
    superclass: Option<ClassReference<'gc>>,
    application: &Rc<MixinApplication>,
) -> Option<(ClassReference<'gc>, Rc<MixinApplication>)> {
    let superclass = superclass?;
    let super_application = application.superclass.clone()?;
    Some((superclass, super_application))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::primitive::mirror_primitive_named;
    use crate::mixin::MethodFastPath;
    use indoc::formatdoc;
    use std::collections::HashMap;

    /// A placeholder `ClassReference` whose own class is itself. Every
    /// object has a class, so constructing *any* object requires already
    /// having a `ClassReference` in hand for it -- and these test classes
    /// are never linked into a real bootstrapped class hierarchy, so there
    /// is no other object to point at. None of these tests (nor
    /// `walk`/`lookup_*`) ever read a class object's own class -- `walk`
    /// only reads its application and superclass -- so any non-panicking
    /// placeholder is sufficient; a self-referential class with an empty
    /// application is the simplest to build. The construction itself lives
    /// in `Heap::allocate_self_referential_class`.
    fn dummy_class<'gc>(heap: &mut Heap<'gc>) -> ClassReference<'gc> {
        let mixin = Rc::new(crate::mixin::Mixin::empty(Symbol::intern("Dummy")));
        let level_facts = crate::mixin::LevelFacts::of(&mixin);
        let application = heap.register_application(MixinApplication {
            index: crate::heap::ApplicationIndex::UNREGISTERED,
            mixin,
            superclass: None,
            inherits_value: false,
            first_mutable_slot: None,
            instantiation_refusal: None,
            nested_slot: crate::hash::IdentityMap::default(),
            declared_slots: Vec::new(),
            slot_count: 0,
            own_slot_base: 0,
            dispatch_ids: crate::mixin::DispatchIds::NONE,
            instance_header: None,
            level_facts,
        });
        heap.allocate_self_referential_class(crate::heap::NewClass {
            application,
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin: None,
        })
    }

    fn class_with_method<'gc>(
        heap: &mut Heap<'gc>,
        name: &str,
        selector: &str,
        access: Option<ast::AccessModifier>,
        superclass: Option<ClassReference<'gc>>,
    ) -> ClassReference<'gc> {
        // Build a minimal MethodDeclaration via ns_syntax::parse of a throwaway
        // module and pull the method out of it -- avoids hand-constructing
        // deeply nested AST literals here.
        let access_keyword = access
            .map(|modifier| format!("{modifier:?} ").to_lowercase())
            .unwrap_or_default();
        let source = formatdoc! {"
            class X = Object new (
            ) (
              {access_keyword}{selector} = ( ^1 )
            ) : ()
        "};
        let module = ns_syntax::parse(&source).expect("parse");
        let method = module
            .root_class
            .instance_side
            .methods()
            .next()
            .expect("a method")
            .clone();
        let mut methods = crate::hash::IdentityMap::default();
        methods.insert(
            Symbol::intern(selector.trim_end_matches(':')),
            MethodEntry::Compiled(Rc::new(MixinMethod::from_declaration(
                Rc::new(method),
                &Rc::new(HashMap::new()),
                false,
                crate::compiler::Lowering::Optimized,
            ))),
        );
        // The application chain must be linked wherever the class-object
        // chain is: `next_level` steps both, so a superclass linked on only
        // one of them would end the walk one level early and these tests
        // would pass for the wrong reason.
        let super_application = superclass
            .as_ref()
            .and_then(|superclass_class| heap.class_application(*superclass_class));
        let mixin = Rc::new(crate::mixin::Mixin {
            methods,
            ..crate::mixin::Mixin::empty(Symbol::intern(name))
        });
        let level_facts = crate::mixin::LevelFacts::of(&mixin);
        let application = heap.register_application(MixinApplication {
            index: crate::heap::ApplicationIndex::UNREGISTERED,
            mixin,
            superclass: super_application,
            inherits_value: false,
            first_mutable_slot: None,
            instantiation_refusal: None,
            nested_slot: crate::hash::IdentityMap::default(),
            declared_slots: Vec::new(),
            slot_count: 0,
            own_slot_base: 0,
            dispatch_ids: crate::mixin::DispatchIds::NONE,
            instance_header: None,
            level_facts,
        });
        let class_of_class = dummy_class(heap);
        heap.allocate_class(
            class_of_class,
            crate::heap::NewClass {
                application,
                enclosing: None,
                declaring_class: None,
                superclass,
                builtin: None,
            },
        )
    }

    #[test]
    fn lookup_public_finds_a_public_method() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = class_with_method(
                &mut mutator.heap,
                "X",
                "foo",
                Some(ast::AccessModifier::Public),
                None,
            );
            assert!(matches!(
                lookup_public(&mutator.heap, class, Symbol::intern("foo")),
                Lookup::Method(..)
            ));
        });
    }

    #[test]
    fn lookup_public_hard_stops_on_a_protected_declaration() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let base = class_with_method(
                &mut mutator.heap,
                "Base",
                "foo",
                Some(ast::AccessModifier::Public),
                None,
            );
            let derived = class_with_method(
                &mut mutator.heap,
                "Derived",
                "foo",
                Some(ast::AccessModifier::Protected),
                Some(base),
            );
            assert!(matches!(
                lookup_public(&mutator.heap, derived, Symbol::intern("foo")),
                Lookup::NotFound
            ));
        });
    }

    #[test]
    fn lookup_protected_skips_a_private_declaration_transparently() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let base = class_with_method(
                &mut mutator.heap,
                "Base",
                "foo",
                Some(ast::AccessModifier::Public),
                None,
            );
            let derived = class_with_method(
                &mut mutator.heap,
                "Derived",
                "foo",
                Some(ast::AccessModifier::Private),
                Some(base),
            );
            // lookup_protected on Derived's own `foo` (private) must not match,
            // but must continue to Base's public `foo`.
            assert!(matches!(
                lookup_protected(&mutator.heap, derived, Symbol::intern("foo")),
                Lookup::Method(..)
            ));
        });
    }

    #[test]
    fn default_access_is_protected() {
        assert_eq!(effective_access(None), ast::AccessModifier::Protected);
    }

    /// A builtin's method is an entry of its own method table:
    /// `Integer`'s `+` is a kernel primitive method (`Kernel.ns`), found as
    /// a `Lookup::Method` whose `MixinMethod.fast_path` names the mirror
    /// row.
    #[test]
    fn a_builtins_own_primitive_is_found_at_its_own_level() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            match lookup_public(&mutator.heap, integer, Symbol::intern("+")) {
                Lookup::Method(method, level) => {
                    assert_eq!(
                        method.fast_path,
                        mirror_primitive_named("integer:add:ifFail:").map(MethodFastPath::Mirror)
                    );
                    assert!(level.same(integer), "found above Integer's own level")
                }
                _ => panic!("`3 + 4` must find Integer's own `+`"),
            }
        });
    }

    /// `Closure` has no `==`; the ordinary walk reaches `Object`'s, since
    /// `Closure`'s superclass is `Object` -- no hand-restored inheritance.
    /// `Object`'s `==` is a kernel primitive method (`Kernel.ns`), found as a
    /// `Lookup::Method` whose `MixinMethod.fast_path` names the mirror row.
    #[test]
    fn a_primitive_a_builtin_lacks_is_found_at_objects_level() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let block = mutator.builtin_class(BuiltinClass::Closure);
            let object = mutator.builtin_class(BuiltinClass::Object);
            match lookup_public(&mutator.heap, block, Symbol::intern("==")) {
                Lookup::Method(method, level) => {
                    assert_eq!(
                        method.fast_path,
                        mirror_primitive_named("object:identicalTo:ifFail:")
                            .map(MethodFastPath::Mirror)
                    );
                    assert!(level.same(object))
                }
                _ => panic!("a Closure's `==` must find Object's `==`"),
            }
        });
    }

    /// §3.1.1: a value has no observable identity, so `Value` declares the
    /// identity method as a send of `=` -- an ordinary method, carrying no
    /// primitive -- and a value receiver finds it below `Object`'s identity
    /// `==`, which it must never reach.
    #[test]
    fn a_value_finds_values_identity_method_before_objects() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            let value = mutator.builtin_class(BuiltinClass::Value);
            match lookup_public(&mutator.heap, integer, Symbol::intern("==")) {
                Lookup::Method(method, level) => {
                    assert_eq!(method.fast_path, None);
                    assert!(level.same(value))
                }
                _ => panic!("`3 == 3` must find Value's `==`"),
            }
        });
    }

    /// `String`'s `=` is a kernel primitive method (`Kernel.ns`): an
    /// ordinary lookup finds it at `String`'s own level, carrying the mirror
    /// row its forwarder sends. Its failure path is its own body's `super =`,
    /// so nothing passes a level over at run time.
    #[test]
    fn strings_equality_is_a_kernel_primitive_method_at_its_own_level() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let string = mutator.builtin_class(BuiltinClass::String);
            match lookup_public(&mutator.heap, string, Symbol::intern("=")) {
                Lookup::Method(method, level) => {
                    assert_eq!(
                        method.fast_path,
                        mirror_primitive_named("string:equals:ifFail:").map(MethodFastPath::Mirror)
                    );
                    assert!(level.same(string), "found at String's own level")
                }
                _ => panic!("String's own `=` must be found at its own level"),
            }
        });
    }

    /// §5.5: `Object`'s `doesNotUnderstand:` is protected -- lookupPublic
    /// stops at it, lookupProtected and the "defined for" walk accept it. It
    /// is an ordinary kernel method (`Kernel.ns`), found as a
    /// `Lookup::Method` with no fast path: it signals a
    /// `MessageNotUnderstood`.
    #[test]
    fn objects_does_not_understand_is_protected() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            let selector = Symbol::DOES_NOT_UNDERSTAND;
            assert!(matches!(
                lookup_public(&mutator.heap, integer, selector),
                Lookup::NotFound
            ));
            let expected = None;
            match lookup_protected(&mutator.heap, integer, selector) {
                Lookup::Method(method, _) => assert_eq!(method.fast_path, expected),
                _ => panic!("lookupProtected must find Object's doesNotUnderstand:"),
            }
            match lookup_defined_for(&mutator.heap, integer, selector) {
                Lookup::Method(method, _) => assert_eq!(method.fast_path, expected),
                _ => panic!("the \"defined for\" walk must find Object's doesNotUnderstand:"),
            }
        });
    }
}
