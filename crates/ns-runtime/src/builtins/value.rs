//! The equality and hash a value object gets from its state, derived in
//! Rust so that `Value` can declare `=` and `hash` without any native ever
//! calling back into Newspeak.
//!
//! §3.1.1 makes a value object deeply immutable and lets it be copied
//! across an actor boundary, so a copy has to be equal to, and hash like,
//! its original: **a value has no observable identity**, and deciding
//! either from one is exactly the mistake these leaves exist to avoid. Two
//! values are equal when their classes are equal -- identical, or two
//! value class objects equal by their own state -- and their states are,
//! all the way down.
//!
//! **A class object is a node too.** One is a value exactly when its
//! enclosing object is (`instantiate::class_object_is_a_value`), and then
//! it has no identity either: its state is its mixin application, its
//! enclosing object and its superclass class object, and a metaclass's is
//! its class (`docs/design/object-model.md`, "Class sides and
//! metaclasses"). Two values of one nested declaration under two equal
//! enclosing values therefore have two distinct but equal classes, so the
//! class of every pair of nodes is itself a pair the walk compares, not an
//! identity test. A class object that is no value compares by identity.
//! Its hash is its name's (`hash_node`, which says why).
//!
//! Neither walk recurses. An image is untrusted input and the graph a
//! decoded value reaches is the image's to choose, so the equality walk
//! runs an explicit work stack and remembers the pairs it has already
//! assumed equal -- instances and class objects alike -- so a cycle,
//! through slots or through classes, terminates in `true` rather than
//! running forever; the hash walk runs a queue, the hash being defined
//! over the first `NODE_BUDGET` nodes of a value's breadth-first unfolding
//! (`derived_hash`). Neither runs for as long as the input likes, and
//! neither's work list grows past the budget.
//!
//! What a leaf cannot decide it declines, and the `ifFail:` block of
//! `Value`'s method answers instead. It never asks the receiver: a native
//! that sent `=` or `hash` back into Newspeak would be a step that runs the
//! loop, which nothing in `builtins/` may be (`docs/design/execution.md`,
//! "No re-entry from Rust"), and would stand in the way of compiling these
//! methods natively later.
//!
//! Every per-shape rule here is the one the shape's own class uses:
//! `numbers::tower`'s and `text`'s named helpers, which their `=` and
//! `hash` primitives call too. A second copy of those rules is how the derived
//! answer and the class's own would drift apart.
//!
//! **Each walk is a second implementation of the method it stands for**:
//! the equality walk of `valueEquals:` (and, for a class object, of
//! `behaviorEquals:`), the hash walk of `valueHashOfSlots`, each sending
//! the slots' own `=` and `hash`. Wherever a walk answers or refuses
//! without sending, it answers what that method would, or refuses where
//! the method would read a state not built yet; where it cannot know, it
//! declines and the method answers. The hash's node budget is no
//! exception: `valueHashOfSlots` visits the same first nodes with a queue
//! of its own.
//! `the_leaves_answer_as_the_methods_they_stand_for_over_every_shape`
//! holds the walks to the methods over every shape, pair by pair.

use crate::interpreter::Arguments;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use crate::builtins::collections::{self, IntegerArgument};
use crate::builtins::first_argument;
use crate::builtins::numbers::tower;
use crate::builtins::primitive::{Primitive, ValuePrimitive};
use crate::builtins::text;
use crate::dispatch::{self, Lookup};
use crate::interpreter::instantiate::{
    ValueQuestion, class_object_is_a_value, under_construction_error,
};
use crate::types::*;
use crate::{Mutator, Symbol};

/// How many nodes the equality walk puts on its work stack before it stops
/// unfolding, and the hash walk visits -- the root included.
///
/// It is charged when a node is **pushed**, not when one is taken, which is
/// what bounds the work vector itself by this number rather than by what
/// the heap holds. Charging on the take instead let one node push all of
/// its slots first, so a wide or cyclic value pushed up to `NODE_BUDGET`
/// times the widest class's slot count before the budget stopped it: an
/// image is untrusted input and chooses that width, so it chose the
/// allocation, which is how a decoded value could turn a bounded walk into
/// an allocation abort.
///
/// For the hash it is part of the definition (`r[runtime.value.hash]`),
/// which `Value>>valueHashOfSlots` writes out as `4096`: two equal values
/// unfold breadth-first to the same sequence of nodes, so both stop at the
/// same node.
const NODE_BUDGET: usize = 4096;

/// `nil`'s hash: 0, what `valueHashOfSlots` answers for it -- its slot
/// count, which is none -- and so what the walk answers for every other
/// instance of `UndefinedObject`, each equal to `nil`, its state no slots
/// either.
const NIL_HASH: i64 = 0;

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ValuePrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        ValuePrimitive::SameAs => {
            let other = arguments.first(&mutator.heap)?;
            // A pair whose receiver is `nil`, `true` or `false` is decided
            // by identity before the walk, as the walk would decide it
            // (`constant_identity` says why the two agree), because it is
            // the commonest pair: every `nil == anObject` (`Value>>==`),
            // 366,000 a run of Richards.
            let equal = if let Some(constant) = constant_identity(mutator, receiver, other) {
                constant
            } else {
                // A refusal, never a decline: `valueEquals:` would compare
                // the slots built so far, which is the answer the refusal
                // withholds.
                match derived_equality(mutator, receiver, other) {
                    Ok(equal) => equal?,
                    Err(object) => {
                        return Some(Err(under_construction_error(
                            mutator,
                            ValueQuestion::Compare,
                            selector,
                            receiver,
                            object,
                        )));
                    }
                }
            };
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Value(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        // Refused as `=` is, for the same reason: `valueHashOfSlots` would
        // hash the slots built so far.
        ValuePrimitive::Hash => match derived_hash(mutator, receiver) {
            Ok(hash) => hash.map(|hash| Ok(Value::small_integer(hash))),
            Err(object) => Some(Err(under_construction_error(
                mutator,
                ValueQuestion::Hash,
                selector,
                receiver,
                object,
            ))),
        },
        // The readers of `valueEquals:` and `valueHashOfSlots` refuse a
        // subject under construction, as the leaves those methods stand in
        // for refuse it, in the words of the question their caller names:
        // so the leaf and the method refuse alike by construction, and
        // neither ever reads a state not built yet.
        ValuePrimitive::SlotCount => {
            if let Some(refusal) =
                refusal_of_unbuilt(mutator, receiver, arguments.first(&mutator.heap))
            {
                return Some(Err(refusal));
            }
            let count = with_declared_slots(mutator, receiver, <[usize]>::len)?;
            Some(Ok(Value::small_integer(count as i64)))
        }
        ValuePrimitive::HashUnfolds => Some(Ok(Value::boolean(matches!(
            hash_node(mutator, receiver),
            HashNode::Unfold(_)
        )))),
        ValuePrimitive::SlotAt => {
            if let Some(refusal) =
                refusal_of_unbuilt(mutator, receiver, arguments.try_get(&mutator.heap, 1))
            {
                return Some(Err(refusal));
            }
            // Its errors name the reader by the slot it reads, as
            // `IndexedCollection>>at:`'s do, not by the question it is asked.
            let selector = Symbol::intern("slotAt:");
            let object = receiver.as_object()?;
            // Resolved to a field index inside the borrow, never by
            // copying the list out: the declared slots and the fields
            // differ as soon as a level declares a nested class, and the
            // leaf must answer the same slot the walks compare.
            let index =
                collections::integer_argument(&mutator.heap, arguments.first(&mutator.heap));
            let (count, field) = with_declared_slots(mutator, receiver, |declared| {
                let field = match index {
                    // An index past `usize` is out of range, never
                    // wrapped to a slot that exists.
                    Some(IntegerArgument::Small(index)) => usize::try_from(index)
                        .ok()
                        .and_then(|index| index.checked_sub(1))
                        .and_then(|position| declared.get(position).copied()),
                    _ => None,
                };
                (declared.len(), field)
            })?;
            let Some(field) = field else {
                // An index outside the range is the program's mistake, not a
                // shape this leaf does not serve, so it is an ordinary
                // Newspeak error in the words `Array>>at:` already uses --
                // never a decline, which would answer the `ifFail:` block
                // and hide it. A `LargeInteger` index is outside it too.
                return Some(Err(match index {
                    Some(IntegerArgument::Small(index)) => collections::out_of_range(
                        receiver,
                        selector,
                        index,
                        first_argument(&mutator.heap, arguments),
                        count,
                    ),
                    Some(IntegerArgument::Large { .. }) => collections::out_of_range(
                        receiver,
                        selector,
                        mutator
                            .heap
                            .describe_argument(arguments.first(&mutator.heap)),
                        first_argument(&mutator.heap, arguments),
                        count,
                    ),
                    None => NewspeakError::argument_error(
                        selector,
                        receiver,
                        first_argument(&mutator.heap, arguments),
                        format!(
                            "{selector} requires an Integer index, got {}",
                            mutator
                                .heap
                                .describe_argument(arguments.first(&mutator.heap))
                        ),
                    ),
                }));
            };
            Some(Ok(mutator
                .heap
                .instance_field(object, field)
                .unwrap_or(Value::nil())))
        }
    }
}

/// The refusal a slot reader answers for `subject` under construction:
/// the one the equality leaf gives where `asking` is `#=`, the hash leaf's
/// for any other, each under that selector; `None` for a complete subject.
fn refusal_of_unbuilt<'gc>(
    mutator: &Mutator<'gc>,
    subject: Value<'gc>,
    asking: Option<Value<'gc>>,
) -> Option<NewspeakError<'gc>> {
    let object = under_construction(mutator, subject)?;
    let selector = asking
        .and_then(Value::as_object)
        .and_then(|symbol| mutator.heap().symbol_value(symbol))
        .unwrap_or_else(|| Symbol::intern("hash"));
    let question = if selector == Symbol::EQUALS {
        ValueQuestion::Compare
    } else {
        ValueQuestion::Hash
    };
    Some(under_construction_error(
        mutator, question, selector, subject, object,
    ))
}

/// Runs `read` over the field indices of the slots `receiver`'s classes
/// **declare**, when it is an ordinary instance -- the only shape whose
/// state is slots at all. `None` for anything else, which is the decline.
///
/// **The one place this question is answered**, for the same reason
/// `state_is_a_value` is one function: the equality walk, the hash walk and
/// the two leaves the Newspeak fallback reads a slot through
/// (`valueSlotCount:asking:ifFail:`, `value:slotAt:asking:ifFail:`) must agree on which
/// slots exist and on their order. If they did not, the fallback would
/// compare different slots than the leaf does, and `=` would answer one
/// thing when the leaf decided and another when it declined.
///
/// A hidden nested-class cache slot is not declared state and is not in the
/// list (`MixinApplication::declared_slots`, which says why). It is
/// computed once, when the application is built, so this hands the caller a
/// borrow of it rather than a copy: the declining path asks once per slot
/// read, and the old shape -- a chain walk and a `Vec` per ask -- made a
/// Newspeak-side slot walk allocate per index.
fn with_declared_slots<'gc, T>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    read: impl FnOnce(&[usize]) -> T,
) -> Option<T> {
    let object = receiver.as_object()?;
    if mutator.heap().is_class(object) {
        return None;
    }
    // The shape test stays what it was -- an ordinary instance has a field
    // count, a String, a Symbol, an Array and a closure do not -- so a
    // shape this leaf does not serve still declines rather than answering
    // an empty state.
    mutator.heap().instance_field_count(object)?;
    let class = mutator.heap().object_class(object);
    // `None` here would mean the receiver's class is not a class object,
    // which nothing an image can encode should produce -- and if it ever
    // did, declining is the safe answer, not a panic on untrusted input.
    mutator.heap().with_class_declared_slots(class, read)
}

/// What a walk found at one node of a value: a verdict the shape's own rule
/// decides outright, an instance whose state is its slots, or two class
/// objects whose state is their application, enclosing object and
/// superclass -- the last two have to be unfolded.
enum Pair<'gc> {
    Decided(bool),
    State(ObjectReference<'gc>, ObjectReference<'gc>),
    Classes(ClassReference<'gc>, ClassReference<'gc>),
}

/// What the hash walk does with one node: adds a hash it computes itself,
/// unfolds an instance whose state is its slots, or -- for a node whose
/// `hash` is its own -- leaves it to that `hash`, which the leaf cannot
/// send, so the leaf declines.
enum HashNode<'gc> {
    Leaf(i64),
    Unfold(ObjectReference<'gc>),
    Own,
}
/// Whether `object` is an ordinary instance whose `=` resolves to the very
/// method `Value` declares, which the bootstrap recorded
/// (`Mutator::value_equality_method`). A class that overrides `=` decides
/// its own equality, and a class that does not inherit `Value`'s -- an
/// `Array`, an ordinary `Object` subclass -- is no value at all. Both are a
/// decline rather than a slot-by-slot guess.
///
/// Compared by `Rc::ptr_eq`, never by selector: any class may declare `=`,
/// and only the method's identity says the lookup landed on `Value`'s.
///
/// **The other predicate about "is a value" is
/// `MixinApplication::inherits_value`**, computed in `Mutator::apply_mixin`
/// and read by `instantiate::check_value_slot`, and the two have to agree
/// on which objects are values: this one is the narrower question -- a
/// value *whose equality is still the derived one*, so a value class that
/// declares its own `=` is a value the flag accepts and this predicate
/// declines. What they may never disagree about is an instance whose `=`
/// is `Value`'s: `Value new` answered `true` here while the flag said
/// `false`, so `Value new = Value new` was `true` and a value's slot
/// refused to hold one.
///
/// **Both walks ask it**: the equality walk unfolds an instance only where
/// it holds, and the hash walk only where it holds and the instance's
/// `hash` is `Value`'s too (`hash_node`). The hash needs no more than that
/// to agree with `=`: it is defined by the breadth-first unfolding, which
/// the leaf and `valueHashOfSlots` follow alike, so two equal values hash
/// alike whichever path computes either.
fn state_is_a_value<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> bool {
    let Some(recorded) = mutator.value_equality_method.as_ref() else {
        return false;
    };
    if mutator.heap().is_class(object) || mutator.heap().instance_field_count(object).is_none() {
        return false;
    }
    let class = mutator.heap().object_class(object);
    match dispatch::lookup_public(mutator.heap(), class, Symbol::EQUALS) {
        Lookup::Method(found, _) => Rc::ptr_eq(&found, recorded),
        _ => false,
    }
}

/// How a walk stands to the `=` of a node below its root, which the
/// fallback compares by sending it `=` (`valueEquals:`) and hashes by
/// sending it `hash` (`valueHashOfSlots`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiverEquality {
    /// A `=` the walk computes bit for bit by a rule of its own: an
    /// immediate's, a `String`'s or `Symbol`'s, a boxed `Float`'s, a
    /// `LargeInteger`'s or a `Fraction`'s, each of exactly the kernel's
    /// class, and a class object's whose `=` is `Behavior`'s or
    /// `Metaclass`'s. The rules are `numbers::tower`'s and `text`'s, the
    /// ones those classes' own `=` leaves call.
    Walked,
    /// `Value`'s own `=`, the derived equality, whose state the walk
    /// unfolds (`state_is_a_value`).
    Derived,
    /// Any other `=` -- a value class's own, an ordinary object's identity
    /// -- which only a send can answer, so the walk declines.
    Own,
}

/// What the walks know about `value`'s `=`, asked of every node below the
/// root before anything is answered by its shape.
///
/// An immediate costs nothing; an object its class word, compared with
/// the kernel's classes, and for an instance the lookup of `=` that
/// `compare_node` and `hash_node` would make anyway. Only a class object
/// needs more: the lookup of its own `=` and of `Behavior`'s and
/// `Metaclass`'s, since a program may declare `=` on a class side.
fn receiver_equality<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> ReceiverEquality {
    let Some(object) = value.as_object() else {
        return ReceiverEquality::Walked;
    };
    let heap = mutator.heap();
    if heap.is_class(object) {
        let Lookup::Method(found, _) =
            dispatch::lookup_public(heap, heap.object_class(object), Symbol::EQUALS)
        else {
            return ReceiverEquality::Own;
        };
        let behaviors = [BuiltinClass::Class, BuiltinClass::Metaclass]
            .into_iter()
            .any(|kind| {
                matches!(
                    dispatch::lookup_public(heap, mutator.builtin_class(kind), Symbol::EQUALS),
                    Lookup::Method(method, _) if Rc::ptr_eq(&method, &found)
                )
            });
        return if behaviors {
            ReceiverEquality::Walked
        } else {
            ReceiverEquality::Own
        };
    }
    let class = heap.object_class(object);
    // Exactly the kernel's classes: a program's subclass may declare an
    // `=` of its own over the same shape -- a `Fraction` subclass holds a
    // numerator and a denominator -- and is asked like any other class.
    let walked = [
        BuiltinClass::String,
        BuiltinClass::Symbol,
        BuiltinClass::Float,
        BuiltinClass::LargeInteger,
        BuiltinClass::Fraction,
    ]
    .into_iter()
    .any(|kind| class.same(mutator.builtin_class(kind)));
    if walked {
        ReceiverEquality::Walked
    } else if state_is_a_value(mutator, object) {
        ReceiverEquality::Derived
    } else {
        ReceiverEquality::Own
    }
}

/// The **declared** slots of `object`, in declaration order, copied out so
/// the walk holds no borrow of the heap while it goes on. The *indices*
/// are never copied out: they are read through `with_declared_slots`, which
/// is also what the two leaves answer from, so a walk and the Newspeak
/// fallback see the same state.
fn slots_of<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> Vec<Value<'gc>> {
    with_declared_slots(mutator, Value::object(object), |declared| {
        declared
            .iter()
            .map(|index| {
                mutator
                    .heap()
                    .instance_field(object, *index)
                    .unwrap_or(Value::nil())
            })
            .collect()
    })
    .unwrap_or_default()
}

/// One node of the equality walk: `None` declines the whole comparison.
///
/// The two nodes' classes are `derived_equality`'s to compare, before this
/// runs: they are identical, or two class objects of one application that
/// the walk goes on to compare as a pair of their own.
///
/// `receiver` is what `receiver_equality` already found for `first`, if
/// the walk asked, so that the lookup of its `=` is made once a pair.
fn compare_node<'gc>(
    mutator: &Mutator<'gc>,
    first: Value<'gc>,
    second: Value<'gc>,
    receiver: Option<ReceiverEquality>,
) -> Option<Pair<'gc>> {
    // A number equals only a number of its value, whatever its class --
    // decided whichever side is the number, so that a node with no number
    // in it (`Fraction new`) is unequal to a number in both directions.
    match (
        tower::operand(mutator.heap(), first),
        tower::operand(mutator.heap(), second),
    ) {
        (None, None) => {}
        (Some(left), Some(right)) => return Some(Pair::Decided(tower::equal(&left, &right))),
        _ => return Some(Pair::Decided(false)),
    }
    match (first.kind(), second.kind()) {
        (ValueKind::Nil, ValueKind::Nil) => Some(Pair::Decided(true)),
        (ValueKind::Boolean(left), ValueKind::Boolean(right)) => Some(Pair::Decided(left == right)),
        (ValueKind::Object(left), ValueKind::Object(right)) => {
            if mutator.heap().is_class(left) && mutator.heap().is_class(right) {
                return Some(Pair::Classes(left, right));
            }
            if mutator.heap().is_symbol(left) {
                return Some(Pair::Decided(text::symbol_equals(left, Some(second))));
            }
            if mutator.heap().is_string(left) {
                return Some(Pair::Decided(text::text_equals(
                    mutator.heap(),
                    left,
                    Some(second),
                )));
            }
            let derived = receiver.map_or_else(
                || state_is_a_value(mutator, left),
                |receiver| receiver == ReceiverEquality::Derived,
            );
            derived.then_some(Pair::State(left, right))
        }
        // An immediate and a heap instance of its class -- `nil` and
        // `UndefinedObject new`, `true` and `True new`: `valueEquals:`
        // compares their declared slots, of which the immediate has none,
        // so the two are equal exactly when the instance has none either.
        (ValueKind::Object(object), _) | (_, ValueKind::Object(object)) => Some(Pair::Decided(
            with_declared_slots(mutator, Value::object(object), <[usize]>::len) == Some(0),
        )),
        // Two immediates of one class but of different kinds: none exist,
        // and "unequal" is the reading that claims no equality unchecked.
        _ => Some(Pair::Decided(false)),
    }
}

/// What the hash walk does with `value` below the root, by the method its
/// `hash` resolves to (`r[runtime.value.hash]`): a hash it computes as
/// that method does -- `nil`'s, a Boolean's, a number's of exactly the
/// kernel's class, a String's or a Symbol's, a value class object's whose
/// `hash` is `Behavior`'s -- an unfolding where both its `hash` and its
/// `=` are `Value`'s, and anything else left to its own `hash`.
/// `Value>>valueHashOfSlots` asks the same question of every node through
/// `valueHashUnfolds:ifFail:`, so the two unfold exactly the same nodes.
fn hash_node<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> HashNode<'gc> {
    let heap = mutator.heap();
    let leaf = |hash: Option<i64>| hash.map_or(HashNode::Own, HashNode::Leaf);
    match value.kind() {
        // `Value>>hash` of `nil`: no slots.
        ValueKind::Nil => HashNode::Leaf(NIL_HASH),
        // `True>>hash` and `False>>hash`.
        ValueKind::Boolean(flag) => HashNode::Leaf(i64::from(flag)),
        // `Integer>>hash` and `Float>>hash`, the tower's.
        ValueKind::SmallInteger(_) | ValueKind::SmallFloat(_) => {
            leaf(tower::value_hash(heap, value))
        }
        ValueKind::Object(object) => {
            if heap.is_class(object) {
                // `Behavior>>hash`: a value class object hashes as its
                // name -- two equal ones share an application, hence a
                // mixin, hence a name; the name, not a dispatch id, because
                // a name survives a copy and an image round trip -- and any
                // other its identity hash, which a leaf does not draw.
                let behaviors = |class_hash: &Rc<crate::mixin::MixinMethod>| {
                    matches!(
                        dispatch::lookup_public(
                            heap,
                            mutator.builtin_class(BuiltinClass::Class),
                            Symbol::intern("hash"),
                        ),
                        Lookup::Method(method, _) if Rc::ptr_eq(&method, class_hash)
                    )
                };
                return match dispatch::lookup_public(
                    heap,
                    heap.object_class(object),
                    Symbol::intern("hash"),
                ) {
                    Lookup::Method(found, _)
                        if behaviors(&found) && class_object_is_a_value(mutator, object) =>
                    {
                        HashNode::Leaf(text::string_hash(&heap.class_display_name(object)))
                    }
                    _ => HashNode::Own,
                };
            }
            let class = heap.object_class(object);
            let kernel = |kind| class.same(mutator.builtin_class(kind));
            // Exactly the kernel's classes, whose own `hash` is the rule
            // named here; a program's subclass is asked like any class.
            if kernel(BuiltinClass::Float)
                || kernel(BuiltinClass::LargeInteger)
                || kernel(BuiltinClass::Fraction)
            {
                return leaf(tower::value_hash(heap, value));
            }
            if kernel(BuiltinClass::Symbol) {
                return leaf(text::symbol_hash(heap, object));
            }
            if kernel(BuiltinClass::String) {
                return leaf(heap.string_hash(object));
            }
            let values_hash = mutator.value_hash_method.as_ref().is_some_and(|recorded| {
                matches!(
                    dispatch::lookup_public(heap, class, Symbol::intern("hash")),
                    Lookup::Method(found, _) if Rc::ptr_eq(&found, recorded)
                )
            });
            if values_hash && state_is_a_value(mutator, object) {
                HashNode::Unfold(object)
            } else {
                HashNode::Own
            }
        }
    }
}
/// Whether `receiver` equals `other`, when `receiver` is `nil`, `true` or
/// `false`, exactly as `derived_equality` answers it; `None` for every
/// other receiver, and for an argument the walk has to see.
///
/// Each of the three is an immediate, never under construction, of a
/// kernel class no other class can be equal to, so the walk's class test
/// finds every object of another class unequal, under construction or not,
/// and another immediate is equal exactly when it is the same. What is left
/// is a heap instance of the constant's own class -- `UndefinedObject new`,
/// `True new` -- which the walk compares by its state, as `valueEquals:`
/// does, and refuses while it is under construction: such an argument is
/// left to the walk. It is told by its dispatch id, the application's, read
/// from the header for a heap argument and from a table for an immediate
/// one -- the inline caches' own key, and cheaper than the class word, which
/// cost Richards 3% (`docs/design/core-library.md`). An argument of another
/// application can never be equal to the constant, and one of the constant's
/// application that is not of its class -- a body-less declaration of its
/// mixin -- the walk answers too.
fn constant_identity<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    other: Value<'gc>,
) -> Option<bool> {
    let identical = if receiver.is_nil() {
        other.is_nil()
    } else {
        other.as_boolean() == Some(receiver.as_boolean()?)
    };
    // An identical argument is an immediate too, so only an unequal one
    // can be an object.
    if !identical
        && other.as_object().is_some_and(|object| {
            mutator.heap().object_dispatch_id(object) == mutator.heap().dispatch_id(receiver)
        })
    {
        return None;
    }
    Some(identical)
}

/// `value` when it is an instance of a value class whose factory has not
/// returned (`Heap::object_under_construction`); `None` for every other
/// value, an immediate included.
fn under_construction<'gc>(
    mutator: &Mutator<'gc>,
    value: Value<'gc>,
) -> Option<ObjectReference<'gc>> {
    value
        .as_object()
        .filter(|object| mutator.heap().object_under_construction(*object))
}

/// Whether `left` and `right` are equal as values; `None` is the decline,
/// and `Err` the instance under construction the walk reached on a pair's
/// receiver side, or on its argument side where the classes left the pair
/// open, before it met an unequal pair: the walk answers whichever of the
/// two it meets first. Below the root, only a pair the walk decides itself
/// refuses; one it leaves to its receiver's own `=` declines instead.
///
/// A pair already on `assumed` is taken to be equal: that is what makes a
/// cyclic value answer `true` instead of unfolding forever, and it is
/// sound because a single `false` anywhere ends the walk -- an assumption
/// survives only when nothing contradicts it.
fn derived_equality<'gc>(
    mutator: &Mutator<'gc>,
    left: Value<'gc>,
    right: Value<'gc>,
) -> Result<Option<bool>, ObjectReference<'gc>> {
    // Two class objects are the operands of `Behavior>>=` and `==`, and so
    // of every `self class == other class`, which was `Object`'s identity
    // leaf before class objects became values: answered here without
    // building the walk wherever the walk's first step would decide. Only
    // for class objects.
    if let (Some(first), Some(second)) = (left.as_object(), right.as_object())
        && mutator.heap().is_class(first)
        && mutator.heap().is_class(second)
    {
        if first.same(second) {
            return Ok(Some(true));
        }
        if !classes_can_be_equal(mutator, first, second) {
            return Ok(Some(false));
        }
    }
    // The root pair is examined before the stack is used, so a pair its
    // first step decides -- two numbers, two values of unrelated classes,
    // a slot's `nil = anObject` -- allocates nothing: an empty `Vec` allocates
    // only at its first push, and this leaf runs for every `=` that
    // reaches it: allocating the stack up front cost Richards 15 % of its
    // time on Windows' heap. The set of assumed pairs is made at its first
    // insert for the same reason: a `HashSet::new()` reads its thread's
    // random keys, a thread-local access and a 16-byte copy on every `=`,
    // which a pair its first step decides never needs.
    let mut root = Some((left, right));
    let mut examining_root = true;
    let mut work = Vec::new();
    let mut assumed: Option<HashSet<(usize, usize)>> = None;
    // The root pair is charged like a pushed one.
    let mut charged = 1usize;
    while let Some((first, second)) = root.take().or_else(|| work.pop()) {
        // Only the first pair taken is the root.
        let at_root = std::mem::take(&mut examining_root);
        // Below the root, a pair is one `valueEquals:` compares by sending
        // `=` to the receiver's side, so the walk answers it only where it
        // computes that `=` itself -- before the tower and the classes,
        // which answer by shape and would otherwise speak for a class whose
        // own `=` might answer differently: `(Box on: aTag) = (Box on: 3)`
        // is whatever `Tag>>=` says. The root's receiver has reached
        // `Value>>=`, whose leaf this is.
        let receiver = (!at_root).then(|| receiver_equality(mutator, first));
        if receiver == Some(ReceiverEquality::Own) {
            return Ok(None);
        }
        // An instance whose factory has not returned has no state yet
        // (§3.1.1, "have been initialized"), and `Value`'s `=` answers no
        // question whose answer depends on it -- decided by the user on
        // 2026-11-28. On the receiver's side of a pair, the side `=` is
        // sent to, it answers nothing at all, before anything else is
        // compared: the walk has just made sure the pair's `=` is one it
        // computes, and so one that refuses its receiver as `valueEquals:`'s
        // send would. The argument's side is tested below, once the classes
        // have had their say.
        if let Some(object) = under_construction(mutator, first) {
            return Err(object);
        }
        // The classes first, for every shape: §3.1.1's equality is equality
        // of state *within* a class, so two values whose classes are unequal
        // are unequal whatever their states look like. Two distinct classes
        // can still be equal -- one nested declaration under two equal
        // enclosing values -- so they are a pair of their own, pushed after
        // this node's own pairs and so examined next: a class that settles
        // the answer does so before a slot can decline it.
        // Two numbers are equal by value across their classes -- `3` and
        // `3.0` -- so the tower decides them before the classes are
        // compared (`docs/design/core-library.md`, "The numeric tower").
        // The second side is read only when the first is a number: a
        // slot's `nil = anObject` then never asks the heap whether
        // `anObject` is one.
        if let Some((left, right)) = tower::operand(mutator.heap(), first)
            .and_then(|left| Some((left, tower::operand(mutator.heap(), second)?)))
        {
            if !tower::equal(&left, &right) {
                return Ok(Some(false));
            }
            continue;
        }
        let first_class = mutator.class_of(first);
        let second_class = mutator.class_of(second);
        // `valueEquals:` compares the classes with `self class == other
        // class`, a send to the receiver's class object, which answers by
        // `Behavior>>==` -- the rule the class test below applies -- unless
        // its class side declares `==` of its own; a program may. Where
        // one does, the classes are that `==`'s to compare, so the walk
        // declines -- asked of a pair `valueEquals:` would compare, the
        // root's (but a class object's, whose `=` is `Behavior`'s) and a
        // derived one's, from a flag of the metaclass's application. An
        // argument under construction needs no test here: `valueEquals:`
        // refuses it itself, through its slot readers, wherever that `==`
        // lets it go on to the slots.
        if receiver.is_none_or(|receiver| receiver == ReceiverEquality::Derived)
            && mutator.heap().class_side_declares_identity(first_class)
            && first
                .as_object()
                .is_none_or(|object| !mutator.heap().is_class(object))
        {
            return Ok(None);
        }
        let classes = if first_class.same(second_class) {
            None
        } else {
            // What `Pair::Classes` would decide at once, asked here so that
            // two values of unrelated classes -- the common unequal pair --
            // answer without unfolding either.
            if !classes_can_be_equal(mutator, first_class, second_class) {
                return Ok(Some(false));
            }
            // A pair already assumed has been unfolded; pushing it again
            // would only spend budget on a node the walk discards.
            let key = identity_pair(mutator, first_class, second_class);
            if assumed.as_ref().is_some_and(|set| set.contains(&key)) {
                None
            } else {
                if charged >= NODE_BUDGET {
                    return Ok(None);
                }
                charged += 1;
                Some((Value::object(first_class), Value::object(second_class)))
            }
        };
        // An argument under construction whose class cannot equal the
        // receiver's is unequal to it whatever its state, and was answered
        // `false` above without reading it -- `nil = aPoint`, `3 = aPoint`.
        // Past that the comparison would read its slots, which it does not
        // have yet. A number is never under construction, so the tower's
        // pairs need no test. A pair whose receiver's `=` is its own was
        // declined above, so this refuses only where the walk decides the
        // pair itself.
        if let Some(object) = under_construction(mutator, second) {
            return Err(object);
        }
        let Some(pair) = compare_node(mutator, first, second, receiver) else {
            return Ok(None);
        };
        match pair {
            Pair::Decided(true) => {}
            Pair::Decided(false) => return Ok(Some(false)),
            // Assumed like a pair of instances, so a cycle through classes
            // -- a class whose enclosing value's slot holds that very class
            // -- ends where one through slots does.
            Pair::Classes(first_class, second_class)
                if !first_class.same(second_class)
                    && assumed
                        .get_or_insert_with(HashSet::new)
                        .insert(identity_pair(mutator, first_class, second_class)) =>
            {
                let Some(pairs) = class_state_pairs(mutator, first_class, second_class) else {
                    return Ok(Some(false));
                };
                // Charged before they are pushed, like an instance's slots.
                if pairs.len() > NODE_BUDGET - charged {
                    return Ok(None);
                }
                charged += pairs.len();
                // Pushed back to front, so that they are examined in the
                // order `behaviorEquals:` compares them: a refusal or an
                // unequal pair is met where that method meets it.
                work.extend(pairs.into_iter().rev());
            }
            Pair::Classes(..) => {}
            Pair::State(first_object, second_object)
                if assumed
                    .get_or_insert_with(HashSet::new)
                    .insert(identity_pair(mutator, first_object, second_object)) =>
            {
                let first_slots = slots_of(mutator, first_object);
                let second_slots = slots_of(mutator, second_object);
                // One class lays out one set of slots, so this holds
                // already; checked because the alternative is zipping two
                // different lengths and calling the shorter prefix equal.
                if first_slots.len() != second_slots.len() {
                    return Ok(Some(false));
                }
                if first_slots.len() > NODE_BUDGET - charged {
                    // Out of budget, with pairs still unexamined: decline
                    // rather than claim an equality that was never checked,
                    // and decline *before* pushing them, so the stack never
                    // grows past the budget. `Value`'s `ifFail:` block then
                    // answers, as it does for any other shape this leaf
                    // will not rule on.
                    return Ok(None);
                }
                charged += first_slots.len();
                // Pushed back to front, so that the slots are examined in
                // declaration order, each unfolded before the next, as
                // `valueEquals:` compares them: a refusal or an unequal pair
                // is met where that method meets it.
                work.extend(first_slots.into_iter().zip(second_slots).rev());
            }
            Pair::State(..) => {}
        }
        work.extend(classes);
    }
    Ok(Some(true))
}

/// The key `assumed` remembers a pair of objects by, instances and class
/// objects alike.
fn identity_pair<'gc>(
    mutator: &Mutator<'gc>,
    first: ObjectReference<'gc>,
    second: ObjectReference<'gc>,
) -> (usize, usize) {
    (
        mutator.heap().object_identity_key(first),
        mutator.heap().object_identity_key(second),
    )
}

/// Whether two distinct class objects can be equal at all: both are values
/// -- a class object that is no value has its identity, and two distinct
/// ones are unequal -- and both apply one mixin application. Every pair
/// `Pair::Classes` answers `true` for passes this, a metaclass pair
/// included, since one application of a class's mixin has one application
/// of its class side over `Class` (`Mutator::attach_metaclass`).
fn classes_can_be_equal<'gc>(
    mutator: &Mutator<'gc>,
    first: ClassReference<'gc>,
    second: ClassReference<'gc>,
) -> bool {
    // The cheapest test first: the two applications.
    let same_application = match (
        mutator.heap().class_application_index(first),
        mutator.heap().class_application_index(second),
    ) {
        (Some(first_application), Some(second_application)) => {
            first_application == second_application
        }
        _ => false,
    };
    // Two body-less declarations applying one mixin share one application
    // (the application cache keys it by mixin and superclass application),
    // yet are two classes, as their body versions, two applications, are:
    // each is its own declaration's.
    same_application
        && class_object_is_a_value(mutator, first)
        && class_object_is_a_value(mutator, second)
        && mutator
            .heap()
            .classes_share_body_less_declaration(first, second)
}

/// The pairs two distinct class objects are equal by, or `None` when they
/// are unequal outright. Two metaclasses are equal when their classes are:
/// a metaclass's enclosing object is its class's and its superclass is
/// always `Class`, so its class is all its state. Any other two are equal
/// when they are values of one application, and for a body-less
/// declaration's class object of one declaration, whose enclosing objects are
/// equal and whose superclass class objects are -- the application alone
/// is not enough, because it is keyed by the superclass's *application*,
/// and two classes can share one over distinct superclass class objects
/// (§6.1).
fn class_state_pairs<'gc>(
    mutator: &Mutator<'gc>,
    first: ClassReference<'gc>,
    second: ClassReference<'gc>,
) -> Option<Vec<(Value<'gc>, Value<'gc>)>> {
    let heap = mutator.heap();
    if let (Some(first_class), Some(second_class)) =
        (heap.class_this_class(first), heap.class_this_class(second))
    {
        return Some(vec![(
            Value::object(first_class),
            Value::object(second_class),
        )]);
    }
    if !classes_can_be_equal(mutator, first, second) {
        return None;
    }
    let as_value =
        |object: Option<ObjectReference<'gc>>| object.map_or(Value::nil(), Value::object);
    // A body-less declaration's class object is also the declaration's,
    // whose enclosing object is state as a body version's own is; nil for
    // a root and for every other class object.
    let declaration_enclosing = |class: ClassReference<'gc>| {
        as_value(
            heap.class_declaration_scope(class)
                .and_then(|scope| scope.enclosing),
        )
    };
    Some(vec![
        (declaration_enclosing(first), declaration_enclosing(second)),
        (
            as_value(heap.class_enclosing(first)),
            as_value(heap.class_enclosing(second)),
        ),
        (
            as_value(heap.class_superclass(first)),
            as_value(heap.class_superclass(second)),
        ),
    ])
}

/// The mask the hash's running sum is kept within, in the leaf and in
/// `Value>>valueHashOfSlots` alike: every node's hash is added and the sum
/// masked at once, so it stays a non-negative `SmallInteger` whatever the
/// nodes add, and the masked sum is the same in both however they reach it.
const HASH_MASK: i128 = (1 << 60) - 1;

/// The hash `value`'s state gives it (`r[runtime.value.hash]`): over the
/// first `NODE_BUDGET` nodes of its unfolding, visited breadth-first --
/// the root first, then each node's slots in declaration order -- each
/// node's hash added and the sum masked by `HASH_MASK`, an unfolded node
/// adding its slot count. `None` is the decline, at the first node whose
/// `hash` is its own (`hash_node`), and `Err` the instance under
/// construction the walk reached first: the root, or a node it unfolds.
///
/// Breadth-first and bounded, decided with the user on 2026-11-28: two
/// equal values unfold to the same sequence of nodes however their states
/// share structure -- a ring of one value and an equal ring of two -- so
/// they hash alike, and a value whose state reaches itself is hashed by a
/// finite prefix of its unfolding rather than not at all. The method does
/// the same with a queue, so neither recurses.
fn derived_hash<'gc>(
    mutator: &Mutator<'gc>,
    value: Value<'gc>,
) -> Result<Option<i64>, ObjectReference<'gc>> {
    let mut total: i128 = 0;
    let mut add = |hash: i128| total = (total + hash) & HASH_MASK;
    // The root has reached `Value>>hash`, so it is unfolded whatever its
    // classes declare -- but a class object's, whose `Behavior>>hash` asks
    // this leaf of it, is hashed as a node.
    let root_slots = if value
        .as_object()
        .is_some_and(|object| mutator.heap().is_class(object))
    {
        match hash_node(mutator, value) {
            HashNode::Leaf(hash) => return Ok(Some(hash)),
            _ => return Ok(None),
        }
    } else {
        if let Some(object) = under_construction(mutator, value) {
            return Err(object);
        }
        value
            .as_object()
            .map_or_else(Vec::new, |object| slots_of(mutator, object))
    };
    add(root_slots.len() as i128);
    // The nodes still to visit, the root counted already; never more than
    // the budget will visit.
    let mut visited = 1usize;
    let mut queue: VecDeque<Value<'gc>> = VecDeque::new();
    let enqueue = |queue: &mut VecDeque<Value<'gc>>, visited: usize, slots: Vec<Value<'gc>>| {
        let room = NODE_BUDGET.saturating_sub(visited + queue.len());
        queue.extend(slots.into_iter().take(room));
    };
    enqueue(&mut queue, visited, root_slots);
    while let Some(current) = queue.pop_front() {
        visited += 1;
        match hash_node(mutator, current) {
            HashNode::Leaf(hash) => add(i128::from(hash)),
            HashNode::Own => return Ok(None),
            HashNode::Unfold(object) => {
                if mutator.heap().object_under_construction(object) {
                    return Err(object);
                }
                let slots = slots_of(mutator, object);
                add(slots.len() as i128);
                enqueue(&mut queue, visited, slots);
            }
        }
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the mask keeps the sum below 2^60"
    )]
    Ok(Some(total as i64))
}
#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::{constant_identity, derived_equality};
    use crate::{HostResult, HostSignal, Interpreter, Root, Symbol, Value, ValueKind};

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// What a leaf's failure block answers in these tests: a value no leaf
    /// here ever produces, so a decline is unmistakable.
    const DECLINED: i64 = -1;

    /// `selector`'s leaf run on `subject` with `arguments`, through a send
    /// to the mirror -- the way the reference lowering runs every kernel
    /// method, so this exercises the leaf exactly as `Kernel.ns` will.
    fn leaf(
        interpreter: &mut Interpreter,
        selector: &str,
        subject: &Root,
        arguments: &[Root],
    ) -> HostResult {
        let mirror = interpreter.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            mutator.stash(mirror)
        });
        let mut sent = vec![subject.clone()];
        sent.extend(arguments.iter().cloned());
        sent.push(interpreter.stash(Value::small_integer(DECLINED)));
        interpreter.send(&mirror, Symbol::intern(selector), &sent)
    }

    /// `valueSlotCount:asking:ifFail:` run on `subject`, asking `#hash`.
    fn slot_count(interpreter: &mut Interpreter, subject: &Root) -> HostResult {
        let asking = asking_hash(interpreter);
        leaf(
            interpreter,
            "valueSlotCount:asking:ifFail:",
            subject,
            &[asking],
        )
    }

    /// `value:slotAt:asking:ifFail:` run on `subject` at `index`, asking
    /// `#hash`.
    fn slot_at(interpreter: &mut Interpreter, subject: &Root, index: Root) -> HostResult {
        let asking = asking_hash(interpreter);
        leaf(
            interpreter,
            "value:slotAt:asking:ifFail:",
            subject,
            &[index, asking],
        )
    }

    /// The question `#hash`, as the slot readers are asked it.
    fn asking_hash(interpreter: &mut Interpreter) -> Root {
        interpreter.enter(|mutator| {
            let symbol = mutator.program_symbol("hash");
            mutator.stash(symbol)
        })
    }

    /// The integer a leaf answered, or `DECLINED`.
    fn answered_integer(interpreter: &mut Interpreter, result: HostResult, what: &str) -> i64 {
        let root = expect_ok(result, what);
        interpreter.enter(|mutator| {
            let answer = mutator.fetch(&root);
            match answer.as_small_integer() {
                Some(integer) => integer,
                None => panic!("{what} answered {}", mutator.heap().describe(answer)),
            }
        })
    }

    /// The boolean a leaf answered; `None` when it declined.
    fn answered_boolean(
        interpreter: &mut Interpreter,
        result: HostResult,
        what: &str,
    ) -> Option<bool> {
        let root = expect_ok(result, what);
        interpreter.enter(|mutator| {
            let answer = mutator.fetch(&root);
            match answer.kind() {
                ValueKind::Boolean(flag) => Some(flag),
                ValueKind::SmallInteger(DECLINED) => None,
                _ => panic!("{what} answered {}", mutator.heap().describe(answer)),
            }
        })
    }

    /// The boolean an ordinary Newspeak send answered.
    fn answered_flag(interpreter: &mut Interpreter, root: &Root, what: &str) -> bool {
        interpreter.enter(|mutator| {
            let answer = mutator.fetch(root);
            match answer.as_boolean() {
                Some(flag) => flag,
                None => panic!("{what} answered {}", mutator.heap().describe(answer)),
            }
        })
    }

    /// The integer an ordinary Newspeak send answered.
    fn answered_number(interpreter: &mut Interpreter, root: &Root, what: &str) -> i64 {
        interpreter.enter(|mutator| {
            let answer = mutator.fetch(root);
            match answer.as_small_integer() {
                Some(integer) => integer,
                None => panic!("{what} answered {}", mutator.heap().describe(answer)),
            }
        })
    }

    /// A module whose `Marker` declares the `=` these tests record as
    /// `Value`'s (`record_marker_as_values_equality`), standing in for
    /// `Kernel.ns`'s.
    ///
    /// `Value` declares the real one, and `a_value_fixture` below goes
    /// through it. This fixture's classes sit below `Object` instead,
    /// because that is the only way to *build* every shape the walks have
    /// to decline: a real value class refuses an instance whose slot holds
    /// an `Array` or reaches any other non-value
    /// (`instantiate::check_value_slot`), and a mutable slot makes the
    /// class uninstantiable altogether. Recording `Marker`'s method is what
    /// makes the leaf treat these instances as values all the same.
    fn a_fixture() -> (Interpreter, Root) {
        let source = indoc! {"
            class Fixture = Object new () (
                class Marker = Object new () (
                    public = other = ( ^self == other )
                )
                class Pair first: firstValue second: secondValue = Marker new (
                    | public first = firstValue. public second = secondValue. |
                ) ()
                class Couple first: firstValue second: secondValue = Marker new (
                    | public first = firstValue. public second = secondValue. |
                ) ()
                class Overriding = Marker new () (
                    public = other = ( ^false )
                )
                class Cyclic = Marker new (| public me = self. |) ()
                class Chain link: previous = Marker new (| public link = previous. |) ()
                class Nesting first: firstValue second: secondValue = Marker new (
                    | public first = firstValue. public second = secondValue. |
                ) (
                    class Tag = Object new () ()
                    public touch = ( ^Tag )
                )
                class Deeper first: firstValue second: secondValue third: thirdValue
                    = Nesting first: firstValue second: secondValue (
                    | public third = thirdValue. |
                ) ()
                public marker = ( ^Marker new )
                public pair: firstValue and: secondValue = ( ^Pair first: firstValue second: secondValue )
                public couple: firstValue and: secondValue = ( ^Couple first: firstValue second: secondValue )
                public overriding = ( ^Overriding new )
                public cyclic = ( ^Cyclic new )
                public chainOf: count = (
                    | index result |
                    result:: nil.
                    index:: 1.
                    [index <= count] whileTrue: [
                        result:: Chain link: result.
                        index:: index + 1].
                    ^result
                )
                public nesting: firstValue and: secondValue = ( ^Nesting first: firstValue second: secondValue )
                public deeper: firstValue and: secondValue and: thirdValue = (
                    ^Deeper first: firstValue second: secondValue third: thirdValue
                )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let fixture = expect_ok(interpreter.instantiate(&class, &[]), "Fixture new");
        (interpreter, fixture)
    }

    /// Records `Marker>>=` as the method `Value` declares, which is what
    /// the bootstrap does with the real one once `Kernel.ns` has it.
    fn record_marker_as_values_equality(interpreter: &mut Interpreter, fixture: &Root) {
        let marker = expect_ok(
            interpreter.send(fixture, Symbol::intern("marker"), &[]),
            "marker",
        );
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(&marker);
            let class = mutator.class_of(receiver);
            let Some(application) = mutator.heap().class_application(class) else {
                panic!("a Marker's class is a class");
            };
            let Some(method) = application.mixin.compiled_method(Symbol::EQUALS) else {
                panic!("Marker declares =");
            };
            mutator.value_equality_method = Some(method.clone());
            // And the `hash` a Marker answers by, as `Value`'s: the hash
            // walk unfolds a node only where both are.
            let crate::dispatch::Lookup::Method(hash, _) =
                crate::dispatch::lookup_public(mutator.heap(), class, Symbol::intern("hash"))
            else {
                panic!("a Marker answers hash");
            };
            mutator.value_hash_method = Some(hash);
        });
    }

    /// A module of real value classes -- declared `= Value new`, so the
    /// bootstrap's own `Value>>=` is the method the leaf recognizes and the
    /// `inherits_value` flag is the one `check_value_slot` reads. Nothing
    /// is recorded by hand here: this is the state a program is in.
    ///
    /// `Tag` overrides `=` and `hash`, which a value class may do -- it is
    /// still a value, so a slot may hold one -- and which takes the leaf
    /// out of the decision, since the leaf recognizes `Value`'s method by
    /// identity. `Boxed` is the one class whose instances therefore reach
    /// both paths, depending on what their slot holds. A `Tag` equals
    /// every `Tag` and nothing else; it asks the other object
    /// `isKindOfTag`, which any other object answers false, rather than
    /// for its class, which no program can read.
    ///
    /// `Outer`'s `Inner` is a class object per `Outer` instance, and a value
    /// because its enclosing `Outer` is. `Looped` holds its own `Inner` in a
    /// slot, so its state reaches itself through a class object's enclosing
    /// object: a cycle through classes.
    ///
    /// `escaped` answers an `Escaping` whose initializer handed it out and
    /// was then cut short by a caught signal: an instance of a value class
    /// that stays under construction for good.
    ///
    /// `undefinedObject`, `truth` and `falsity` answer second instances of
    /// the constants' kernel classes, heap objects, and `myBoolean` one of a
    /// program's subclass of `Boolean`; `agreeable` one of a value class
    /// whose own `=` answers `true` for anything; `plain` an ordinary
    /// object; `loose:` a value whose class side declares a `==` that
    /// answers `true` for anything; `half` a `Fraction` and `symbol` a
    /// Symbol; `large`, `negativeLarge` and
    /// `boxedFloat` answer numbers that are objects.
    fn a_value_fixture() -> (Interpreter, Root) {
        let source = indoc! {"
            class ValueFixture = Object <: Value () (
                class Point x: xValue y: yValue = Value new (
                    | public x = xValue. public y = yValue. |
                ) ()
                class Tag = Value new () (
                    public = other = ( ^other isKindOfTag )
                    public hash = ( ^7 )
                    public isKindOfTag = ( ^true )
                    public class Inner = Value new () ()
                )
                class Boxed on: contents = Value new (
                    | public contents = contents. |
                ) ()
                class Outer x: xValue = Value new (
                    | public x = xValue. |
                ) (
                    public class Inner = Value new () ()
                )
                class Looped x: xValue = Value new (
                    | public x = xValue. public inner = Inner. |
                ) (
                    public class Inner = Value new () ()
                )
                class Lazy x: xValue = Value new (
                    | public x = xValue. |
                ) (
                    lazy public doubled = x + x.
                )
                class Escaping box: box = Value new (
                    | public itself = box at: 1 put: self. public never = Error signal: 'stop'. |
                ) ()
                class MyBoolean = Boolean new () ()
                class Agreeable = Value new () (
                    public = other = ( ^true )
                    public hash = ( ^7 )
                )
                public agreeable = ( ^Agreeable new )
                class Plain = Object new () ()
                public plain = ( ^Plain new )
                class Loose x: xValue = Value new (
                    | public x = xValue. |
                ) () : (
                    public == other = ( ^true )
                )
                public loose: xValue = ( ^Loose x: xValue )
                class Numeral n: number = Value new (
                    | public n = number. |
                ) (
                    public = other = ( ^n = other )
                    public hash = ( ^n hash )
                )
                public numeral: number = ( ^Numeral n: number )
                class Hashy = Value new () (
                    public hash = ( ^5 )
                )
                public hashy = ( ^Hashy new )
                class HashSide = Value new () () : (
                    public hash = ( ^9 )
                )
                public hashSide = ( ^HashSide new )
                class EqualsSide = Value new () () : (
                    public = other = ( ^true )
                )
                public equalsSide = ( ^EqualsSide new )
                class Ring after: anchor pairs: count = Value new (
                    | public n = 1. public next = count = 1
                        ifTrue: [anchor isNil ifTrue: [self] ifFalse: [anchor]]
                        ifFalse: [
                            Ring
                                after: (anchor isNil ifTrue: [self] ifFalse: [anchor])
                                pairs: count - 1]. |
                ) ()
                public ringOf: count = ( ^Ring after: nil pairs: count )
                class BigRing = Value new (
                    | public n = 1000000000000000000. public next = self. |
                ) ()
                public bigRing = ( ^BigRing new )
                class Link n: number next: previous = Value new (
                    | public n = number. public next = previous. |
                ) ()
                public chainOf: count = (
                    | index result |
                    result:: nil.
                    index:: 1.
                    [index <= count] whileTrue: [
                        result:: Link n: index next: result.
                        index:: index + 1].
                    ^result
                )
                public half = ( ^1 / 2 )
                public symbol = ( ^#abc )
                public pointX: xValue y: yValue = ( ^Point x: xValue y: yValue )
                public undefinedObject = ( ^UndefinedObject new )
                public truth = ( ^True new )
                public falsity = ( ^False new )
                public myBoolean = ( ^MyBoolean new )
                public large = ( ^123456789012345678901234567890 )
                public negativeLarge = ( ^0 - 123456789012345678901234567890 )
                public boxedFloat = ( ^1.0e300 )
                public lazy: xValue = ( ^Lazy x: xValue )
                public tag = ( ^Tag new )
                public boxed: contents = ( ^Boxed on: contents )
                public outer: xValue = ( ^Outer x: xValue )
                public looped: xValue = ( ^Looped x: xValue )
                public escaped = (
                    | box |
                    box:: Array new: 1.
                    [Escaping box: box] on: Error do: [:error | nil].
                    ^box at: 1
                )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the value fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("value_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the value fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let fixture = expect_ok(interpreter.instantiate(&class, &[]), "ValueFixture new");
        (interpreter, fixture)
    }

    fn send(
        interpreter: &mut Interpreter,
        receiver: &Root,
        selector: &str,
        arguments: &[Root],
    ) -> Root {
        expect_ok(
            interpreter.send(receiver, Symbol::intern(selector), arguments),
            selector,
        )
    }

    /// The class of `subject`, read on the host side, since `class` is
    /// answered only to its receiver's own sends.
    fn class_of(interpreter: &mut Interpreter, subject: &Root) -> Root {
        interpreter.enter(|mutator| {
            let class = mutator.class_of(mutator.fetch(subject));
            mutator.stash(Value::object(class))
        })
    }

    fn a_string(interpreter: &mut Interpreter, text: &str) -> Root {
        interpreter.enter(|mutator| {
            let string = mutator.new_string(text.to_string());
            mutator.stash(Value::object(string))
        })
    }

    fn an_array(interpreter: &mut Interpreter) -> Root {
        interpreter.enter(|mutator| {
            let array = mutator.new_array(vec![Value::small_integer(1)]);
            mutator.stash(Value::object(array))
        })
    }

    /// Two strings built separately: equal, and hashing alike. That neither
    /// is the other object is the whole point of a value's equality.
    #[test]
    fn two_equal_strings_compare_equal_and_hash_alike() {
        let (mut interpreter, _fixture) = a_fixture();
        let first = a_string(&mut interpreter, "abc");
        let second = a_string(&mut interpreter, "abc");
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&second),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "'abc' sameAs: 'abc'"),
            Some(true)
        );
        let first_hash = leaf(&mut interpreter, "valueHash:ifFail:", &first, &[]);
        let first_hash = answered_integer(&mut interpreter, first_hash, "'abc' valueHash");
        let second_hash = leaf(&mut interpreter, "valueHash:ifFail:", &second, &[]);
        let second_hash = answered_integer(&mut interpreter, second_hash, "'abc' valueHash");
        assert_ne!(first_hash, DECLINED);
        assert_eq!(first_hash, second_hash);
    }

    /// Two numbers are one value when their exact values are, whatever
    /// their classes: the tower decides them before the classes are
    /// compared, as `Float>>=` and `Integer>>=` decide them, and the
    /// derived hash gives the two one hash.
    #[test]
    fn an_integer_and_a_float_of_one_magnitude_are_one_value() {
        let (mut interpreter, _fixture) = a_fixture();
        let float = interpreter.enter(|mutator| {
            let float = mutator.new_float(1.0);
            mutator.stash(float)
        });
        let integer = interpreter.stash(Value::small_integer(1));
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &float,
            std::slice::from_ref(&integer),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "1.0 sameAs: 1"),
            Some(true)
        );
        let float_hash = leaf(&mut interpreter, "valueHash:ifFail:", &float, &[]);
        let float_hash = answered_integer(&mut interpreter, float_hash, "1.0 valueHash");
        let integer_hash = leaf(&mut interpreter, "valueHash:ifFail:", &integer, &[]);
        let integer_hash = answered_integer(&mut interpreter, integer_hash, "1 valueHash");
        assert_ne!(float_hash, DECLINED);
        assert_eq!(float_hash, integer_hash);
    }

    #[test]
    fn two_instances_of_one_value_class_compare_by_their_slots() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let three = interpreter.stash(Value::small_integer(3));
        let first = send(
            &mut interpreter,
            &fixture,
            "pair:and:",
            &[one.clone(), two.clone()],
        );
        let same = send(&mut interpreter, &fixture, "pair:and:", &[one.clone(), two]);
        let different = send(&mut interpreter, &fixture, "pair:and:", &[one, three]);
        let equal = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&same),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, equal, "equal slots"),
            Some(true)
        );
        let unequal = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            &[different],
        );
        assert_eq!(
            answered_boolean(&mut interpreter, unequal, "unequal slots"),
            Some(false)
        );
        let first_hash = leaf(&mut interpreter, "valueHash:ifFail:", &first, &[]);
        let first_hash = answered_integer(&mut interpreter, first_hash, "valueHash");
        let same_hash = leaf(&mut interpreter, "valueHash:ifFail:", &same, &[]);
        let same_hash = answered_integer(&mut interpreter, same_hash, "valueHash");
        assert_ne!(first_hash, DECLINED);
        assert_eq!(first_hash, same_hash, "equal values hash alike");
    }

    /// The same slots in another class: §3.1.1's equality is equality of
    /// state *within* a class.
    #[test]
    fn an_instance_of_another_class_is_never_equal() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let pair = send(
            &mut interpreter,
            &fixture,
            "pair:and:",
            &[one.clone(), two.clone()],
        );
        let couple = send(&mut interpreter, &fixture, "couple:and:", &[one, two]);
        let compared = leaf(&mut interpreter, "value:sameAs:ifFail:", &pair, &[couple]);
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "a Pair against a Couple"),
            Some(false)
        );
    }

    /// `| public me = self. |`: the walk assumes a pair it is already
    /// unfolding to be equal, so it terminates instead of following the
    /// cycle for ever.
    #[test]
    fn a_cyclic_value_compares_equal_to_itself_and_terminates() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let cyclic = send(&mut interpreter, &fixture, "cyclic", &[]);
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &cyclic,
            std::slice::from_ref(&cyclic),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "a cyclic value against itself"),
            Some(true)
        );
        let hash = leaf(&mut interpreter, "valueHash:ifFail:", &cyclic, &[]);
        let hash = answered_integer(&mut interpreter, hash, "a cyclic value's hash");
        assert_ne!(hash, DECLINED, "the hash terminates on a cycle too");
    }

    /// A value deeper than the node budget: the equality declines rather
    /// than claim an equality it never checked, and the hash answers a
    /// truncated but consistent number.
    ///
    /// Both walks charge the budget when a node is **pushed**, so the work
    /// vector never grows past `NODE_BUDGET` entries whatever the value's
    /// width. Charging on the take let a node push all of its slots first,
    /// and an image -- untrusted input -- picks that width.
    #[test]
    fn a_value_deeper_than_the_budget_declines_and_still_hashes() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let depth = interpreter.stash(Value::small_integer(super::NODE_BUDGET as i64 + 100));
        let first = send(
            &mut interpreter,
            &fixture,
            "chainOf:",
            std::slice::from_ref(&depth),
        );
        let second = send(&mut interpreter, &fixture, "chainOf:", &[depth]);
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&second),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "two chains past the budget"),
            None
        );
        let first_hash = leaf(&mut interpreter, "valueHash:ifFail:", &first, &[]);
        let first_hash = answered_integer(&mut interpreter, first_hash, "the first chain's hash");
        let second_hash = leaf(&mut interpreter, "valueHash:ifFail:", &second, &[]);
        let second_hash =
            answered_integer(&mut interpreter, second_hash, "the second chain's hash");
        assert_ne!(first_hash, DECLINED, "a truncated hash is still a hash");
        assert_eq!(
            first_hash, second_hash,
            "two equal values are cut at the same node"
        );
    }

    /// An `Array` is no value: it is mutable and sits under `Object`, so
    /// its `=` is `Object`'s identity. A slot holding one takes the whole
    /// comparison out of the leaf's hands.
    #[test]
    fn a_slot_holding_an_array_makes_the_leaf_decline() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let one = interpreter.stash(Value::small_integer(1));
        let array = an_array(&mut interpreter);
        let other_array = an_array(&mut interpreter);
        let first = send(
            &mut interpreter,
            &fixture,
            "pair:and:",
            &[one.clone(), array],
        );
        let second = send(&mut interpreter, &fixture, "pair:and:", &[one, other_array]);
        let compared = leaf(&mut interpreter, "value:sameAs:ifFail:", &first, &[second]);
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "a slot holding an Array"),
            None
        );
    }

    /// A class that declares its own `=` decides its own equality, so the
    /// derivation may not read its slots instead.
    #[test]
    fn a_slot_whose_class_declares_its_own_equality_makes_the_leaf_decline() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let one = interpreter.stash(Value::small_integer(1));
        let overriding = send(&mut interpreter, &fixture, "overriding", &[]);
        let other = send(&mut interpreter, &fixture, "overriding", &[]);
        let first = send(
            &mut interpreter,
            &fixture,
            "pair:and:",
            &[one.clone(), overriding],
        );
        let second = send(&mut interpreter, &fixture, "pair:and:", &[one, other]);
        let compared = leaf(&mut interpreter, "value:sameAs:ifFail:", &first, &[second]);
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "a slot that overrides ="),
            None
        );
    }

    /// The bootstrap records `Value>>=` itself, and the derivation asks
    /// whether an instance's `=` *is* that method. The fixture's classes
    /// inherit `Object` through a `Marker` that declares its own `=`, so
    /// they are no values at all and the leaf declines them -- which is
    /// what keeps the derivation off every ordinary object.
    #[test]
    fn an_instance_whose_equality_is_not_values_declines() {
        let (mut interpreter, fixture) = a_fixture();
        assert!(
            interpreter.enter(|mutator| mutator.value_equality_method.is_some()),
            "the bootstrap records Kernel.ns's Value>>="
        );
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let first = send(
            &mut interpreter,
            &fixture,
            "pair:and:",
            &[one.clone(), two.clone()],
        );
        let second = send(&mut interpreter, &fixture, "pair:and:", &[one, two]);
        let compared = leaf(&mut interpreter, "value:sameAs:ifFail:", &first, &[second]);
        assert_eq!(
            answered_boolean(
                &mut interpreter,
                compared,
                "an instance whose class declares its own ="
            ),
            None
        );
    }

    /// A value class below the real `Value`, which exercises the recorded
    /// method and the `inherits_value` flag together: the flag is what lets
    /// the instance be built with slots at all
    /// (`instantiate::check_value_slot`), and the method the bootstrap
    /// recorded off `Value`'s own application is what makes the leaf unfold
    /// the instance instead of declining it. Every other test here records
    /// a stand-in by hand, so nothing else would notice the bootstrap
    /// recording the wrong method.
    #[test]
    fn two_points_below_the_real_value_class_are_one_value() {
        let (mut interpreter, fixture) = a_value_fixture();
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let three = interpreter.stash(Value::small_integer(3));
        let first = send(
            &mut interpreter,
            &fixture,
            "pointX:y:",
            &[one.clone(), two.clone()],
        );
        let same = send(&mut interpreter, &fixture, "pointX:y:", &[one.clone(), two]);
        let different = send(&mut interpreter, &fixture, "pointX:y:", &[one, three]);
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&same),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "the leaf on two Points"),
            Some(true),
            "the leaf decides a value class below the real `Value`"
        );
        for (argument, expected, what) in [
            (&same, true, "two Points of one state"),
            (&different, false, "two Points of different states"),
        ] {
            let equal = send(
                &mut interpreter,
                &first,
                "=",
                std::slice::from_ref(argument),
            );
            assert_eq!(answered_flag(&mut interpreter, &equal, what), expected);
            let identical = send(
                &mut interpreter,
                &first,
                "==",
                std::slice::from_ref(argument),
            );
            assert_eq!(
                answered_flag(&mut interpreter, &identical, what),
                expected,
                "{what}: `Value>>==` answers what `=` answers"
            );
        }
        let first_hash = send(&mut interpreter, &first, "hash", &[]);
        let first_hash = answered_number(&mut interpreter, &first_hash, "a Point's hash");
        let same_hash = send(&mut interpreter, &same, "hash", &[]);
        let same_hash = answered_number(&mut interpreter, &same_hash, "a Point's hash");
        assert_eq!(first_hash, same_hash, "two equal Points hash alike");
    }

    /// `hash_node` hashes an `Integer` slot with the integer's own hash,
    /// which `Integer>>hash` answers and `valueHashOfSlots` sums, so a value
    /// the leaf hashes and one the fallback hashes hash alike wherever they
    /// are equal: a `Boxed` holding `3` and a `Boxed` holding a `Numeral`
    /// whose own `=` finds it equal to `3` and whose own `hash` is `3`'s.
    /// Two values of a shape each path takes alone still hash consistently
    /// within it -- a `Boxed` holding an `Integer`, which the leaf hashes,
    /// and a `Boxed` holding a `Tag`, whose own `=` takes the leaf out of
    /// the decision.
    // spec: runtime.value.hash
    #[test]
    fn a_leaf_hashed_value_hashes_as_the_fallback_hashes_an_equal_one() {
        let (mut interpreter, fixture) = a_value_fixture();
        let one = interpreter.stash(Value::small_integer(1));
        let decided = send(
            &mut interpreter,
            &fixture,
            "boxed:",
            std::slice::from_ref(&one),
        );
        let other_decided = send(&mut interpreter, &fixture, "boxed:", &[one]);
        let tag = send(&mut interpreter, &fixture, "tag", &[]);
        let declined = send(&mut interpreter, &fixture, "boxed:", &[tag]);
        let other_tag = send(&mut interpreter, &fixture, "tag", &[]);
        let other_declined = send(&mut interpreter, &fixture, "boxed:", &[other_tag]);

        let hashed = leaf(&mut interpreter, "valueHash:ifFail:", &decided, &[]);
        assert_ne!(
            answered_integer(&mut interpreter, hashed, "a Boxed holding an Integer"),
            DECLINED,
            "an Integer slot is a shape the leaf hashes itself"
        );
        let hashed = leaf(&mut interpreter, "valueHash:ifFail:", &declined, &[]);
        assert_eq!(
            answered_integer(&mut interpreter, hashed, "a Boxed holding a Tag"),
            DECLINED,
            "a slot whose class brings its own `=` leaves the decision to Newspeak"
        );

        let three = interpreter.stash(Value::small_integer(3));
        let numeral = send(
            &mut interpreter,
            &fixture,
            "numeral:",
            std::slice::from_ref(&three),
        );
        let holding_three = send(&mut interpreter, &fixture, "boxed:", &[three]);
        let holding_numeral = send(&mut interpreter, &fixture, "boxed:", &[numeral]);
        let equal = send(
            &mut interpreter,
            &holding_numeral,
            "=",
            std::slice::from_ref(&holding_three),
        );
        assert!(
            answered_flag(&mut interpreter, &equal, "a boxed numeral 3 = a boxed 3"),
            "the numeral's own `=` finds the two slots equal"
        );
        let hashes = [&holding_numeral, &holding_three].map(|subject| {
            let hashed = send(&mut interpreter, subject, "hash", &[]);
            answered_number(&mut interpreter, &hashed, "a boxed 3's hash")
        });
        assert_eq!(
            hashes[0], hashes[1],
            "the fallback's hash of the one and the leaf's of the other agree"
        );

        for (first, second, what) in [
            (&decided, &other_decided, "two leaf-hashed values"),
            (&declined, &other_declined, "two fallback-hashed values"),
        ] {
            let equal = send(&mut interpreter, first, "=", std::slice::from_ref(second));
            assert!(
                answered_flag(&mut interpreter, &equal, what),
                "{what} are equal"
            );
            let first_hash = send(&mut interpreter, first, "hash", &[]);
            let first_hash = answered_number(&mut interpreter, &first_hash, what);
            let second_hash = send(&mut interpreter, second, "hash", &[]);
            let second_hash = answered_number(&mut interpreter, &second_hash, what);
            assert_eq!(first_hash, second_hash, "{what} hash alike");
        }
    }

    /// `outer: x`'s nested `Inner`, one class object per `Outer`.
    fn an_inner_class(interpreter: &mut Interpreter, fixture: &Root, x_value: i64) -> Root {
        let argument = interpreter.stash(Value::small_integer(x_value));
        let outer = send(interpreter, fixture, "outer:", &[argument]);
        send(interpreter, &outer, "Inner", &[])
    }

    /// Two `Inner`s of two equal `Outer`s are two class objects, and one
    /// value: §3.1.1 makes the two `Outer`s the same value and §6.1 has the
    /// same receiver answer the same class. `==` answers what `=` does.
    #[test]
    fn two_value_class_objects_of_one_declaration_compare_equal() {
        let (mut interpreter, fixture) = a_value_fixture();
        let first = an_inner_class(&mut interpreter, &fixture, 1);
        let second = an_inner_class(&mut interpreter, &fixture, 1);
        assert!(
            !interpreter.enter(|mutator| {
                let first = mutator.fetch(&first);
                let second = mutator.fetch(&second);
                matches!((first.as_object(), second.as_object()), (Some(left), Some(right)) if left.same(right))
            }),
            "each Outer has a class object of its own"
        );
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&second),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "two equal Inners"),
            Some(true)
        );
        for selector in ["=", "=="] {
            let equal = send(
                &mut interpreter,
                &first,
                selector,
                std::slice::from_ref(&second),
            );
            assert!(
                answered_flag(&mut interpreter, &equal, selector),
                "Inner {selector} Inner"
            );
        }
    }

    /// The same declaration under two unequal `Outer`s: the class objects'
    /// enclosing objects differ, so they do.
    #[test]
    fn class_objects_of_one_declaration_under_unequal_values_compare_unequal() {
        let (mut interpreter, fixture) = a_value_fixture();
        let first = an_inner_class(&mut interpreter, &fixture, 1);
        let second = an_inner_class(&mut interpreter, &fixture, 2);
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&second),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "Inners of unequal Outers"),
            Some(false)
        );
        let equal = send(&mut interpreter, &first, "==", &[second]);
        assert!(!answered_flag(&mut interpreter, &equal, "=="));
    }

    /// An instance's class is compared as a pair, not by identity, so two
    /// instances of two equal class objects are one value -- and two of two
    /// unequal ones are not, although both are stateless.
    #[test]
    fn instances_of_equal_class_objects_compare_equal() {
        let (mut interpreter, fixture) = a_value_fixture();
        let first_class = an_inner_class(&mut interpreter, &fixture, 1);
        let second_class = an_inner_class(&mut interpreter, &fixture, 1);
        let other_class = an_inner_class(&mut interpreter, &fixture, 2);
        let first = send(&mut interpreter, &first_class, "new", &[]);
        let second = send(&mut interpreter, &second_class, "new", &[]);
        let other = send(&mut interpreter, &other_class, "new", &[]);
        for (argument, expected, what) in [
            (&second, Some(true), "instances of equal classes"),
            (&other, Some(false), "instances of unequal classes"),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                &first,
                std::slice::from_ref(argument),
            );
            assert_eq!(
                answered_boolean(&mut interpreter, compared, what),
                expected,
                "{what}"
            );
        }
        let first_hash = send(&mut interpreter, &first, "hash", &[]);
        let first_hash = answered_number(&mut interpreter, &first_hash, "an Inner's hash");
        let second_hash = send(&mut interpreter, &second, "hash", &[]);
        let second_hash = answered_number(&mut interpreter, &second_hash, "an Inner's hash");
        assert_eq!(first_hash, second_hash, "two equal instances hash alike");
    }

    /// `Fixture` is an ordinary object, so a class nested in it is no value
    /// and keeps its identity: equal to itself, unequal to the same
    /// declaration under another `Fixture`. Its hash is `Object`'s identity
    /// hash, so the hash leaf declines while the equality leaf answers --
    /// consistent, because that equality is identity.
    #[test]
    fn a_class_object_that_is_no_value_compares_by_identity() {
        let (mut interpreter, fixture) = a_fixture();
        let fixture_class = class_of(&mut interpreter, &fixture);
        let other_fixture = send(&mut interpreter, &fixture_class, "new", &[]);
        let marker = send(&mut interpreter, &fixture, "marker", &[]);
        let marker_class = class_of(&mut interpreter, &marker);
        let other_marker = send(&mut interpreter, &other_fixture, "marker", &[]);
        let other_marker_class = class_of(&mut interpreter, &other_marker);
        for (argument, expected, what) in [
            (
                &marker_class,
                true,
                "a class that is no value against itself",
            ),
            (
                &other_marker_class,
                false,
                "one declaration under two ordinary objects",
            ),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                &marker_class,
                std::slice::from_ref(argument),
            );
            assert_eq!(
                answered_boolean(&mut interpreter, compared, what),
                Some(expected),
                "{what}"
            );
            for selector in ["=", "=="] {
                let equal = send(
                    &mut interpreter,
                    &marker_class,
                    selector,
                    std::slice::from_ref(argument),
                );
                assert_eq!(
                    answered_flag(&mut interpreter, &equal, what),
                    expected,
                    "{what}: {selector}"
                );
            }
        }
        let hashed = leaf(&mut interpreter, "valueHash:ifFail:", &marker_class, &[]);
        assert_eq!(
            answered_integer(&mut interpreter, hashed, "a class that is no value"),
            DECLINED,
            "a class object that is no value keeps its identity hash"
        );
        let first_hash = send(&mut interpreter, &marker_class, "hash", &[]);
        let first_hash = answered_number(&mut interpreter, &first_hash, "Marker hash");
        let again = send(&mut interpreter, &marker_class, "hash", &[]);
        let again = answered_number(&mut interpreter, &again, "Marker hash");
        assert_eq!(first_hash, again, "an identity hash is stable");
    }

    /// Two equal class objects hash alike, through the leaf and through
    /// `Behavior>>hash` alike, and so do their metaclasses.
    #[test]
    fn equal_class_objects_hash_alike() {
        let (mut interpreter, fixture) = a_value_fixture();
        let first = an_inner_class(&mut interpreter, &fixture, 1);
        let second = an_inner_class(&mut interpreter, &fixture, 1);
        let first_hash = leaf(&mut interpreter, "valueHash:ifFail:", &first, &[]);
        let first_hash = answered_integer(&mut interpreter, first_hash, "the first Inner");
        let second_hash = leaf(&mut interpreter, "valueHash:ifFail:", &second, &[]);
        let second_hash = answered_integer(&mut interpreter, second_hash, "the second Inner");
        assert_ne!(
            first_hash, DECLINED,
            "a value class object's hash is decided"
        );
        assert_eq!(first_hash, second_hash);
        let first_metaclass = class_of(&mut interpreter, &first);
        let second_metaclass = class_of(&mut interpreter, &second);
        let equal = send(
            &mut interpreter,
            &first_metaclass,
            "=",
            std::slice::from_ref(&second_metaclass),
        );
        assert!(answered_flag(
            &mut interpreter,
            &equal,
            "two equal metaclasses"
        ));
        let first_hash = send(&mut interpreter, &first_metaclass, "hash", &[]);
        let first_hash = answered_number(&mut interpreter, &first_hash, "a metaclass's hash");
        let second_hash = send(&mut interpreter, &second_metaclass, "hash", &[]);
        let second_hash = answered_number(&mut interpreter, &second_hash, "a metaclass's hash");
        assert_eq!(first_hash, second_hash, "two equal metaclasses hash alike");
    }

    /// `Tag` brings its own `=`, so the walk declines on a pair of `Tag`s
    /// and the Newspeak side of `Behavior>>=` compares two `Tag`s' `Inner`s
    /// instead, reading their state through the four leaves it has for
    /// it. The two agree with what the leaf decides where it can: two
    /// `Inner`s of two equal `Tag`s are equal, and hash alike, while an
    /// `Inner` and an `Outer`'s `Inner` share no application.
    #[test]
    fn a_class_object_under_a_value_with_its_own_equality_is_compared_in_newspeak() {
        let (mut interpreter, fixture) = a_value_fixture();
        let first_tag = send(&mut interpreter, &fixture, "tag", &[]);
        let second_tag = send(&mut interpreter, &fixture, "tag", &[]);
        let first = send(&mut interpreter, &first_tag, "Inner", &[]);
        let second = send(&mut interpreter, &second_tag, "Inner", &[]);
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&second),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "two Tags' Inners"),
            None,
            "a Tag's own `=` takes the leaf out of the decision"
        );
        for selector in ["=", "=="] {
            let equal = send(
                &mut interpreter,
                &first,
                selector,
                std::slice::from_ref(&second),
            );
            assert!(
                answered_flag(&mut interpreter, &equal, selector),
                "two Tags' Inners: {selector}"
            );
        }
        let first_hash = send(&mut interpreter, &first, "hash", &[]);
        let first_hash = answered_number(&mut interpreter, &first_hash, "an Inner's hash");
        let second_hash = send(&mut interpreter, &second, "hash", &[]);
        let second_hash = answered_number(&mut interpreter, &second_hash, "an Inner's hash");
        assert_eq!(first_hash, second_hash, "two equal Inners hash alike");
        let unrelated = an_inner_class(&mut interpreter, &fixture, 1);
        let equal = send(&mut interpreter, &first, "=", &[unrelated]);
        assert!(
            !answered_flag(&mut interpreter, &equal, "a Tag's Inner and an Outer's"),
            "two declarations are two classes"
        );
    }

    /// The Newspeak side of `Behavior>>=` compares a body-less declaration's
    /// class objects by both their enclosing objects, as the walk does: two
    /// `D`s of two equal `Tag`s -- `Tag` brings its own `=`, so the leaf
    /// declines -- whose clauses applied the `Inner` of two unequal `Lib`s
    /// share one application and one declaration, and their declarations'
    /// enclosing objects are equal, but their levels' are not.
    #[test]
    fn the_newspeak_side_compares_a_body_less_class_objects_level_enclosing_object() {
        let source = indoc! {"
            class LevelFixture = Object <: Value () (
                class Lib n: number = Value new (| public n = number. |) (
                    public = other = ( ^n = other n )
                    public hash = ( ^n )
                    public class Inner = Value new () ()
                )
                class Tag key: aKey other: aLib = Value new (
                    | public key = aKey. public lib = aLib. |
                ) (
                    public = other = ( ^key = other key )
                    public hash = ( ^key )
                    public class D = Value new <: libInner.
                    libInner = ( ^lib Inner )
                )
                public tagOver: number = ( ^Tag key: 1 other: (Lib n: number) )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the level fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("level_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the level fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let fixture = expect_ok(interpreter.instantiate(&class, &[]), "LevelFixture new");
        let classes: Vec<Root> = [1, 1, 2]
            .into_iter()
            .map(|number| {
                let argument =
                    interpreter.enter(|mutator| mutator.stash(Value::small_integer(number)));
                let tag = send(&mut interpreter, &fixture, "tagOver:", &[argument]);
                send(&mut interpreter, &tag, "D", &[])
            })
            .collect();
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &classes[0],
            std::slice::from_ref(&classes[2]),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "two Tags' Ds"),
            None,
            "a Tag's own `=` takes the leaf out of the decision"
        );
        let equal = send(
            &mut interpreter,
            &classes[0],
            "=",
            std::slice::from_ref(&classes[1]),
        );
        assert!(
            answered_flag(&mut interpreter, &equal, "over equal Libs"),
            "two Ds over equal Libs are equal"
        );
        let equal = send(
            &mut interpreter,
            &classes[0],
            "=",
            std::slice::from_ref(&classes[2]),
        );
        assert!(
            !answered_flag(&mut interpreter, &equal, "over unequal Libs"),
            "two Ds over unequal Libs are not"
        );
    }

    /// The answers `derived_equality` gives two class objects before it
    /// builds its walk -- `self class == other class`'s path: identical
    /// class objects are equal, whether values or not, and two distinct
    /// ones that are not values of one application are unequal, metaclasses
    /// included. Each is checked against `=` and `==` sent in Newspeak, and
    /// an equal pair against its hash.
    #[test]
    fn two_class_objects_are_decided_before_the_walk() {
        let (mut interpreter, fixture) = a_fixture();
        let builtin = |interpreter: &mut Interpreter, kind: crate::BuiltinClass| {
            interpreter.enter(|mutator| {
                let class = mutator.builtin_class(kind);
                mutator.stash(Value::object(class))
            })
        };
        let string = builtin(&mut interpreter, crate::BuiltinClass::String);
        let symbol = builtin(&mut interpreter, crate::BuiltinClass::Symbol);
        let string_metaclass = class_of(&mut interpreter, &string);
        let symbol_metaclass = class_of(&mut interpreter, &symbol);
        let marker = send(&mut interpreter, &fixture, "marker", &[]);
        let marker_class = class_of(&mut interpreter, &marker);
        for (first, second, expected, what) in [
            (&string, &string, true, "a value class object and itself"),
            (
                &marker_class,
                &marker_class,
                true,
                "a class that is no value and itself",
            ),
            (
                &string_metaclass,
                &string_metaclass,
                true,
                "a metaclass and itself",
            ),
            (&string, &symbol, false, "two builtin classes"),
            (
                &string_metaclass,
                &symbol_metaclass,
                false,
                "two builtin metaclasses",
            ),
            (
                &string,
                &string_metaclass,
                false,
                "a class and its metaclass",
            ),
            (
                &string,
                &marker_class,
                false,
                "a value class and one that is none",
            ),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                first,
                std::slice::from_ref(second),
            );
            assert_eq!(
                answered_boolean(&mut interpreter, compared, what),
                Some(expected),
                "{what}: the leaf"
            );
            for selector in ["=", "=="] {
                let equal = send(
                    &mut interpreter,
                    first,
                    selector,
                    std::slice::from_ref(second),
                );
                assert_eq!(
                    answered_flag(&mut interpreter, &equal, what),
                    expected,
                    "{what}: {selector}"
                );
            }
            if expected {
                let first_hash = send(&mut interpreter, first, "hash", &[]);
                let first_hash = answered_number(&mut interpreter, &first_hash, what);
                let second_hash = send(&mut interpreter, second, "hash", &[]);
                let second_hash = answered_number(&mut interpreter, &second_hash, what);
                assert_eq!(first_hash, second_hash, "{what}: equal, so hashing alike");
            }
        }
    }

    /// `Looped`'s slot holds its own `Inner`, whose enclosing object is the
    /// `Looped` again: the walk assumes the class pair as it does an
    /// instance pair, so it terminates rather than unfolding the cycle.
    #[test]
    fn a_cycle_through_a_nested_class_terminates() {
        let (mut interpreter, fixture) = a_value_fixture();
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let first = send(
            &mut interpreter,
            &fixture,
            "looped:",
            std::slice::from_ref(&one),
        );
        let second = send(&mut interpreter, &fixture, "looped:", &[one]);
        let other = send(&mut interpreter, &fixture, "looped:", &[two]);
        for (argument, expected, what) in [
            (&second, Some(true), "two equal loops"),
            (&other, Some(false), "two unequal loops"),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                &first,
                std::slice::from_ref(argument),
            );
            assert_eq!(
                answered_boolean(&mut interpreter, compared, what),
                expected,
                "{what}"
            );
        }
        let first_hash = leaf(&mut interpreter, "valueHash:ifFail:", &first, &[]);
        let first_hash = answered_integer(&mut interpreter, first_hash, "a loop's hash");
        let second_hash = leaf(&mut interpreter, "valueHash:ifFail:", &second, &[]);
        let second_hash = answered_integer(&mut interpreter, second_hash, "a loop's hash");
        assert_ne!(first_hash, DECLINED);
        assert_eq!(first_hash, second_hash, "two equal loops hash alike");
    }

    /// A nested class declaration is not a value's state. §6 gives every
    /// instance "a distinct nested class for each nested class
    /// declaration", cached in a hidden slot of the enclosing instance, so
    /// two equal values hold *different* class objects there once each has
    /// been sent its own accessor. Counting that slot made them answer
    /// `false`, at exit status 0. Both walks and both reflective leaves
    /// read `declared_slots`, which is why one test covers all four.
    #[test]
    fn a_nested_classs_cache_slot_is_not_part_of_a_values_state() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        // Three shapes: neither accessor run, one run, both run. The
        // one-sided case is the one a fix that merely ignored a *differing*
        // class object would still get wrong, since one side's slot is nil.
        for touches in 0..=2 {
            let first = send(
                &mut interpreter,
                &fixture,
                "nesting:and:",
                &[one.clone(), two.clone()],
            );
            let second = send(
                &mut interpreter,
                &fixture,
                "nesting:and:",
                &[one.clone(), two.clone()],
            );
            if touches >= 1 {
                send(&mut interpreter, &first, "touch", &[]);
            }
            if touches == 2 {
                send(&mut interpreter, &second, "touch", &[]);
            }
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                &first,
                std::slice::from_ref(&second),
            );
            assert_eq!(
                answered_boolean(&mut interpreter, compared, "two equal nesting values"),
                Some(true),
                "{touches} of the two accessors run: the values are equal"
            );
            let first_hash = leaf(&mut interpreter, "valueHash:ifFail:", &first, &[]);
            let first_hash = answered_integer(&mut interpreter, first_hash, "the first hash");
            let second_hash = leaf(&mut interpreter, "valueHash:ifFail:", &second, &[]);
            let second_hash = answered_integer(&mut interpreter, second_hash, "the second hash");
            assert_ne!(first_hash, DECLINED, "{touches} run: the hash decides");
            assert_eq!(
                first_hash, second_hash,
                "{touches} of the two accessors run: two equal values hash alike"
            );
            // The count the Newspeak fallback would iterate: the two
            // declared slots, never the hidden third.
            let count = slot_count(&mut interpreter, &first);
            assert_eq!(
                answered_integer(&mut interpreter, count, "the declared slot count"),
                2,
                "{touches} run: only the declared slots count"
            );
        }
    }

    /// A lazy slot (§6.3.3) is state derived from the rest, and decided
    /// with the user on 2026-09-26 (`docs/design/core-library.md`, "Lazy
    /// slots first") to stay out of a value's state: two values one of
    /// which has filled it are equal, and the fallback's count skips it.
    /// Counting it would answer `false` for the one-sided pair, at exit
    /// status 0.
    #[test]
    fn a_lazy_slot_is_not_part_of_a_values_state() {
        let (mut interpreter, fixture) = a_value_fixture();
        let three = interpreter.stash(Value::small_integer(3));
        let first = send(
            &mut interpreter,
            &fixture,
            "lazy:",
            std::slice::from_ref(&three),
        );
        let second = send(
            &mut interpreter,
            &fixture,
            "lazy:",
            std::slice::from_ref(&three),
        );
        let doubled = send(&mut interpreter, &first, "doubled", &[]);
        assert_eq!(
            answered_number(&mut interpreter, &doubled, "doubled"),
            6,
            "the lazy slot is filled on the first instance"
        );
        for (subject, what) in [(&first, "filled"), (&second, "empty")] {
            let count = slot_count(&mut interpreter, subject);
            assert_eq!(
                answered_integer(&mut interpreter, count, "the declared slot count"),
                1,
                "the {what} instance counts its one eager slot only"
            );
        }
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&second),
        );
        assert_eq!(
            answered_boolean(
                &mut interpreter,
                compared,
                "a filled and an empty lazy value"
            ),
            Some(true),
            "a filled lazy slot does not make two values unequal"
        );
    }

    /// The declared slots of a chain are not a prefix of the fields: each
    /// level lays its own slots down and then one hidden slot per nested
    /// declaration, so `Deeper`'s `third` sits *after* `Nesting`'s cache
    /// slot. The leaves must index past it, which is what keeps them in
    /// step with the walks -- both go through `declared_slots`.
    #[test]
    fn a_hidden_slot_between_two_levels_does_not_shift_the_reflective_leaves() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let three = interpreter.stash(Value::small_integer(3));
        let deeper = send(
            &mut interpreter,
            &fixture,
            "deeper:and:and:",
            &[one, two, three],
        );
        send(&mut interpreter, &deeper, "touch", &[]);
        let count = slot_count(&mut interpreter, &deeper);
        assert_eq!(
            answered_integer(&mut interpreter, count, "Deeper's declared slot count"),
            3,
            "first, second and third -- not the inherited cache slot"
        );
        for (index, expected) in [(1, 1), (2, 2), (3, 3)] {
            let position = interpreter.stash(Value::small_integer(index));
            let slot = slot_at(&mut interpreter, &deeper, position);
            assert_eq!(
                answered_integer(&mut interpreter, slot, "a declared slot"),
                expected,
                "slot {index} is the declared one, not the hidden slot beside it"
            );
        }
    }

    #[test]
    fn the_reflective_leaves_read_the_slots_and_refuse_an_index_out_of_range() {
        let (mut interpreter, fixture) = a_fixture();
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let pair = send(&mut interpreter, &fixture, "pair:and:", &[one, two]);
        let count = slot_count(&mut interpreter, &pair);
        assert_eq!(answered_integer(&mut interpreter, count, "slotCount"), 2);
        for (index, expected) in [(1, 1), (2, 2)] {
            let argument = interpreter.stash(Value::small_integer(index));
            let slot = slot_at(&mut interpreter, &pair, argument);
            assert_eq!(
                answered_integer(&mut interpreter, slot, "slotAt:"),
                expected
            );
        }
        // 2 ** 32 + 1 wraps to 1 on a 32-bit target unless it is refused.
        for index in [0, 3, (1 << 32) + 1] {
            let argument = interpreter.stash(Value::small_integer(index));
            match slot_at(&mut interpreter, &pair, argument) {
                Err(HostSignal::Error(error)) => {
                    assert_eq!(error.selector, Symbol::intern("slotAt:"));
                    assert!(error.message.contains("out of range"), "{}", error.message);
                }
                _ => panic!("slotAt: {index} must raise an error, not answer"),
            }
        }
        // A LargeInteger is an Integer no slot index reaches: out of range,
        // not "no Integer".
        for negative in [false, true] {
            let argument = interpreter.enter(|mutator| {
                let integer = crate::ArbitraryInteger::from_parts(negative, vec![0, 1]);
                let value = mutator.new_integer(integer);
                mutator.stash(value)
            });
            match slot_at(&mut interpreter, &pair, argument) {
                Err(HostSignal::Error(error)) => {
                    let sign = if negative { "-" } else { "" };
                    assert_eq!(
                        error.message,
                        format!(
                            "index {sign}18446744073709551616 out of range: valid indices are 1..2 (size 2)"
                        )
                    );
                }
                _ => panic!("slotAt: with a LargeInteger must raise an error, not answer"),
            }
        }
        let text = a_string(&mut interpreter, "abc");
        let count = slot_count(&mut interpreter, &text);
        assert_eq!(
            answered_integer(&mut interpreter, count, "a String's slotCount"),
            DECLINED,
            "a String's state is its payload, not slots"
        );
    }

    /// The text `=`'s refusal of an `Escaping` still under construction
    /// carries.
    const ESCAPING_NOT_COMPARED: &str = concat!(
        "cannot compare an instance of Escaping: it is still under construction, ",
        "and an instance of a value class is a value only once its factory has returned"
    );

    /// The text `hash`'s refusal of an `Escaping` still under construction
    /// carries.
    const ESCAPING_NOT_HASHED: &str = concat!(
        "cannot hash an instance of Escaping: it is still under construction, ",
        "and an instance of a value class is a value only once its factory has returned"
    );

    /// The refusal `result` carries: its selector and its text. Panics on
    /// an answer, so a leaf that answers or declines fails the test.
    fn refusal(interpreter: &mut Interpreter, result: HostResult, what: &str) -> (Symbol, String) {
        match result {
            Err(HostSignal::Error(error)) => (error.selector, error.message.to_string()),
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Ok(root) => interpreter.enter(|mutator| {
                let answer = mutator.fetch(&root);
                panic!("{what} answered {}", mutator.heap().describe(answer))
            }),
        }
    }

    /// `=` refuses a receiver whose factory never returned, before it
    /// compares anything: its slots are not all initialized, so it has no
    /// state to be equal by -- and a refusal, never a decline, so
    /// `valueEquals:` never meets it either.
    // spec: semantics.value.under-construction
    // spec: runtime.value.equality
    #[test]
    fn the_equality_leaf_refuses_a_receiver_under_construction() {
        let (mut interpreter, fixture) = a_value_fixture();
        let escaped = send(&mut interpreter, &fixture, "escaped", &[]);
        let one = interpreter.stash(Value::small_integer(1));
        let point = send(&mut interpreter, &fixture, "pointX:y:", &[one.clone(), one]);
        let nil = interpreter.stash(Value::nil());
        for (receiver, argument, what) in [
            (&escaped, &escaped, "escaped = escaped"),
            (&escaped, &point, "escaped = point"),
            (&escaped, &nil, "escaped = nil"),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                receiver,
                std::slice::from_ref(argument),
            );
            let (selector, message) = refusal(&mut interpreter, compared, what);
            assert_eq!(selector, Symbol::EQUALS, "{what}");
            assert_eq!(message, ESCAPING_NOT_COMPARED, "{what}");
        }
    }

    /// An argument whose factory never returned is unequal to a receiver
    /// of a class it cannot be equal to: that answer is fixed whatever its
    /// state, so the leaf gives it, `false`, without reading the state.
    // spec: semantics.value.under-construction
    // spec: runtime.value.equality
    #[test]
    fn the_equality_leaf_answers_an_argument_under_construction_of_another_class() {
        let (mut interpreter, fixture) = a_value_fixture();
        let escaped = send(&mut interpreter, &fixture, "escaped", &[]);
        let one = interpreter.stash(Value::small_integer(1));
        let point = send(&mut interpreter, &fixture, "pointX:y:", &[one.clone(), one]);
        let nil = interpreter.stash(Value::nil());
        let truth = interpreter.stash(Value::boolean(true));
        for (receiver, what) in [
            (&point, "point = escaped"),
            (&nil, "nil = escaped"),
            (&truth, "true = escaped"),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                receiver,
                std::slice::from_ref(&escaped),
            );
            assert_eq!(
                answered_boolean(&mut interpreter, compared, what),
                Some(false),
                "{what}"
            );
        }
    }

    /// `hash` refuses a receiver whose factory never returned: a hash taken
    /// from the slots it has so far would change once the rest were
    /// initialized.
    // spec: semantics.value.under-construction
    #[test]
    fn the_hash_leaf_refuses_a_receiver_under_construction() {
        let (mut interpreter, fixture) = a_value_fixture();
        let escaped = send(&mut interpreter, &fixture, "escaped", &[]);
        let hashed = leaf(&mut interpreter, "valueHash:ifFail:", &escaped, &[]);
        let (selector, message) = refusal(&mut interpreter, hashed, "escaped hash");
        assert_eq!(selector, Symbol::intern("hash"));
        assert_eq!(message, ESCAPING_NOT_HASHED);
    }

    /// A complete value may hold one under construction -- storing it is
    /// allowed -- but neither walk answers past it: both reach it as a slot
    /// and refuse there, rather than comparing or hashing the half-built
    /// state.
    // spec: semantics.value.under-construction
    #[test]
    fn the_walk_refuses_a_value_under_construction_it_reaches() {
        let (mut interpreter, fixture) = a_value_fixture();
        let escaped = send(&mut interpreter, &fixture, "escaped", &[]);
        let first = send(
            &mut interpreter,
            &fixture,
            "boxed:",
            std::slice::from_ref(&escaped),
        );
        let second = send(
            &mut interpreter,
            &fixture,
            "boxed:",
            std::slice::from_ref(&escaped),
        );
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &first,
            std::slice::from_ref(&second),
        );
        let (selector, message) = refusal(&mut interpreter, compared, "boxed = boxed");
        assert_eq!(selector, Symbol::EQUALS);
        assert_eq!(message, ESCAPING_NOT_COMPARED);
        let hashed = leaf(&mut interpreter, "valueHash:ifFail:", &first, &[]);
        let (selector, message) = refusal(&mut interpreter, hashed, "boxed hash");
        assert_eq!(selector, Symbol::intern("hash"));
        assert_eq!(message, ESCAPING_NOT_HASHED);
    }

    /// Among a value's slots, each pair is held to the rule `=` holds its
    /// operands to, the receiver's side being the slot `valueEquals:`
    /// would send `=` to: a slot under construction there refuses, and one
    /// on the argument's side is unequal to a slot of a class it cannot be
    /// equal to.
    // spec: semantics.value.under-construction
    // spec: runtime.value.equality
    #[test]
    fn the_walk_holds_each_pair_of_slots_to_the_rule_of_its_operands() {
        let (mut interpreter, fixture) = a_value_fixture();
        let escaped = send(&mut interpreter, &fixture, "escaped", &[]);
        let nil = interpreter.stash(Value::nil());
        let holding_escaped = send(
            &mut interpreter,
            &fixture,
            "boxed:",
            std::slice::from_ref(&escaped),
        );
        let holding_nil = send(
            &mut interpreter,
            &fixture,
            "boxed:",
            std::slice::from_ref(&nil),
        );
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &holding_nil,
            std::slice::from_ref(&holding_escaped),
        );
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "boxed nil = boxed escaped"),
            Some(false)
        );
        let compared = leaf(
            &mut interpreter,
            "value:sameAs:ifFail:",
            &holding_escaped,
            std::slice::from_ref(&holding_nil),
        );
        let (selector, message) = refusal(&mut interpreter, compared, "boxed escaped = boxed nil");
        assert_eq!(selector, Symbol::EQUALS);
        assert_eq!(message, ESCAPING_NOT_COMPARED);
    }

    /// A slot under construction whose class declares its own `=` is
    /// compared by that `=`, as `valueEquals:` sends it, on either side of
    /// the pair: the leaf declines rather than refuse, and `=` answers
    /// what the `Tag`'s own `=` answers. The `Tag` is marked through the
    /// heap, the state a factory leaves it in until it returns.
    // spec: semantics.value.under-construction
    // spec: runtime.value.equality
    #[test]
    fn a_slot_under_construction_with_its_own_equality_is_compared_by_it() {
        let (mut interpreter, fixture) = a_value_fixture();
        let marked_tag = send(&mut interpreter, &fixture, "tag", &[]);
        let tag = send(&mut interpreter, &fixture, "tag", &[]);
        interpreter.enter(|mutator| {
            let object = mutator.fetch(&marked_tag).as_object().expect("a Tag");
            mutator.heap.mark_under_construction(object);
        });
        let holding_marked = send(
            &mut interpreter,
            &fixture,
            "boxed:",
            std::slice::from_ref(&marked_tag),
        );
        let holding_tag = send(
            &mut interpreter,
            &fixture,
            "boxed:",
            std::slice::from_ref(&tag),
        );
        for (receiver, argument, what) in [
            (
                &holding_marked,
                &holding_tag,
                "boxed marked tag = boxed tag",
            ),
            (
                &holding_tag,
                &holding_marked,
                "boxed tag = boxed marked tag",
            ),
            (
                &holding_marked,
                &holding_marked,
                "boxed marked tag = itself",
            ),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                receiver,
                std::slice::from_ref(argument),
            );
            assert_eq!(
                answered_boolean(&mut interpreter, compared, what),
                None,
                "{what}: the leaf declines"
            );
            let sent = send(
                &mut interpreter,
                receiver,
                "=",
                std::slice::from_ref(argument),
            );
            assert!(answered_flag(&mut interpreter, &sent, what), "{what}");
        }
    }

    /// A value's hash is defined by the first nodes of its breadth-first
    /// unfolding, so values equal by any path hash alike: a ring of one
    /// value and an equal ring of two, a cycle whose slots are
    /// LargeIntegers -- whose sum leaves the SmallInteger range and is
    /// masked into it -- a `true` slot against a `True new` one, whose own
    /// `hash` the walk sends, and a `Hashy`, `hash` its own and `=`
    /// `Value`'s, beside a `Numeral` equal to `3`. Each is hashed by the
    /// leaf where it answers and by `valueHashOfSlots`, the two the same.
    // spec: runtime.value.hash
    #[test]
    fn equal_values_hash_alike_by_their_breadth_first_unfolding() {
        let (mut interpreter, fixture) = a_value_fixture();
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let three = interpreter.stash(Value::small_integer(3));
        let truth = interpreter.stash(Value::boolean(true));
        let true_new = send(&mut interpreter, &fixture, "truth", &[]);
        let hashy = send(&mut interpreter, &fixture, "hashy", &[]);
        let numeral = send(
            &mut interpreter,
            &fixture,
            "numeral:",
            std::slice::from_ref(&three),
        );
        let pairs = [
            (
                "a ring of one and a ring of two",
                send(&mut interpreter, &fixture, "ringOf:", &[one]),
                send(&mut interpreter, &fixture, "ringOf:", &[two]),
            ),
            (
                "a big ring and another",
                send(&mut interpreter, &fixture, "bigRing", &[]),
                send(&mut interpreter, &fixture, "bigRing", &[]),
            ),
            (
                "a box of true and a box of True new",
                send(&mut interpreter, &fixture, "boxed:", &[truth]),
                send(&mut interpreter, &fixture, "boxed:", &[true_new]),
            ),
            (
                "a hashy beside a numeral 3 and beside 3",
                send(
                    &mut interpreter,
                    &fixture,
                    "pointX:y:",
                    &[hashy.clone(), numeral],
                ),
                send(&mut interpreter, &fixture, "pointX:y:", &[hashy, three]),
            ),
        ];
        for (what, first, second) in &pairs {
            let equal = send(&mut interpreter, first, "=", std::slice::from_ref(second));
            assert!(
                answered_flag(&mut interpreter, &equal, what),
                "{what}: equal"
            );
            let hashes = [first, second].map(|subject| {
                let hashed = send(&mut interpreter, subject, "hash", &[]);
                answered_number(&mut interpreter, &hashed, what)
            });
            assert_eq!(hashes[0], hashes[1], "{what}: hash alike");
            for subject in [first, second] {
                let method = interpreter.send_lexically(
                    subject,
                    crate::types::BuiltinClass::Value,
                    Symbol::intern("valueHashOfSlots"),
                    &[],
                );
                let method = answered_number(&mut interpreter, &expect_ok(method, what), what);
                assert_eq!(
                    method, hashes[0],
                    "{what}: valueHashOfSlots answers the same"
                );
            }
        }
    }

    /// A slot whose class declares its own `=` is compared by that `=`
    /// whatever the other slot holds -- `nil`, `3`, a value of another
    /// class -- complete or under construction: `Agreeable>>=` answers
    /// `true` for anything, so the walk declines the pair, before its tower
    /// and its class test could answer `false` by shape, and the send
    /// answers `true`. With the other slot on the receiver's side, the
    /// walk decides the pair itself and answers `false`, an `Agreeable`
    /// under construction included, since the classes cannot be equal.
    // spec: semantics.value.under-construction
    // spec: runtime.value.equality
    #[test]
    fn a_slot_with_its_own_equality_is_compared_by_it_against_any_slot() {
        let (mut interpreter, fixture) = a_value_fixture();
        let complete = send(&mut interpreter, &fixture, "agreeable", &[]);
        let marked = send(&mut interpreter, &fixture, "agreeable", &[]);
        interpreter.enter(|mutator| {
            let object = mutator.fetch(&marked).as_object().expect("an Agreeable");
            mutator.heap.mark_under_construction(object);
        });
        let one = interpreter.stash(Value::small_integer(1));
        let point = send(&mut interpreter, &fixture, "pointX:y:", &[one.clone(), one]);
        let others = [
            ("nil", interpreter.stash(Value::nil())),
            ("3", interpreter.stash(Value::small_integer(3))),
            ("a point", point),
        ];
        for (agreeable, which) in [(&complete, "agreeable"), (&marked, "marked agreeable")] {
            let holding_agreeable = send(
                &mut interpreter,
                &fixture,
                "boxed:",
                std::slice::from_ref(agreeable),
            );
            for (name, other) in &others {
                let holding_other = send(
                    &mut interpreter,
                    &fixture,
                    "boxed:",
                    std::slice::from_ref(other),
                );
                for (receiver, argument, expected, what) in [
                    (
                        &holding_agreeable,
                        &holding_other,
                        true,
                        format!("boxed {which} = boxed {name}"),
                    ),
                    (
                        &holding_other,
                        &holding_agreeable,
                        false,
                        format!("boxed {name} = boxed {which}"),
                    ),
                ] {
                    let compared = leaf(
                        &mut interpreter,
                        "value:sameAs:ifFail:",
                        receiver,
                        std::slice::from_ref(argument),
                    );
                    let leaf_answer = answered_boolean(&mut interpreter, compared, &what);
                    // The walk declines, never answers `false`, where the
                    // receiver's slot brings its own `=`.
                    let expected_leaf = if expected { None } else { Some(false) };
                    assert_eq!(leaf_answer, expected_leaf, "{what}: the leaf");
                    let sent = send(
                        &mut interpreter,
                        receiver,
                        "=",
                        std::slice::from_ref(argument),
                    );
                    assert_eq!(
                        answered_flag(&mut interpreter, &sent, &what),
                        expected,
                        "{what}"
                    );
                }
            }
        }
    }

    /// A slot under construction whose class declares its own `hash` is
    /// hashed by that `hash`, as `valueHashOfSlots` sends it: the leaf
    /// declines rather than refuse, and `hash` answers the slot count plus
    /// the `Tag`'s own 7, as for a complete `Tag`.
    // spec: semantics.value.under-construction
    // spec: runtime.value.hash
    #[test]
    fn a_slot_under_construction_with_its_own_hash_is_hashed_by_it() {
        let (mut interpreter, fixture) = a_value_fixture();
        let marked_tag = send(&mut interpreter, &fixture, "tag", &[]);
        interpreter.enter(|mutator| {
            let object = mutator.fetch(&marked_tag).as_object().expect("a Tag");
            mutator.heap.mark_under_construction(object);
        });
        let holding_marked = send(
            &mut interpreter,
            &fixture,
            "boxed:",
            std::slice::from_ref(&marked_tag),
        );
        let hashed = leaf(&mut interpreter, "valueHash:ifFail:", &holding_marked, &[]);
        assert_eq!(
            answered_integer(&mut interpreter, hashed, "boxed marked tag hash"),
            DECLINED
        );
        let sent = send(&mut interpreter, &holding_marked, "hash", &[]);
        assert_eq!(
            answered_number(&mut interpreter, &sent, "boxed marked tag hash"),
            1 + 7
        );
    }

    /// `nil`, `true` and `false` are answered by identity before the walk,
    /// so that `nil == anObject`, the commonest pair the leaf sees, never
    /// pays for the walk's tests: held here to what the walk itself
    /// answers for each of them against every shape of argument -- the
    /// three constants, numbers immediate and boxed, a string, an array,
    /// an object that is no value, a value, a class object, a second
    /// instance of each constant's class, an instance of a program's
    /// subclass of `Boolean` and instances under construction -- and to
    /// answering nothing for any other receiver.
    ///
    /// The kernel's initializers of `UndefinedObject`, `True`, `False` and
    /// `Boolean` run no program code, so a program never sees one of their
    /// instances under construction; a debugger stopped in one can. Such
    /// an instance is made here by setting a complete one's mark through
    /// the heap, which is the state its factory leaves it in until it
    /// returns. Against it the walk refuses a constant of its own class,
    /// and the fast path leaves the pair to the walk, so `nil = it`
    /// refuses as the walk does; a complete one it leaves to the walk too,
    /// which finds it equal, its state being no slots, as the constant's.
    // spec: runtime.value.equality
    // spec: semantics.value.under-construction
    #[test]
    fn the_constants_are_answered_by_identity_as_the_walk_answers_them() {
        let (mut interpreter, fixture) = a_value_fixture();
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let point = send(&mut interpreter, &fixture, "pointX:y:", &[one, two]);
        let point_class = class_of(&mut interpreter, &point);
        let escaped = send(&mut interpreter, &fixture, "escaped", &[]);
        let string = a_string(&mut interpreter, "nil");
        let array = an_array(&mut interpreter);
        let instances = ["undefinedObject", "truth", "falsity", "myBoolean"];
        let complete = instances.map(|selector| send(&mut interpreter, &fixture, selector, &[]));
        let marked = instances.map(|selector| send(&mut interpreter, &fixture, selector, &[]));
        interpreter.enter(|mutator| {
            for root in &marked {
                let object = mutator.fetch(root).as_object().expect("a heap instance");
                mutator.heap.mark_under_construction(object);
            }
        });
        let large = send(&mut interpreter, &fixture, "large", &[]);
        let negative_large = send(&mut interpreter, &fixture, "negativeLarge", &[]);
        let boxed_float = send(&mut interpreter, &fixture, "boxedFloat", &[]);
        interpreter.enter(|mutator| {
            for number in [&large, &negative_large, &boxed_float] {
                let number = mutator.fetch(number);
                assert!(
                    number.as_object().is_some(),
                    "{} is an object",
                    mutator.heap().describe(number)
                );
            }
        });
        let mut arguments: Vec<Root> = [
            Value::nil(),
            Value::boolean(true),
            Value::boolean(false),
            Value::small_integer(0),
            Value::small_float(0.0),
        ]
        .into_iter()
        .map(|value| interpreter.stash(value))
        .collect();
        arguments.extend([string, array, fixture, point, point_class, escaped.clone()]);
        arguments.extend(complete);
        arguments.extend(marked.iter().cloned());
        arguments.extend([large, negative_large, boxed_float]);
        interpreter.enter(|mutator| {
            for receiver in [Value::nil(), Value::boolean(true), Value::boolean(false)] {
                for argument in &arguments {
                    let other = mutator.fetch(argument);
                    let what = format!(
                        "{} = {}",
                        mutator.heap().describe(receiver),
                        mutator.heap().describe(other)
                    );
                    let walked = derived_equality(mutator, receiver, other);
                    match constant_identity(mutator, receiver, other) {
                        Some(identical) => assert!(
                            matches!(walked, Ok(Some(equal)) if equal == identical),
                            "{what}: the walk answers otherwise"
                        ),
                        // Left to the walk only for a heap instance of the
                        // constant's own class, never for the common pair.
                        None => assert!(
                            other.as_object().is_some_and(|object| mutator
                                .heap()
                                .object_class(object)
                                .same(mutator.class_of(receiver))),
                            "{what}: left to the walk"
                        ),
                    }
                }
            }
            for argument in &arguments[3..] {
                let receiver = mutator.fetch(argument);
                assert_eq!(
                    constant_identity(mutator, receiver, Value::nil()),
                    None,
                    "{} is no constant",
                    mutator.heap().describe(receiver)
                );
            }
        });
        // Through the whole leaf: a constant against an instance of its own
        // class under construction refuses it, and against one of a class
        // that cannot be equal to its own -- `escaped`, `myBoolean` --
        // answers `false`.
        let [marked_nil, marked_true, marked_false, marked_boolean] = marked;
        let nil = interpreter.stash(Value::nil());
        let truth = interpreter.stash(Value::boolean(true));
        let falsity = interpreter.stash(Value::boolean(false));
        for (receiver, argument, what) in [
            (
                &nil,
                &marked_nil,
                "nil = UndefinedObject new, under construction",
            ),
            (&truth, &marked_true, "true = True new, under construction"),
            (
                &falsity,
                &marked_false,
                "false = False new, under construction",
            ),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                receiver,
                std::slice::from_ref(argument),
            );
            let (selector, _) = refusal(&mut interpreter, compared, what);
            assert_eq!(selector, Symbol::EQUALS, "{what}");
        }
        for (receiver, argument, what) in [
            (&nil, &escaped, "nil = escaped"),
            (
                &truth,
                &marked_boolean,
                "true = MyBoolean new, under construction",
            ),
            (
                &truth,
                &marked_false,
                "true = False new, under construction",
            ),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                receiver,
                std::slice::from_ref(argument),
            );
            assert_eq!(
                answered_boolean(&mut interpreter, compared, what),
                Some(false),
                "{what}"
            );
        }
    }

    /// What a leaf or the method it stands for did with one pair: answered
    /// (described), declined, refused an instance under construction, or
    /// failed otherwise -- each as text, since a `Value` has no `Debug`.
    #[derive(Debug, PartialEq, Eq)]
    enum Outcome {
        Answered(String),
        Declined,
        Refused(String),
        Failed(String),
    }

    fn outcome_of(interpreter: &mut Interpreter, result: HostResult, leaf: bool) -> Outcome {
        match result {
            Ok(root) => interpreter.enter(|mutator| {
                let answer = mutator.fetch(&root);
                if leaf && answer.as_small_integer() == Some(DECLINED) {
                    Outcome::Declined
                } else {
                    Outcome::Answered(mutator.heap().describe(answer))
                }
            }),
            Err(HostSignal::Error(error)) if error.message.contains("still under construction") => {
                Outcome::Refused(error.message.to_string())
            }
            Err(HostSignal::Error(error)) => Outcome::Failed(error.message.to_string()),
            Err(HostSignal::Return) => panic!("a non-local return escaped"),
        }
    }

    /// Every shape the walks meet, each a root and the contents of a box:
    /// immediates, boxed numbers, a Fraction, strings and a Symbol, class
    /// objects -- one whose class side declares `==` among them --,
    /// instances of `Value>>=` classes, values with their own `=` and
    /// `hash`, an ordinary object, a value whose state reaches itself
    /// through a class, nested values, and each instance also under
    /// construction. Each entry names whether its root is marked.
    fn every_shape(interpreter: &mut Interpreter, fixture: &Root) -> Vec<(String, Root, bool)> {
        let mut shapes: Vec<(String, Root, bool)> = Vec::new();
        let immediates = [
            ("nil", Value::nil()),
            ("true", Value::boolean(true)),
            ("false", Value::boolean(false)),
            ("0", Value::small_integer(0)),
            ("3", Value::small_integer(3)),
            ("0.0", Value::small_float(0.0)),
        ];
        for (name, value) in immediates {
            shapes.push((name.to_string(), interpreter.stash(value), false));
        }
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let built: Vec<(&str, &str, Vec<Root>)> = vec![
            ("large", "large", vec![]),
            ("negative large", "negativeLarge", vec![]),
            ("boxed float", "boxedFloat", vec![]),
            ("a half", "half", vec![]),
            ("a symbol", "symbol", vec![]),
            ("a point 1@2", "pointX:y:", vec![one.clone(), two.clone()]),
            (
                "another point 1@2",
                "pointX:y:",
                vec![one.clone(), two.clone()],
            ),
            ("a point 1@1", "pointX:y:", vec![one.clone(), one.clone()]),
            ("a tag", "tag", vec![]),
            ("an agreeable", "agreeable", vec![]),
            ("a plain object", "plain", vec![]),
            ("a loose 1", "loose:", vec![one.clone()]),
            ("an outer 1", "outer:", vec![one.clone()]),
            ("a looped 1", "looped:", vec![one.clone()]),
            ("another looped 1", "looped:", vec![one.clone()]),
            ("a my boolean", "myBoolean", vec![]),
            ("an undefined object", "undefinedObject", vec![]),
            (
                "a numeral 3",
                "numeral:",
                vec![interpreter.stash(Value::small_integer(3))],
            ),
            ("a True new", "truth", vec![]),
            ("a False new", "falsity", vec![]),
            ("a hashy", "hashy", vec![]),
            ("a hash side", "hashSide", vec![]),
            ("an equals side", "equalsSide", vec![]),
            ("a ring of one", "ringOf:", vec![one.clone()]),
            ("a ring of two", "ringOf:", vec![two.clone()]),
            ("a big ring", "bigRing", vec![]),
            (
                "a chain of 2100",
                "chainOf:",
                vec![interpreter.stash(Value::small_integer(2100))],
            ),
        ];
        let mut instances = Vec::new();
        for (name, selector, arguments) in built {
            let root = send(interpreter, fixture, selector, &arguments);
            instances.push((name.to_string(), root));
        }
        let strings = [
            ("'abc'", a_string(interpreter, "abc")),
            ("another 'abc'", a_string(interpreter, "abc")),
        ];
        for (name, root) in strings {
            shapes.push((name.to_string(), root, false));
        }
        for (name, root) in &instances {
            shapes.push((name.clone(), root.clone(), false));
        }
        let point_class = class_of(interpreter, &instances[5].1);
        shapes.push(("the Point class".to_string(), point_class, false));
        let loose_class = class_of(interpreter, &instances[11].1);
        shapes.push(("the Loose class".to_string(), loose_class, false));
        // Class sides with only their own `hash`, and with only their own
        // `=`.
        for (name, instance) in ["a hash side", "an equals side"]
            .map(|name| (name, instances.iter().find(|(found, _)| found == name)))
        {
            let (_, instance) = instance.expect("the instance is built above");
            let class = class_of(interpreter, instance);
            shapes.push((format!("the class of {name}"), class, false));
        }
        // Under construction: a second instance of each class with slots
        // or its own `=`, marked through the heap as its factory leaves it.
        for (name, selector, arguments) in [
            (
                "a marked point 1@2",
                "pointX:y:",
                vec![one.clone(), two.clone()],
            ),
            ("a marked tag", "tag", vec![]),
            ("a marked agreeable", "agreeable", vec![]),
            ("a marked loose 1", "loose:", vec![one.clone()]),
            ("a marked my boolean", "myBoolean", vec![]),
        ] {
            let root = send(interpreter, fixture, selector, &arguments);
            interpreter.enter(|mutator| {
                let object = mutator.fetch(&root).as_object().expect("an instance");
                mutator.heap.mark_under_construction(object);
            });
            shapes.push((name.to_string(), root, true));
        }
        let escaped = send(interpreter, fixture, "escaped", &[]);
        shapes.push(("an escaped".to_string(), escaped, true));
        // Two slots, the first unequal and the second under construction:
        // which of the two the comparison meets first is its order's.
        let marked_point = shapes
            .iter()
            .find(|(name, ..)| name == "a marked point 1@2")
            .map(|(_, root, _)| root.clone())
            .expect("a marked point");
        for (name, x_value) in [
            ("a point 1@marked", one.clone()),
            ("a point 2@marked", two.clone()),
        ] {
            let root = send(
                interpreter,
                fixture,
                "pointX:y:",
                &[x_value, marked_point.clone()],
            );
            shapes.push((name.to_string(), root, false));
        }
        // Each shape a slot, and a box of a box of a point.
        let mut boxes = Vec::new();
        for (name, root, _) in &shapes {
            let boxed = interpreter.send(
                fixture,
                Symbol::intern("boxed:"),
                std::slice::from_ref(root),
            );
            // A slot must hold a value: an ordinary object is refused.
            if let Ok(boxed) = boxed {
                boxes.push((format!("a box of {name}"), boxed, false));
            }
        }
        let deeper = send(
            interpreter,
            fixture,
            "boxed:",
            std::slice::from_ref(&boxes[0].1),
        );
        boxes.push(("a box of a box of nil".to_string(), deeper, false));
        shapes.extend(boxes);
        shapes
    }

    /// The leaves are a second implementation of the methods they stand
    /// for: `Value>>=`'s of `valueEquals:`, `Behavior>>=`'s of
    /// `behaviorEquals:`, `Value>>hash`'s of `valueHashOfSlots`. Held here
    /// to them over every pair of every shape (`every_shape`), each pair
    /// run through the leaf and through the method, sent lexically as the
    /// leaf's failure block sends it:
    ///
    /// - where the leaf answers, the method answers the same;
    /// - where the leaf refuses, the method refuses the same object -- but
    ///   where an operand itself is under construction, whose state the
    ///   method would read directly where the leaf refuses to;
    /// - where the leaf declines, the method is the answer by construction;
    /// - a hash leaf answers the very number `valueHashOfSlots` answers,
    ///   or refuses where it does, and two values `=` finds equal hash
    ///   alike.
    // spec: runtime.value.equality
    // spec: runtime.value.hash
    // spec: semantics.value.under-construction
    #[test]
    fn the_leaves_answer_as_the_methods_they_stand_for_over_every_shape() {
        let (mut interpreter, fixture) = a_value_fixture();
        let shapes = every_shape(&mut interpreter, &fixture);
        // Which method each receiver's `=` and `hash` are, by identity.
        let (equality, hash) = interpreter.enter(|mutator| {
            let lookup = |root: &Root, selector: &str| {
                let class = mutator.class_of(mutator.fetch(root));
                match crate::dispatch::lookup_public(
                    mutator.heap(),
                    class,
                    Symbol::intern(selector),
                ) {
                    crate::dispatch::Lookup::Method(method, _) => Some(method),
                    _ => None,
                }
            };
            let value_class = mutator.builtin_class(crate::types::BuiltinClass::Value);
            let value_mixin = mutator
                .heap()
                .class_application(value_class)
                .expect("Value's application")
                .mixin
                .clone();
            let class_class = mutator.builtin_class(crate::types::BuiltinClass::Class);
            let behavior_equality =
                match crate::dispatch::lookup_public(mutator.heap(), class_class, Symbol::EQUALS) {
                    crate::dispatch::Lookup::Method(method, _) => method,
                    _ => panic!("Behavior declares ="),
                };
            let values_equality = value_mixin
                .compiled_method(Symbol::EQUALS)
                .expect("=")
                .clone();
            let values_hash = value_mixin
                .compiled_method(Symbol::intern("hash"))
                .expect("hash")
                .clone();
            let equality: Vec<Option<crate::types::BuiltinClass>> = shapes
                .iter()
                .map(|(_, root, _)| {
                    let found = lookup(root, "=")?;
                    if std::rc::Rc::ptr_eq(&found, &values_equality) {
                        Some(crate::types::BuiltinClass::Value)
                    } else if std::rc::Rc::ptr_eq(&found, &behavior_equality) {
                        Some(crate::types::BuiltinClass::Behavior)
                    } else {
                        None
                    }
                })
                .collect();
            let hash: Vec<bool> = shapes
                .iter()
                .map(|(_, root, _)| {
                    lookup(root, "hash")
                        .is_some_and(|found| std::rc::Rc::ptr_eq(&found, &values_hash))
                })
                .collect();
            (equality, hash)
        });
        let mut compared = 0;
        let mut declined = 0;
        for (receiver_index, (receiver_name, receiver, receiver_marked)) in
            shapes.iter().enumerate()
        {
            let Some(lexical_class) = equality[receiver_index] else {
                continue;
            };
            let fallback = match lexical_class {
                crate::types::BuiltinClass::Value => "valueEquals:",
                _ => "behaviorEquals:",
            };
            for (argument_name, argument, argument_marked) in &shapes {
                let what = format!("{receiver_name} = {argument_name}");
                let leaf_result = leaf(
                    &mut interpreter,
                    "value:sameAs:ifFail:",
                    receiver,
                    std::slice::from_ref(argument),
                );
                let leaf_outcome = outcome_of(&mut interpreter, leaf_result, true);
                if leaf_outcome == Outcome::Declined {
                    declined += 1;
                    continue;
                }
                if matches!(leaf_outcome, Outcome::Refused(_))
                    && (*receiver_marked || *argument_marked)
                {
                    continue;
                }
                let method_result = interpreter.send_lexically(
                    receiver,
                    lexical_class,
                    Symbol::intern(fallback),
                    std::slice::from_ref(argument),
                );
                let method_outcome = outcome_of(&mut interpreter, method_result, false);
                assert_eq!(leaf_outcome, method_outcome, "{what}");
                compared += 1;
            }
        }
        for (index, (name, root, marked)) in shapes.iter().enumerate() {
            if !hash[index] || *marked {
                continue;
            }
            let leaf_result = leaf(&mut interpreter, "valueHash:ifFail:", root, &[]);
            let leaf_outcome = outcome_of(&mut interpreter, leaf_result, true);
            if leaf_outcome == Outcome::Declined {
                continue;
            }
            let method_result = interpreter.send_lexically(
                root,
                crate::types::BuiltinClass::Value,
                Symbol::intern("valueHashOfSlots"),
                &[],
            );
            let method_outcome = outcome_of(&mut interpreter, method_result, false);
            assert_eq!(leaf_outcome, method_outcome, "{name} hash");
        }
        // Two values `=` finds equal hash alike, whichever path answered --
        // but where a class's own `=` and `hash` decide, which keep that
        // contract only as far as the program keeps it: `Agreeable`'s `=`
        // answers `true` for anything, its `hash` 7, and so does the
        // class side of `EqualsSide`, its `hash` its name's.
        let own_equality = |name: &str| {
            name.contains("tag")
                || name.contains("agreeable")
                || name.contains("the class of an equals side")
        };
        for (receiver_name, receiver, receiver_marked) in &shapes {
            for (argument_name, argument, argument_marked) in &shapes {
                if *receiver_marked
                    || *argument_marked
                    || own_equality(receiver_name)
                    || own_equality(argument_name)
                {
                    continue;
                }
                let Ok(equal) =
                    interpreter.send(receiver, Symbol::EQUALS, std::slice::from_ref(argument))
                else {
                    continue;
                };
                let equal = interpreter.enter(|mutator| mutator.fetch(&equal).as_boolean());
                if equal != Some(true) {
                    continue;
                }
                let hashes = [receiver, argument].map(|subject| {
                    let hashed = interpreter.send(subject, Symbol::intern("hash"), &[]);
                    outcome_of(&mut interpreter, hashed, false)
                });
                if matches!(hashes[0], Outcome::Answered(_)) {
                    assert_eq!(
                        hashes[0], hashes[1],
                        "{receiver_name} = {argument_name}, so their hashes agree"
                    );
                }
            }
        }
        assert!(
            compared > 1000,
            "{compared} pairs compared, {declined} declined"
        );
    }

    /// The mark is a header bit, and every mover copies or slides the
    /// header whole: a cut-short construction stays marked, and a complete
    /// value unmarked, through a scavenge into a survivor space, a tenure
    /// into the old space and a full collection's compaction.
    // `collect_now` and `scavenge_now` are a debug build's.
    #[cfg(debug_assertions)]
    // spec: vm.heap.under-construction-only-on-value-instances
    #[test]
    fn a_construction_mark_survives_a_scavenge_and_a_full_collection() {
        let (mut interpreter, fixture) = a_value_fixture();
        let escaped = send(&mut interpreter, &fixture, "escaped", &[]);
        let one = interpreter.stash(Value::small_integer(1));
        let point = send(&mut interpreter, &fixture, "pointX:y:", &[one.clone(), one]);
        let marks = |interpreter: &mut Interpreter| {
            interpreter.enter(|mutator| {
                [&escaped, &point].map(|root| {
                    let object = mutator.fetch(root).as_object().expect("an instance");
                    mutator.heap().object_under_construction(object)
                })
            })
        };
        assert_eq!(
            marks(&mut interpreter),
            [true, false],
            "before any collection"
        );
        interpreter.scavenge_now();
        assert_eq!(marks(&mut interpreter), [true, false], "after a scavenge");
        interpreter.tenure_now();
        assert_eq!(marks(&mut interpreter), [true, false], "after a tenure");
        interpreter.collect_now();
        assert_eq!(
            marks(&mut interpreter),
            [true, false],
            "after a full collection"
        );
    }
}
