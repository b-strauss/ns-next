//! The equality and hash a value object gets from its state, derived in
//! Rust so that `Value` can declare `=` and `hash` without any native ever
//! calling back into Newspeak.
//!
//! §3.1.1 makes a value object deeply immutable and lets it be copied
//! across an actor boundary, so a copy has to be equal to, and hash like,
//! its original: **a value has no observable identity**, and deciding
//! either from one is exactly the mistake these leaves exist to avoid. Two
//! values are equal when their classes agree and their states do, all the
//! way down.
//!
//! Neither walk recurses. An image is untrusted input and the graph a
//! decoded value reaches is the image's to choose, so both walks run an
//! explicit work stack; the equality walk remembers the pairs it has
//! already assumed equal, so a cycle terminates in `true` rather than
//! running forever, and both stop once they have taken `NODE_BUDGET` nodes
//! off that stack, so neither runs for as long as the input likes.
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

use crate::builtins::primitive::ValuePrimitive;
use crate::builtins::{numbers, text};
use crate::dispatch::{self, Lookup};
use crate::types::*;
use crate::{Mutator, Symbol};

/// How many nodes either walk takes off its work stack before it stops
/// unfolding.
///
/// It is checked when a node is taken, not when one is pushed, so a node
/// already taken pushes all of its slots before the next check: the work is
/// bounded by `NODE_BUDGET` times the widest class's slot count, and the
/// work vector itself by what the heap holds, not by this number. What the
/// budget does rule out is a walk that runs for as long as the input likes
/// -- a wide graph still costs a bounded multiple of it, and a deep or
/// cyclic one stops here.
///
/// It is also what keeps a truncated hash *consistent*: two equal values
/// unfold to the same nodes in the same order, so cutting both off at the
/// same count cuts them off alike. Every node contributes a hash below 2^32
/// plus a slot count below 2^16, so a sum of this many stays far inside an
/// `i64`.
const NODE_BUDGET: usize = 4096;

/// `nil`'s hash. `Nil` is a singleton below `Value`, so any constant will
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
            derived_equality(mutator, receiver, other).map(|equal| Ok(Value::Boolean(equal)))
        }
        ValuePrimitive::Hash => {
            derived_hash(mutator, receiver).map(|hash| Ok(Value::SmallInteger(hash)))
        }
        ValuePrimitive::SlotCount => {
            let count = own_slot_count(mutator, receiver)?;
            Some(Ok(Value::SmallInteger(count as i64)))
        }
        ValuePrimitive::SlotAt => {
            let Value::Object(object) = receiver else {
                return None;
            };
            let count = own_slot_count(mutator, receiver)?;
            Some(match arguments.first() {
                Some(Value::SmallInteger(index)) if *index >= 1 && (*index as usize) <= count => {
                    Ok(mutator
                        .heap
                        .instance_field(object, (*index as usize) - 1)
                        .unwrap_or(Value::Nil))
                }
                // An index outside the range is the program's mistake, not a
                // shape this leaf does not serve, so it is an ordinary
                // Newspeak error in the words `Array>>at:` already uses --
                // never a decline, which would answer the `ifFail:` block
                // and hide it.
                Some(Value::SmallInteger(index)) => Err(NewspeakError {
                    selector,
                    receiver,
                    message: format!(
                        "index {index} out of range: valid indices are 1..{count} (size {count})"
                    ),
                }),
                other => Err(NewspeakError {
                    selector,
                    receiver,
                    message: format!(
                        "{selector} requires an Integer index, got {}",
                        mutator.heap.describe_argument(other)
                    ),
                }),
            })
        }
    }
}

/// How many slots `receiver` lays out, when it is an ordinary instance --
/// the only shape whose state is slots at all. `None` for anything else,
/// which is the decline.
fn own_slot_count<'gc>(mutator: &Mutator<'gc>, receiver: Value<'gc>) -> Option<usize> {
    let Value::Object(object) = receiver else {
        return None;
    };
    if mutator.heap().is_class(object) {
        return None;
    }
    mutator.heap().instance_field_count(object)
}

/// What a walk found at one node of a value: either a verdict the shape's
/// own rule decides outright, or an instance whose state is its slots and
/// so has to be unfolded.
enum Pair<'gc> {
    Decided(bool),
    State(ObjectReference<'gc>, ObjectReference<'gc>),
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
fn state_is_a_value<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> bool {
    let Some(recorded) = mutator.value_equality_method.as_ref() else {
        return false;
    };
    if mutator.heap().is_class(object) || mutator.heap().instance_field_count(object).is_none() {
        return false;
    }
    let class = mutator.heap().class_of_object(object);
    match dispatch::lookup_public(mutator.heap(), class, Symbol::EQUALS) {
        Lookup::Method(found, _) => Rc::ptr_eq(&found, recorded),
        _ => false,
    }
}

/// The slots of `object`, copied out so the walk holds no borrow of the
/// heap while it goes on.
fn slots_of<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> Vec<Value<'gc>> {
    let count = mutator.heap().instance_field_count(object).unwrap_or(0);
    (0..count)
        .map(|index| {
            mutator
                .heap()
                .instance_field(object, index)
                .unwrap_or(Value::Nil)
        })
        .collect()
}

/// One node of the equality walk: `None` declines the whole comparison.
fn compare_node<'gc>(
    mutator: &Mutator<'gc>,
    first: Value<'gc>,
    second: Value<'gc>,
) -> Option<Pair<'gc>> {
    // The class first, for every shape: §3.1.1's equality is equality of
    // state *within* a class, so two values of different classes are
    // unequal whatever their states look like.
    if !mutator.class_of(&first).same(mutator.class_of(&second)) {
        return Some(Pair::Decided(false));
    }
    match (first, second) {
        (Value::Nil, Value::Nil) => Some(Pair::Decided(true)),
        (Value::Boolean(left), Value::Boolean(right)) => Some(Pair::Decided(left == right)),
        (Value::SmallInteger(left), _) => {
            Some(Pair::Decided(numbers::integer_equals(left, Some(&second))))
        }
        (Value::Float(left), _) => Some(Pair::Decided(numbers::float_equals(left, Some(&second)))),
        (Value::Object(left), Value::Object(right)) => {
            if let Some(symbol) = mutator.heap().symbol_value(left) {
                return Some(Pair::Decided(text::symbol_equals(
                    mutator.heap(),
                    left,
                    symbol,
                    Some(&second),
                )));
            }
            if let Some(payload) = mutator.heap().str_value(left) {
                return Some(Pair::Decided(text::text_equals(
                    mutator.heap(),
                    left,
                    &payload,
                    Some(&second),
                )));
            }
            state_is_a_value(mutator, left).then_some(Pair::State(left, right))
        }
        // One class, two shapes: nothing builds such a pair today, and
        // answering "unequal" is the reading that cannot claim an equality
        // it did not check.
        _ => Some(Pair::Decided(false)),
    }
}

/// One node of the hash walk: `None` declines the whole hash.
fn hash_node<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> Option<Node<'gc>> {
    match value {
        Value::Nil => Some(Node::Leaf(NIL_HASH)),
        // `Boolean>>hash`'s two answers, so a boolean slot hashes as the
        // boolean itself does.
        Value::Boolean(flag) => Some(Node::Leaf(i64::from(flag))),
        // Not `Integer>>hash`, which answers the integer itself and so can
        // be negative or as large as an `i64`: a slot's hash has to stay
        // below 2^32 for the sum below to stay inside one. Nothing requires
        // the two to agree -- an `Integer` declares its own `hash` and never
        // reaches this leaf as a receiver.
        Value::SmallInteger(integer) => Some(Node::Leaf(crate::hash::content_hash(integer))),
        Value::Float(float) => Some(Node::Leaf(numbers::float_hash(float))),
        Value::Object(object) => {
            if let Some(symbol) = mutator.heap().symbol_value(object) {
                return Some(Node::Leaf(text::symbol_hash(symbol)));
            }
            if let Some(payload) = mutator.heap().str_value(object) {
                let class = mutator.heap().class_of_object(object);
                // `String` and `Character` share a payload, and each hashes
                // as its own class's `hash` does: a character by its code
                // point, a string by its content.
                return Some(Node::Leaf(
                    if mutator.heap().class_builtin(class) == Some(BuiltinClass::Character) {
                        text::character_of(&payload) as i64
                    } else {
                        text::string_hash(&payload)
                    },
                ));
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
    let mut work = vec![(left, right)];
    let mut assumed: HashSet<(usize, usize)> = HashSet::new();
    let mut visited = 0usize;
    while let Some((first, second)) = work.pop() {
        if visited == NODE_BUDGET {
            // Out of budget, with pairs still unexamined: decline rather
            // than claim an equality that was never checked. `Value`'s
            // `ifFail:` block then answers, as it does for any other shape
            // this leaf will not rule on.
            return None;
        }
        visited += 1;
        match compare_node(mutator, first, second)? {
            Pair::Decided(true) => {}
            Pair::Decided(false) => return Some(false),
            Pair::State(first_object, second_object) => {
                let key = (
                    mutator.heap().object_identity_key(first_object),
                    mutator.heap().object_identity_key(second_object),
                );
                if !assumed.insert(key) {
                    continue;
                }
                let first_slots = slots_of(mutator, first_object);
                let second_slots = slots_of(mutator, second_object);
                // One class lays out one set of slots, so this holds
                // already; checked because the alternative is zipping two
                // different lengths and calling the shorter prefix equal.
                if first_slots.len() != second_slots.len() {
                    return Some(false);
                }
                work.extend(first_slots.into_iter().zip(second_slots));
            }
        }
    }
    Some(true)
}

/// The hash `value`'s state gives it; `None` is the decline. Every node
/// reached contributes its own hash, and every instance its slot count, as
/// the sum the answer is.
fn derived_hash<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> Option<i64> {
    let mut work = vec![value];
    let mut total: i64 = 0;
    let mut visited = 0usize;
    while let Some(current) = work.pop() {
        // Unlike the comparison, a truncated hash is still a valid hash:
        // two equal values unfold in the same order, so both are cut at the
        // same place and both answer the same number.
        if visited == NODE_BUDGET {
            break;
        }
        visited += 1;
        match hash_node(mutator, current)? {
            Node::Leaf(hash) => total += hash,
            Node::State(object) => {
                let slots = slots_of(mutator, object);
                total += slots.len() as i64;
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

    use crate::{HostResult, HostSignal, Interpreter, Root, Symbol, Value};

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
        sent.push(interpreter.stash(Value::SmallInteger(DECLINED)));
        interpreter.send(&mirror, Symbol::intern(selector), &sent)
    }

    /// The integer a leaf answered, or `DECLINED`.
    fn answered_integer(interpreter: &mut Interpreter, result: HostResult, what: &str) -> i64 {
        let root = expect_ok(result, what);
        interpreter.enter(|mutator| match mutator.fetch(&root) {
            Value::SmallInteger(integer) => integer,
            other => panic!("{what} answered {}", mutator.heap().describe(&other)),
        })
    }

    /// The boolean a leaf answered; `None` when it declined.
    fn answered_boolean(
        interpreter: &mut Interpreter,
        result: HostResult,
        what: &str,
    ) -> Option<bool> {
        let root = expect_ok(result, what);
        interpreter.enter(|mutator| match mutator.fetch(&root) {
            Value::Boolean(flag) => Some(flag),
            Value::SmallInteger(DECLINED) => None,
            other => panic!("{what} answered {}", mutator.heap().describe(&other)),
        })
    }

    /// A module whose `Marker` declares the `=` these tests record as
    /// `Value`'s. `Value` declares none yet, so the derivation would
    /// decline every instance; recording `Marker`'s puts the leaf in the
    /// state Task 5's `Value>>=` will put it in for real, without changing
    /// `Kernel.ns` ahead of that task.
    fn a_fixture() -> (Interpreter, Root) {
        let source = indoc! {"
            Newspeak3
            'T'
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
                public marker = ( ^Marker new )
                public pair: firstValue and: secondValue = ( ^Pair first: firstValue second: secondValue )
                public couple: firstValue and: secondValue = ( ^Couple first: firstValue second: secondValue )
                public overriding = ( ^Overriding new )
                public cyclic = ( ^Cyclic new )
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
            let class = mutator.class_of(&receiver);
            let Some(application) = mutator.heap().class_application(class) else {
                panic!("a Marker's class is a class");
            };
            let Some(method) = application.mixin.compiled_method(Symbol::EQUALS) else {
                panic!("Marker declares =");
            };
            mutator.value_equality_method = Some(method.clone());
        });
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
            mutator.stash(Value::Object(string))
        })
    }

    fn an_array(interpreter: &mut Interpreter) -> Root {
        interpreter.enter(|mutator| {
            let array = mutator.new_array(vec![Value::SmallInteger(1)]);
            mutator.stash(Value::Object(array))
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

    /// `1.0 = 1` is true, because `Float>>=` widens its argument. The
    /// derived equality asks the classes first, and `Integer` is not
    /// `Float`.
    #[test]
    fn an_integer_and_a_float_of_one_magnitude_are_not_one_value() {
        let (mut interpreter, _fixture) = a_fixture();
        let float = interpreter.stash(Value::Float(1.0));
        let integer = interpreter.stash(Value::SmallInteger(1));
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
        let one = interpreter.stash(Value::SmallInteger(1));
        let two = interpreter.stash(Value::SmallInteger(2));
        let three = interpreter.stash(Value::SmallInteger(3));
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
        let one = interpreter.stash(Value::SmallInteger(1));
        let two = interpreter.stash(Value::SmallInteger(2));
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

    /// An `Array` is no value: it is mutable and sits under `Object`, so
    /// its `=` is `Object`'s identity. A slot holding one takes the whole
    /// comparison out of the leaf's hands.
    #[test]
    fn a_slot_holding_an_array_makes_the_leaf_decline() {
        let (mut interpreter, fixture) = a_fixture();
        record_marker_as_values_equality(&mut interpreter, &fixture);
        let one = interpreter.stash(Value::SmallInteger(1));
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
        let one = interpreter.stash(Value::SmallInteger(1));
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
        let one = interpreter.stash(Value::SmallInteger(1));
        let two = interpreter.stash(Value::SmallInteger(2));
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

    #[test]
    fn the_reflective_leaves_read_the_slots_and_refuse_an_index_out_of_range() {
        let (mut interpreter, fixture) = a_fixture();
        let one = interpreter.stash(Value::SmallInteger(1));
        let two = interpreter.stash(Value::SmallInteger(2));
        let pair = send(&mut interpreter, &fixture, "pair:and:", &[one, two]);
        let count = leaf(&mut interpreter, "valueSlotCount:ifFail:", &pair, &[]);
        assert_eq!(answered_integer(&mut interpreter, count, "slotCount"), 2);
        for (index, expected) in [(1, 1), (2, 2)] {
            let argument = interpreter.stash(Value::SmallInteger(index));
            let slot = leaf(&mut interpreter, "value:slotAt:ifFail:", &pair, &[argument]);
            assert_eq!(
                answered_integer(&mut interpreter, slot, "slotAt:"),
                expected
            );
        }
        for index in [0, 3] {
            let argument = interpreter.stash(Value::SmallInteger(index));
            match leaf(&mut interpreter, "value:slotAt:ifFail:", &pair, &[argument]) {
                Err(HostSignal::Error(error)) => {
                    assert_eq!(error.selector, Symbol::intern("slotAt:"));
                    assert!(error.message.contains("out of range"), "{}", error.message);
                }
                _ => panic!("slotAt: {index} must raise an error, not answer"),
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
