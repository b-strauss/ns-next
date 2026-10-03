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
//! `numbers`' and `text`'s named helpers, which their `=` and `hash`
//! primitives call too. A second copy of those rules is how the derived
//! answer and the class's own would drift apart.

use std::collections::HashSet;
use std::rc::Rc;

use crate::builtins::collections::{self, IntegerArgument};
use crate::builtins::primitive::{Primitive, ValuePrimitive};
use crate::builtins::{numbers, text};
use crate::dispatch::{self, Lookup};
use crate::interpreter::instantiate::class_object_is_a_value;
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
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        ValuePrimitive::SameAs => {
            let other = *arguments.first()?;
            let equal = derived_equality(mutator, receiver, other)?;
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Value(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        ValuePrimitive::Hash => {
            derived_hash(mutator, receiver).map(|hash| Ok(Value::small_integer(hash)))
        }
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
            let index = collections::integer_argument(&mutator.heap, arguments.first().copied());
            let (count, field) = with_declared_slots(mutator, receiver, |declared| {
                let field = match index {
                    Some(IntegerArgument::Small(index)) if index >= 1 => {
                        declared.get((index as usize) - 1).copied()
                    }
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
                    Some(IntegerArgument::Small(index)) => {
                        collections::out_of_range(receiver, selector, index, count)
                    }
                    Some(IntegerArgument::Large { .. }) => collections::out_of_range(
                        receiver,
                        selector,
                        mutator.heap.describe_argument(arguments.first().copied()),
                        count,
                    ),
                    None => NewspeakError {
                        selector,
                        receiver,
                        message: format!(
                            "{selector} requires an Integer index, got {}",
                            mutator.heap.describe_argument(arguments.first().copied())
                        ),
                    },
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
    match (first.kind(), second.kind()) {
        (ValueKind::Nil, ValueKind::Nil) => Some(Pair::Decided(true)),
        (ValueKind::Boolean(left), ValueKind::Boolean(right)) => Some(Pair::Decided(left == right)),
        (ValueKind::SmallInteger(left), _) => {
            Some(Pair::Decided(numbers::integer_equals(left, Some(second))))
        }
        (ValueKind::SmallFloat(left), _) => Some(Pair::Decided(numbers::float_equals(
            mutator.heap(),
            left,
            Some(second),
        ))),
        (ValueKind::Object(left), ValueKind::Object(right)) => {
            if mutator.heap().is_class(left) && mutator.heap().is_class(right) {
                return Some(Pair::Classes(left, right));
            }
            if let Some(symbol) = mutator.heap().symbol_value(left) {
                return Some(Pair::Decided(text::symbol_equals(
                    mutator.heap(),
                    left,
                    symbol,
                    Some(second),
                )));
            }
            if let Some(integer) = mutator.heap().large_integer(left) {
                return Some(Pair::Decided(numbers::large_integer_equals(
                    mutator.heap(),
                    &integer,
                    Some(second),
                )));
            }
            if let Some(number) = mutator.heap().float_value(Value::object(left)) {
                return Some(Pair::Decided(numbers::float_equals(
                    mutator.heap(),
                    number,
                    Some(second),
                )));
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
    match value.kind() {
        ValueKind::Nil => Some(Node::Leaf(NIL_HASH)),
        // `True>>hash` and `False>>hash`, so a boolean slot hashes as the
        // boolean itself does.
        ValueKind::Boolean(flag) => Some(Node::Leaf(i64::from(flag))),
        // Not `Integer>>hash`, which answers the integer itself and so can
        // be negative or as large as an `i64`: a slot's hash has to stay
        // below 2^32 for the sum below to stay inside one. Nothing requires
        // the two to agree -- an `Integer` declares its own `hash` and never
        // reaches this leaf as a receiver.
        ValueKind::SmallInteger(integer) => Some(Node::Leaf(crate::hash::content_hash(integer))),
        ValueKind::SmallFloat(float) => Some(Node::Leaf(numbers::float_hash(float))),
        ValueKind::Object(object) => {
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
            if let Some(symbol) = mutator.heap().symbol_value(object) {
                return Some(Node::Leaf(text::symbol_hash(symbol)));
            }
            if let Some(integer) = mutator.heap().large_integer(object) {
                return Some(Node::Leaf(numbers::large_integer_hash(&integer)));
            }
            if let Some(number) = mutator.heap().float_value(Value::object(object)) {
                return Some(Node::Leaf(numbers::float_hash(number)));
            }
            if let Some(hash) = mutator.heap().string_hash(object) {
                return Some(Node::Leaf(hash));
            }
            state_is_a_value(mutator, object).then_some(Node::State(object))
        }
    }
}

/// Whether `left` and `right` are equal as values; `None` is the decline.
///
/// A pair already on `assumed` is taken to be equal: that is what makes a
/// cyclic value answer `true` instead of unfolding forever, and it is
/// sound because a single `false` anywhere ends the walk -- an assumption
/// survives only when nothing contradicts it.
fn derived_equality<'gc>(
    mutator: &Mutator<'gc>,
    left: Value<'gc>,
    right: Value<'gc>,
) -> Option<bool> {
    // Two class objects are the operands of `Behavior>>=` and `==`, and so
    // of every `self class == other class`, which was `Object`'s identity
    // leaf before class objects became values: answered here without
    // building the walk wherever the walk's first step would decide. Only
    // for class objects.
    if let (Some(first), Some(second)) = (left.as_object(), right.as_object()) {
        if mutator.heap().is_class(first) && mutator.heap().is_class(second) {
            if first.same(second) {
                return Some(true);
            }
            if !classes_can_be_equal(mutator, first, second) {
                return Some(false);
            }
        }
    }
    let mut work = vec![(left, right)];
    let mut assumed: HashSet<(usize, usize)> = HashSet::new();
    // The root pair is on the stack already.
    let mut charged = 1usize;
    while let Some((first, second)) = work.pop() {
        // The classes first, for every shape: §3.1.1's equality is equality
        // of state *within* a class, so two values whose classes are unequal
        // are unequal whatever their states look like. Two distinct classes
        // can still be equal -- one nested declaration under two equal
        // enclosing values -- so they are a pair of their own, pushed after
        // this node's own pairs and so examined next: a class that settles
        // the answer does so before a slot can decline it.
        let first_class = mutator.class_of(first);
        let second_class = mutator.class_of(second);
        let classes = if first_class.same(second_class) {
            None
        } else {
            // What `Pair::Classes` would decide at once, asked here so that
            // two values of unrelated classes -- the common unequal pair --
            // answer without unfolding either.
            if !classes_can_be_equal(mutator, first_class, second_class) {
                return Some(false);
            }
            // A pair already assumed has been unfolded; pushing it again
            // would only spend budget on a node the walk discards.
            if assumed.contains(&identity_pair(mutator, first_class, second_class)) {
                None
            } else {
                if charged >= NODE_BUDGET {
                    return None;
                }
                charged += 1;
                Some((Value::object(first_class), Value::object(second_class)))
            }
        };
        match compare_node(mutator, first, second)? {
            Pair::Decided(true) => {}
            Pair::Decided(false) => return Some(false),
            // Assumed like a pair of instances, so a cycle through classes
            // -- a class whose enclosing value's slot holds that very class
            // -- ends where one through slots does.
            Pair::Classes(first_class, second_class)
                if !first_class.same(second_class)
                    && assumed.insert(identity_pair(mutator, first_class, second_class)) =>
            {
                let Some(pairs) = class_state_pairs(mutator, first_class, second_class) else {
                    return Some(false);
                };
                // Charged before they are pushed, like an instance's slots.
                if pairs.len() > NODE_BUDGET - charged {
                    return None;
                }
                charged += pairs.len();
                work.extend(pairs);
            }
            Pair::Classes(..) => {}
            Pair::State(first_object, second_object)
                if assumed.insert(identity_pair(mutator, first_object, second_object)) =>
            {
                let first_slots = slots_of(mutator, first_object);
                let second_slots = slots_of(mutator, second_object);
                // One class lays out one set of slots, so this holds
                // already; checked because the alternative is zipping two
                // different lengths and calling the shorter prefix equal.
                if first_slots.len() != second_slots.len() {
                    return Some(false);
                }
                if first_slots.len() > NODE_BUDGET - charged {
                    // Out of budget, with pairs still unexamined: decline
                    // rather than claim an equality that was never checked,
                    // and decline *before* pushing them, so the stack never
                    // grows past the budget. `Value`'s `ifFail:` block then
                    // answers, as it does for any other shape this leaf
                    // will not rule on.
                    return None;
                }
                charged += first_slots.len();
                work.extend(first_slots.into_iter().zip(second_slots));
            }
            Pair::State(..) => {}
        }
        work.extend(classes);
    }
    Some(true)
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
    if !class_object_is_a_value(mutator, first) || !class_object_is_a_value(mutator, second) {
        return false;
    }
    match (
        mutator.heap().class_application(first),
        mutator.heap().class_application(second),
    ) {
        (Some(first_application), Some(second_application)) => {
            Rc::ptr_eq(&first_application, &second_application)
        }
        _ => false,
    }
}

/// The pairs two distinct class objects are equal by, or `None` when they
/// are unequal outright. Two metaclasses are equal when their classes are:
/// a metaclass's enclosing object is its class's and its superclass is
/// always `Class`, so its class is all its state. Any other two are equal
/// when they are values of one application whose enclosing objects are
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
    Some(vec![
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

/// The hash `value`'s state gives it; `None` is the decline. Every node
/// reached contributes its own hash, and every instance its slot count, as
/// the sum the answer is.
fn derived_hash<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> Option<i64> {
    let mut work = vec![value];
    let mut total: i64 = 0;
    // The root is on the stack already.
    let mut charged = 1usize;
    while let Some(current) = work.pop() {
        match hash_node(mutator, current)? {
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
    Some(total)
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
    /// both paths, depending on what their slot holds.
    ///
    /// `Outer`'s `Inner` is a class object per `Outer` instance, and a value
    /// because its enclosing `Outer` is. `Looped` holds its own `Inner` in a
    /// slot, so its state reaches itself through a class object's enclosing
    /// object: a cycle through classes.
    fn a_value_fixture() -> (Interpreter, Root) {
        let source = indoc! {"
            class ValueFixture = Object <: Value () (
                class Point x: xValue y: yValue = Value new (
                    | public x = xValue. public y = yValue. |
                ) ()
                class Tag = Value new () (
                    public = other = ( ^self class == other class )
                    public hash = ( ^7 )
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
                public pointX: xValue y: yValue = ( ^Point x: xValue y: yValue )
                public lazy: xValue = ( ^Lazy x: xValue )
                public tag = ( ^Tag new )
                public boxed: contents = ( ^Boxed on: contents )
                public outer: xValue = ( ^Outer x: xValue )
                public looped: xValue = ( ^Looped x: xValue )
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

    /// The derived equality asks the classes first, and `Integer` is not
    /// `Float`. `Float>>=` is class-tight for the same reason (2026-09-23),
    /// so the leaf here and `Float`'s own `=` answer this pair alike --
    /// while `Float>>=` widened an `Integer` argument, the two disagreed
    /// about `1.0 = 1` and `1.0 hash = 1 hash` was false either way.
    #[test]
    fn an_integer_and_a_float_of_one_magnitude_are_not_one_value() {
        let (mut interpreter, _fixture) = a_fixture();
        let float = interpreter.enter(|mutator| {
            let float = mutator.new_float(1.0);
            mutator.stash(float)
        });
        let integer = interpreter.stash(Value::small_integer(1));
        let compared = leaf(&mut interpreter, "value:sameAs:ifFail:", &float, &[integer]);
        assert_eq!(
            answered_boolean(&mut interpreter, compared, "1.0 sameAs: 1"),
            Some(false)
        );
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

    /// `hash_node` hashes an `Integer` slot with `hash::content_hash` while
    /// `Integer>>hash`, which the Newspeak fallback sums, answers the
    /// integer itself. The two are allowed to differ only because a value
    /// the leaf hashes and a value the fallback hashes can never be equal:
    /// the leaf's decision is a question about the slots' classes, and two
    /// values of one class whose slots' classes differ are unequal slot by
    /// slot.
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
        let fixture_class = send(&mut interpreter, &fixture, "class", &[]);
        let other_fixture = send(&mut interpreter, &fixture_class, "new", &[]);
        let marker = send(&mut interpreter, &fixture, "marker", &[]);
        let marker_class = send(&mut interpreter, &marker, "class", &[]);
        let other_marker = send(&mut interpreter, &other_fixture, "marker", &[]);
        let other_marker_class = send(&mut interpreter, &other_marker, "class", &[]);
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
        let first_metaclass = send(&mut interpreter, &first, "class", &[]);
        let second_metaclass = send(&mut interpreter, &second, "class", &[]);
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
    /// instead, reading their state through the three leaves it has for
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
        let string_metaclass = send(&mut interpreter, &string, "class", &[]);
        let symbol_metaclass = send(&mut interpreter, &symbol, "class", &[]);
        let marker = send(&mut interpreter, &fixture, "marker", &[]);
        let marker_class = send(&mut interpreter, &marker, "class", &[]);
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
        for index in [0, 3] {
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
}
