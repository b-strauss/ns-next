//! `Array` and `OrderedCollection` native methods.
//!
//! Design note (Milestone 1 simplification, per this task's brief):
//! `OrderedCollection` needs a mutable, growable payload, but
//! `ObjectData::Array(Vec<Value>)` (Task 3) is shared by both `Array` and
//! `OrderedCollection` -- the spec's Core Library list groups them loosely
//! together, and `Vec<Value>` already grows fine either way (mutated in
//! place through the `RefCell`, which already supports this). So for
//! Milestone 1, `OrderedCollection` is backed by the very same
//! `ObjectData::Array` variant rather than a second growable-collection
//! payload variant. A real `OrderedCollection` would grow independently of
//! a literal array's fixed size; revisit if a later milestone needs the two
//! to diverge (e.g. `Array` becoming genuinely fixed-size).

use crate::interpreter::{EvalResult, Signal};
use crate::types::*;
use crate::{Interpreter, Symbol};

fn out_of_range(receiver: &Value, selector: Symbol, index: i64, len: usize) -> NewspeakError {
    NewspeakError {
        selector,
        receiver: receiver.clone(),
        message: format!("index {index} out of range: valid indices are 1..{len} (size {len})"),
    }
}

fn not_an_integer(receiver: &Value, selector: Symbol, got: Option<&Value>) -> NewspeakError {
    NewspeakError {
        selector,
        receiver: receiver.clone(),
        message: format!("{selector} requires an Integer index, got {got:?}"),
    }
}

/// `size`, `at:`, `do:` -- shared by `Array` and `OrderedCollection` since
/// both share the `ObjectData::Array` payload (see this module's design
/// note). Returns `None` if `receiver` isn't `ObjectData::Array`-backed at
/// all, or if `selector` isn't one of these three (letting the caller fall
/// through to `doesNotUnderstand:`, or to the class-specific selector
/// handled by `dispatch_array`/`dispatch_ordered_collection` itself).
fn dispatch_common(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<EvalResult> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    match selector.as_str() {
        "size" => match &obj.borrow().data {
            ObjectData::Array(items) => Some(Ok(Value::SmallInt(items.len() as i64))),
            _ => None,
        },
        "at:" => {
            let items = match &obj.borrow().data {
                ObjectData::Array(items) => items.clone(),
                _ => return None,
            };
            Some(match args.first() {
                Some(Value::SmallInt(index)) => {
                    if *index >= 1 && (*index as usize) <= items.len() {
                        Ok(items[(*index as usize) - 1].clone())
                    } else {
                        Err(Signal::Error(out_of_range(
                            &receiver,
                            selector,
                            *index,
                            items.len(),
                        )))
                    }
                }
                other => Err(Signal::Error(not_an_integer(&receiver, selector, other))),
            })
        }
        // Task 12 fix: `do:` invokes its block argument via `call_block`,
        // which now forwards a `Signal::Return` (a non-local `^expr` inside
        // the block, e.g. `coll do: [:each | ... ifTrue: [^x]]`) straight
        // through instead of converting it into a `NewspeakError` -- so
        // this whole function's return type must be able to carry `Signal`,
        // not just `NewspeakError`, and this arm's `?` does exactly that.
        "do:" => {
            let items = match &obj.borrow().data {
                ObjectData::Array(items) => items.clone(),
                _ => return None,
            };
            let Some(block) = args.first().cloned() else {
                return Some(Err(Signal::Error(NewspeakError {
                    selector,
                    receiver: receiver.clone(),
                    message: "do: requires a block argument".to_string(),
                })));
            };
            for item in items {
                if let Err(signal) = super::call_block(interp, &block, vec![item]) {
                    return Some(Err(signal));
                }
            }
            Some(Ok(receiver.clone()))
        }
        _ => None,
    }
}

pub fn dispatch_array(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<EvalResult> {
    if selector.as_str() == "at:put:" {
        let Value::Obj(obj) = &receiver else {
            return None;
        };
        let len = match &obj.borrow().data {
            ObjectData::Array(items) => items.len(),
            _ => return None,
        };
        let index = match args.first() {
            Some(Value::SmallInt(index)) => *index,
            other => {
                return Some(Err(Signal::Error(not_an_integer(
                    &receiver, selector, other,
                ))))
            }
        };
        if index < 1 || (index as usize) > len {
            return Some(Err(Signal::Error(out_of_range(
                &receiver, selector, index, len,
            ))));
        }
        let value = args.get(1).cloned().unwrap_or(Value::Nil);
        if let ObjectData::Array(items) = &mut obj.borrow_mut().data {
            items[(index as usize) - 1] = value;
        }
        return Some(Ok(receiver.clone()));
    }
    dispatch_common(interp, receiver, selector, args)
}

pub fn dispatch_ordered_collection(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<EvalResult> {
    if selector.as_str() == "add:" {
        let Value::Obj(obj) = &receiver else {
            return None;
        };
        if !matches!(&obj.borrow().data, ObjectData::Array(_)) {
            return None;
        }
        // Matches the reference library's convention: `add:` returns the
        // argument, not the receiver.
        let value = args.first().cloned().unwrap_or(Value::Nil);
        if let ObjectData::Array(items) = &mut obj.borrow_mut().data {
            items.push(value.clone());
        }
        return Some(Ok(value));
    }
    dispatch_common(interp, receiver, selector, args)
}
