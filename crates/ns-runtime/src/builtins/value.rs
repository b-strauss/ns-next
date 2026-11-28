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
//! decoded value reaches is the image's to choose, so both walks run an
//! explicit work stack; the equality walk remembers the pairs it has
//! already assumed equal -- instances and class objects alike -- so a
//! cycle, through slots or through classes, terminates in `true` rather than
//! running forever, and both stop once `NODE_BUDGET` nodes have gone *onto*
//! that stack, so neither runs for as long as the input likes and neither
//! stack grows past the budget.
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

use crate::interpreter::Arguments;
use std::collections::HashSet;
use std::rc::Rc;

use crate::builtins::collections::{self, IntegerArgument};
use crate::builtins::first_argument;
use crate::builtins::numbers::tower;
use crate::builtins::primitive::{Primitive, ValuePrimitive};
use crate::builtins::text;
use crate::dispatch::{self, Lookup};
use crate::interpreter::instantiate::{class_object_is_a_value, under_construction_error};
use crate::types::*;
use crate::{Mutator, Symbol};

/// How many nodes either walk puts on its work stack before it stops
/// unfolding -- the root included.
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
/// It is also what keeps a truncated hash *consistent*: two equal values
/// unfold to the same nodes in the same order and have the same slot
/// counts at each of them, so both are charged alike and both stop at the
/// same node. Every node contributes a hash below 2^32 plus a slot count
/// below 2^16, so a sum of this many stays far inside an `i64`.
const NODE_BUDGET: usize = 4096;

/// `nil`'s hash. `UndefinedObject` is a singleton below `Value`, so any constant will
/// do; 0 is the one `Object`'s identity hash already answered for it.
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
            // A refusal, never a decline: `valueEquals:` would compare the
            // slots built so far, which is the answer the refusal withholds.
            let equal = match derived_equality(mutator, receiver, other) {
                Ok(equal) => equal?,
                Err(object) => {
                    return Some(Err(under_construction_error(
                        mutator, selector, receiver, object,
                    )));
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
                mutator, selector, receiver, object,
            ))),
        },
        ValuePrimitive::SlotCount => {
            let count = with_declared_slots(mutator, receiver, <[usize]>::len)?;
            Some(Ok(Value::small_integer(count as i64)))
        }
        ValuePrimitive::SlotAt => {
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

/// Runs `read` over the field indices of the slots `receiver`'s classes
/// **declare**, when it is an ordinary instance -- the only shape whose
/// state is slots at all. `None` for anything else, which is the decline.
///
/// **The one place this question is answered**, for the same reason
/// `state_is_a_value` is one function: the equality walk, the hash walk and
/// the two leaves the Newspeak fallback reads a slot through
/// (`valueSlotCount:ifFail:`, `value:slotAt:ifFail:`) must agree on which
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

/// What a walk found at one node while hashing, the same two ways.
enum Node<'gc> {
    Leaf(i64),
    State(ObjectReference<'gc>),
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
/// **The one predicate both walks ask**, and deliberately one function: if
/// the equality walk and the hash walk ever decided "this is a value I can
/// unfold" differently, two instances `=` answered `true` for could take
/// the declining path on one side of `hash` only and hash apart -- the one
/// thing §3.1.1 does not allow. `the_equality_and_the_hash_decline_on_the_same_shapes`
/// pins that they agree, shape by shape.
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
fn compare_node<'gc>(
    mutator: &Mutator<'gc>,
    first: Value<'gc>,
    second: Value<'gc>,
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
            state_is_a_value(mutator, left).then_some(Pair::State(left, right))
        }
        // One application, two shapes: nothing builds such a pair today,
        // and answering "unequal" is the reading that cannot claim an
        // equality it did not check.
        _ => Some(Pair::Decided(false)),
    }
}

/// One node of the hash walk: `None` declines the whole hash.
fn hash_node<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> Option<Node<'gc>> {
    // Every number's slot hash is its numeric hash folded by
    // `content_hash`: equal numbers in two slots -- `3` and `3.0` -- hash
    // alike, as `compare_node` makes them equal, and the fold keeps a
    // slot's hash below 2^32, so the sum over a value's slots stays inside
    // an `i64`.
    let number = || {
        tower::value_hash(mutator.heap(), value)
            .map(|hash| Node::Leaf(crate::hash::content_hash(hash)))
    };
    match value.kind() {
        ValueKind::Nil => Some(Node::Leaf(NIL_HASH)),
        // `True>>hash` and `False>>hash`, so a boolean slot hashes as the
        // boolean itself does.
        ValueKind::Boolean(flag) => Some(Node::Leaf(i64::from(flag))),
        ValueKind::SmallInteger(_) | ValueKind::SmallFloat(_) => number(),
        ValueKind::Object(object) => {
            if let Some(leaf) = number() {
                return Some(leaf);
            }
            // A value class object hashes as its name: two equal ones share
            // an application, hence a mixin, hence a name, so this is a
            // valid hash, and unlike one that folded in the enclosing
            // object's it never unfolds a builtin class's enclosing
            // `Kernel`, whose `vmMirror` slot the walk declines. The name,
            // not a dispatch id, because a name survives a copy and an
            // image round trip. A class object that is no value declines,
            // and `Behavior>>hash` answers its identity hash -- as its `=`
            // is identity.
            if mutator.heap().is_class(object) {
                return class_object_is_a_value(mutator, object).then(|| {
                    Node::Leaf(text::string_hash(
                        &mutator.heap().class_display_name(object),
                    ))
                });
            }
            if let Some(hash) = text::symbol_hash(mutator.heap(), object) {
                return Some(Node::Leaf(hash));
            }
            if let Some(hash) = mutator.heap().string_hash(object) {
                return Some(Node::Leaf(hash));
            }
            state_is_a_value(mutator, object).then_some(Node::State(object))
        }
    }
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
/// and `Err` the instance under construction the walk reached, which
/// refuses the comparison whatever the other nodes would have answered.
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
    // first step decides -- `nil = anObject`, two numbers, two values of
    // unrelated classes -- allocates nothing: an empty `Vec` allocates
    // only at its first push, and this leaf runs for every `=` that
    // reaches it: allocating the stack up front cost Richards 15 % of its
    // time on Windows' heap. The set of assumed pairs is made at its first
    // insert for the same reason: a `HashSet::new()` reads its thread's
    // random keys, a thread-local access and a 16-byte copy on every `=`,
    // which a pair its first step decides never needs.
    let mut root = Some((left, right));
    let mut work = Vec::new();
    let mut assumed: Option<HashSet<(usize, usize)>> = None;
    // The root pair is charged like a pushed one.
    let mut charged = 1usize;
    while let Some((first, second)) = root.take().or_else(|| work.pop()) {
        // Before anything else, the root pair included: an instance whose
        // factory has not returned has no state yet, so no verdict --
        // not even one the tower or the classes would reach without its
        // slots -- may be answered for a pair it is part of (§3.1.1, "have
        // been initialized").
        if let Some(object) =
            under_construction(mutator, first).or_else(|| under_construction(mutator, second))
        {
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
        // The second side is read only when the first is a number: `nil ==
        // anObject`, the common pair, then never asks the heap whether
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
        let Some(pair) = compare_node(mutator, first, second) else {
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
                work.extend(pairs);
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
                work.extend(first_slots.into_iter().zip(second_slots));
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

/// The hash `value`'s state gives it; `None` is the decline, and `Err`
/// the instance under construction the walk reached. Every node reached
/// contributes its own hash, and every instance its slot count, as the sum
/// the answer is.
fn derived_hash<'gc>(
    mutator: &Mutator<'gc>,
    value: Value<'gc>,
) -> Result<Option<i64>, ObjectReference<'gc>> {
    let mut work = vec![value];
    let mut total: i64 = 0;
    // The root is on the stack already.
    let mut charged = 1usize;
    while let Some(current) = work.pop() {
        // The root first: a hash taken from the slots an instance under
        // construction has so far would change once the rest were
        // initialized, and a hash cut off there would be no hash of the
        // value it becomes.
        if let Some(object) = under_construction(mutator, current) {
            return Err(object);
        }
        let Some(node) = hash_node(mutator, current) else {
            return Ok(None);
        };
        match node {
            Node::Leaf(hash) => total += hash,
            Node::State(object) => {
                let slots = slots_of(mutator, object);
                total += slots.len() as i64;
                // Unlike the comparison, a truncated hash is still a valid
                // hash: two equal values unfold in the same order with the
                // same slot counts, so both are cut at the same node and
                // both answer the same number. Cut before the slots are
                // pushed, which is what bounds the stack by the budget.
                if slots.len() > NODE_BUDGET - charged {
                    break;
                }
                charged += slots.len();
                // Pushed back to front, so the stack unfolds the slots in
                // declaration order: the sum does not care, but the node
                // budget cuts two equal values at the same slot only if
                // both are walked the same way.
                for slot in slots.into_iter().rev() {
                    work.push(slot);
                }
            }
        }
    }
    Ok(Some(total))
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

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
                public pointX: xValue y: yValue = ( ^Point x: xValue y: yValue )
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

    /// `hash_node` hashes an `Integer` slot with `hash::content_hash` of
    /// its numeric hash while `Integer>>hash`, which the Newspeak fallback
    /// sums, answers the integer itself. The two are allowed to differ only
    /// because a value the leaf hashes and a value the fallback hashes can
    /// never be equal: the leaf's decision is a question about the slots'
    /// classes, and two values of one class whose slots' classes differ are
    /// unequal slot by slot -- a pair of numbers apart, which are both
    /// shapes the leaf hashes, alike when they are equal.
    ///
    /// Both shapes of one class are built here -- a `Boxed` holding an
    /// `Integer`, which the leaf hashes, and a `Boxed` holding a `Tag`,
    /// whose own `=` takes the leaf out of the decision -- and pinned
    /// unequal in both directions, each shape hashing consistently within
    /// itself. A change that let the leaf decide for one of an equal pair
    /// would fail here rather than silently making two equal values hash
    /// apart.
    #[test]
    fn a_leaf_hashed_value_and_a_fallback_hashed_value_are_never_equal() {
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

        for (receiver, argument) in [(&decided, &declined), (&declined, &decided)] {
            let equal = send(
                &mut interpreter,
                receiver,
                "=",
                std::slice::from_ref(argument),
            );
            assert!(
                !answered_flag(
                    &mut interpreter,
                    &equal,
                    "a leaf-hashed against a fallback-hashed"
                ),
                "the two hashing rules never have to agree, because the two values are unequal"
            );
        }

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

    /// The two derivations decline on exactly the same shapes. They ask one
    /// predicate about an instance -- `state_is_a_value`, which is one
    /// function for this reason -- and each shape's own rule decides the
    /// rest. If the two ever diverged, a pair `=` answers `true` for could
    /// reach `Value`'s Newspeak walk on one side of `hash` only, and two
    /// equal values would hash apart: exactly the contract
    /// `goldens/library/value/value_hashes_follow_value_equality.ns` asserts, and
    /// one a change to either walk alone could break silently.
    #[test]
    fn the_equality_and_the_hash_decline_on_the_same_shapes() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let a_class = interpreter.enter(|mutator| {
            let class = mutator.builtin_class(crate::BuiltinClass::String);
            mutator.stash(Value::object(class))
        });
        let subjects: Vec<(&str, Root)> = vec![
            (
                "an instance whose = is the recorded one",
                send(&mut interpreter, &fixture, "pair:and:", &[one.clone(), two]),
            ),
            (
                "an instance with no slots",
                send(&mut interpreter, &fixture, "marker", &[]),
            ),
            (
                "an instance that overrides =",
                send(&mut interpreter, &fixture, "overriding", &[]),
            ),
            (
                "an instance that reaches itself",
                send(&mut interpreter, &fixture, "cyclic", &[]),
            ),
            ("a String", a_string(&mut interpreter, "abc")),
            ("an Array", an_array(&mut interpreter)),
            ("an instance whose slot holds an Array", {
                let array = an_array(&mut interpreter);
                send(
                    &mut interpreter,
                    &fixture,
                    "pair:and:",
                    &[one.clone(), array],
                )
            }),
            // A builtin class: enclosed by the `Kernel`, which is a value,
            // so a value class object, which both leaves decide. A class
            // object that is no value is the one shape where they part --
            // the equality answers identity and the hash declines to the
            // identity hash -- which
            // `a_class_object_that_is_no_value_compares_by_identity` pins.
            ("a value class object", a_class),
            ("an Integer", one),
            ("nil", interpreter.stash(Value::nil())),
            ("a Boolean", interpreter.stash(Value::boolean(true))),
            (
                "a Float",
                interpreter.enter(|mutator| {
                    let float = mutator.new_float(1.5);
                    mutator.stash(float)
                }),
            ),
        ];
        let mut declined = 0;
        let mut answered = 0;
        for (what, subject) in subjects {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                &subject,
                std::slice::from_ref(&subject),
            );
            let equality_declined = answered_boolean(&mut interpreter, compared, what).is_none();
            let hashed = leaf(&mut interpreter, "valueHash:ifFail:", &subject, &[]);
            let hash_declined = answered_integer(&mut interpreter, hashed, what) == DECLINED;
            assert_eq!(
                equality_declined, hash_declined,
                "{what}: the equality leaf and the hash leaf must decline together"
            );
            if equality_declined {
                declined += 1;
            } else {
                answered += 1;
            }
        }
        // Without this the test would pass just as well if every shape
        // declined, or none did -- agreement is only worth pinning over a
        // list that reaches both answers.
        assert!(
            declined >= 3 && answered >= 6,
            "the list must exercise both outcomes: {declined} declined, {answered} answered"
        );
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
            let count = leaf(&mut interpreter, "valueSlotCount:ifFail:", &first, &[]);
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
            let count = leaf(&mut interpreter, "valueSlotCount:ifFail:", subject, &[]);
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
        let count = leaf(&mut interpreter, "valueSlotCount:ifFail:", &deeper, &[]);
        assert_eq!(
            answered_integer(&mut interpreter, count, "Deeper's declared slot count"),
            3,
            "first, second and third -- not the inherited cache slot"
        );
        for (index, expected) in [(1, 1), (2, 2), (3, 3)] {
            let position = interpreter.stash(Value::small_integer(index));
            let slot = leaf(
                &mut interpreter,
                "value:slotAt:ifFail:",
                &deeper,
                &[position],
            );
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
        let count = leaf(&mut interpreter, "valueSlotCount:ifFail:", &pair, &[]);
        assert_eq!(answered_integer(&mut interpreter, count, "slotCount"), 2);
        for (index, expected) in [(1, 1), (2, 2)] {
            let argument = interpreter.stash(Value::small_integer(index));
            let slot = leaf(&mut interpreter, "value:slotAt:ifFail:", &pair, &[argument]);
            assert_eq!(
                answered_integer(&mut interpreter, slot, "slotAt:"),
                expected
            );
        }
        // 2 ** 32 + 1 wraps to 1 on a 32-bit target unless it is refused.
        for index in [0, 3, (1 << 32) + 1] {
            let argument = interpreter.stash(Value::small_integer(index));
            match leaf(&mut interpreter, "value:slotAt:ifFail:", &pair, &[argument]) {
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
            match leaf(&mut interpreter, "value:slotAt:ifFail:", &pair, &[argument]) {
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
        let count = leaf(&mut interpreter, "valueSlotCount:ifFail:", &text, &[]);
        assert_eq!(
            answered_integer(&mut interpreter, count, "a String's slotCount"),
            DECLINED,
            "a String's state is its payload, not slots"
        );
    }

    /// The text every refusal of a value question about an `Escaping`
    /// still under construction carries.
    const ESCAPING_UNDER_CONSTRUCTION: &str = concat!(
        "an instance of Escaping is still under construction: ",
        "an instance of a value class is a value only once its factory has returned"
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

    /// `=` refuses a receiver or an argument whose factory never returned,
    /// before it compares anything: its slots are not all initialized, so
    /// it has no state to be equal by -- and a refusal, never a decline,
    /// so `valueEquals:` never meets it either.
    // spec: semantics.value.under-construction
    #[test]
    fn the_equality_leaf_refuses_an_operand_under_construction() {
        let (mut interpreter, fixture) = a_value_fixture();
        let escaped = send(&mut interpreter, &fixture, "escaped", &[]);
        let one = interpreter.stash(Value::small_integer(1));
        let point = send(&mut interpreter, &fixture, "pointX:y:", &[one.clone(), one]);
        for (receiver, argument, what) in [
            (&escaped, &escaped, "escaped = escaped"),
            (&escaped, &point, "escaped = point"),
            (&point, &escaped, "point = escaped"),
        ] {
            let compared = leaf(
                &mut interpreter,
                "value:sameAs:ifFail:",
                receiver,
                std::slice::from_ref(argument),
            );
            let (selector, message) = refusal(&mut interpreter, compared, what);
            assert_eq!(selector, Symbol::EQUALS, "{what}");
            assert_eq!(message, ESCAPING_UNDER_CONSTRUCTION, "{what}");
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
        assert_eq!(message, ESCAPING_UNDER_CONSTRUCTION);
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
        assert_eq!(message, ESCAPING_UNDER_CONSTRUCTION);
        let hashed = leaf(&mut interpreter, "valueHash:ifFail:", &first, &[]);
        let (selector, message) = refusal(&mut interpreter, hashed, "boxed hash");
        assert_eq!(selector, Symbol::intern("hash"));
        assert_eq!(message, ESCAPING_UNDER_CONSTRUCTION);
    }

    /// The mark is a header bit, and every mover copies or slides the
    /// header whole: a cut-short construction stays marked, and a complete
    /// value unmarked, through a scavenge into a survivor space, a tenure
    /// into the old space and a full collection's compaction.
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
