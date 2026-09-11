//! Send helpers (`LexicalSend`, `SuperSend`), local and outer-local access,
//! closure invocation, and the errors the VM raises for a deferred literal.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::dispatch::{self, Lookup};
use crate::heap::{ActivationHeader, ActivationRef, Heap};
use crate::types::*;
use crate::Symbol;

use super::{EvalResult, Interpreter, Signal};

/// The program-level error a bare identifier raises when it has no binding
/// and names no builtin class -- in Milestone 1 and 2 reachable only for a
/// bare `super`, which `resolver::resolve_ident` deliberately leaves
/// unbound and which the lexer deliberately does not reserve.
///
/// Raised by `vm::make_value` for `bytecode::Literal::UnresolvedIdent`.
pub(crate) fn unresolved_identifier_error(name: &str, receiver: Value) -> Signal {
    Signal::Error(NewspeakError {
        selector: Symbol::intern(name),
        receiver,
        message: format!("internal: unresolved identifier `{name}`"),
    })
}

/// The error a lexer-accepted integer literal that `parse_int_literal`
/// rejects raises when it is *evaluated*. Raised by `vm::make_value` for
/// `bytecode::Literal::MalformedInt`.
pub(crate) fn malformed_int_error(message: String) -> Signal {
    Signal::Error(NewspeakError {
        selector: Symbol::intern("parse"),
        receiver: Value::Nil,
        message,
    })
}

pub(crate) fn read_local(heap: &Heap, act: &ActivationRef, index: u16) -> Value {
    heap.activation_local(act, index)
}

pub(crate) fn write_local(heap: &mut Heap, act: &ActivationRef, index: u16, value: Value) {
    heap.activation_set_local(act, index, value);
}

/// Where a `Binding::OuterLocal` lives: in the running activation itself,
/// or in a frame some closure captured.
enum OuterFrame {
    Own(ActivationRef),
    Captured(FrameRef),
}

/// The activation `depth` hops out along the `captured_frame` chain. The
/// first hop leaves the running activation for its closure's captured frame;
/// the rest follow frames. It stops hopping at a non-closure activation, so a
/// depth past the lexical nesting lands on the outermost body rather than
/// running off the chain -- `image::decode::enclosing_body` is the static
/// counterpart and relies on exactly this, for an image whose `depth` it
/// verified against the lexical nesting rather than the chain.
fn frame_at_depth(heap: &Heap, act: &ActivationRef, depth: u16) -> OuterFrame {
    if depth == 0 {
        return OuterFrame::Own(*act);
    }
    let Some(mut frame) = heap.activation_captured(act) else {
        return OuterFrame::Own(*act);
    };
    for _ in 1..depth {
        frame = heap.frame_captured(&frame).unwrap_or(frame);
    }
    OuterFrame::Captured(frame)
}

/// The VM's `LoadOuterLocal`.
pub(crate) fn read_outer_local(heap: &Heap, act: &ActivationRef, depth: u16, index: u16) -> Value {
    match frame_at_depth(heap, act, depth) {
        OuterFrame::Own(own) => heap.activation_local(&own, index),
        OuterFrame::Captured(frame) => heap.frame_local(&frame, index),
    }
}

/// The VM's `StoreOuterLocal`.
pub(crate) fn write_outer_local(
    heap: &mut Heap,
    act: &ActivationRef,
    depth: u16,
    index: u16,
    value: Value,
) {
    match frame_at_depth(heap, act, depth) {
        OuterFrame::Own(own) => heap.activation_set_local(&own, index, value),
        OuterFrame::Captured(frame) => heap.frame_set_local(&frame, index, value),
    }
}

/// The `home` a closure created in `act` gets: a closure activation passes
/// its own closure's `home` on; any other activation -- method, initializer,
/// superclass clause or message -- is its own return target, so it is
/// reified and is its own home. The VM's `MakeClosure` calls it.
pub(crate) fn home_of(heap: &mut Heap, act: &ActivationRef) -> FrameRef {
    match heap.activation_home(act) {
        Some(home) => home,
        None => heap.reify(act),
    }
}

/// Writes `value` into instance field `index` on `instance`'s
/// `ObjectData::Instance` fields -- silently skipped when `instance` is not
/// an object, or is an object with some other `ObjectData` shape, or
/// `index` is out of range. `vm::run_code`'s `Instr::InitSlot` arm writes a
/// slot initializer's value through it.
pub(crate) fn store_instance_field(heap: &mut Heap, instance: &Value, index: usize, value: Value) {
    if let Value::Obj(obj) = instance {
        heap.set_instance_field(obj, index, value);
    }
}

/// Evaluates an `ast::Literal::Int`'s source text, which the lexer hands
/// over verbatim.
///
/// Two shapes reach here (`lexer.rs`'s `lex_number`, and `parser/expr.rs`'s
/// `parse_literal`, which classifies anything containing `r` as an `Int`):
/// a plain decimal literal (`42`, `-42`), and the Smalltalk/Newspeak radix
/// literal `<radix>r<digits>` (`16rFF`, `2r1010`, `16r-1F`, `-16rFF`) --
/// the spec's Source Format section accepts both. `text.parse::<i64>()`
/// alone rejected every radix literal with a misleading "out of i64 range".
///
/// Milestone 1 gap kept explicit: the lexer also accepts a *fractional*
/// radix literal (`16r1.8`), which the parser still classifies as an `Int`.
/// That needs radix-aware float construction; it is rejected here with a
/// message that says so rather than silently truncating.
///
/// Returns the `NewspeakError` message on failure (never panics -- this is a
/// Newspeak-program-input path).
pub(crate) fn parse_int_literal(text: &str) -> Result<i64, String> {
    let Some((radix_part, digits)) = text.split_once('r') else {
        return text
            .parse::<i64>()
            .map_err(|_| format!("integer literal out of i64 range: {text}"));
    };
    // A leading `-` on the radix itself (`-16rFF`) negates the whole value;
    // a leading `-` on the digits (`16r-1F`) is handled by `from_str_radix`.
    let (negated, radix_digits) = match radix_part.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, radix_part),
    };
    let radix: u32 = radix_digits
        .parse()
        .map_err(|_| format!("malformed radix in integer literal `{text}`"))?;
    if !(2..=36).contains(&radix) {
        return Err(format!(
            "radix {radix} out of range in integer literal `{text}`: must be 2..36"
        ));
    }
    if digits.contains('.') {
        return Err(format!(
            "fractional radix literals are not supported in Milestone 1: `{text}`"
        ));
    }
    let value = i64::from_str_radix(digits, radix).map_err(|_| {
        format!("malformed or out-of-i64-range radix integer literal `{text}` (radix {radix})")
    })?;
    if negated {
        value
            .checked_neg()
            .ok_or_else(|| format!("integer literal out of i64 range: {text}"))
    } else {
        Ok(value)
    }
}

/// A slot's getter (`x`, no colon) or setter (`x:`, exactly one trailing
/// colon and no others) shape, per `newspeak-spec.pdf` §6.3.2 -- a slot's
/// getter/setter are "implicitly defined by the slot declaration", not real
/// `MethodDecl`s in the class's mixin, so `dispatch_lexical_send` must
/// recognize and serve them directly rather than relying on method lookup
/// to ever find them. Returns `None` for a genuine multi-keyword selector
/// (e.g. `at:put:`), which can never denote a slot access.
fn slot_selector_shape(selector: Symbol) -> Option<(Symbol, bool)> {
    let s = selector.as_str();
    if !s.contains(':') {
        return Some((selector, false));
    }
    if s.ends_with(':') && s.matches(':').count() == 1 {
        return Some((Symbol::intern(&s[..s.len() - 1]), true));
    }
    None
}

/// What an implicit-receiver send resolves to on one class, per
/// `newspeak-spec.pdf` §5.9: a `private` declaration *at exactly this
/// class* wins first, because a private member is never overridden and so
/// there is no search; anything else goes through the ordinary
/// `lookup_protected` walk.
///
/// A `private class N` is a private declaration on exactly the same footing
/// (§6: the implicit accessor has the declaration's own accessibility), and
/// has to be caught by the same first step: `lookup_protected` skips a
/// private declaration transparently and keeps walking, so leaving the
/// nested case out would make `private class N` unreachable from the class
/// that declares it while `private n = (...)` is reachable.
///
/// One definition because `dispatch_lexical_send` asks it of two classes --
/// a class object's metaclass and then the receiver's own class.
fn lexical_lookup(heap: &crate::heap::Heap, class: &ClassRef, selector: Symbol) -> Lookup {
    match dispatch::lookup_defined_for(heap, class, selector) {
        Lookup::Method(method, defining_class) if method.access == ast::AccessModifier::Private => {
            Lookup::Method(method, defining_class)
        }
        Lookup::NestedClass(mixin, declaring_class)
            if dispatch::nested_access(&mixin) == ast::AccessModifier::Private =>
        {
            Lookup::NestedClass(mixin, declaring_class)
        }
        _ => dispatch::lookup_protected(heap, class, selector),
    }
}

/// `newspeak-spec.pdf` §5.7-§5.9: `o` = the kth enclosing object of this
/// activation's defining level (`dispatch::kth_enclosing_object`);
/// dispatch privately (direct, non-virtual) if the target is `private` at
/// depth `k`, otherwise virtually via `lookup_protected` on `o`'s actual
/// class -- or, when `o` is a class object, on its metaclass first (§6.3).
pub(crate) fn dispatch_lexical_send(
    interp: &mut Interpreter,
    act: &ActivationRef,
    depth: u16,
    selector: Symbol,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    // The lexical depth counts levels out from the class that *defines* this
    // method, so the walk needs this activation's `current_class`, not the
    // receiver's own class -- see `dispatch::kth_enclosing_object`.
    let (self_value, defining_class) = interp.heap.activation_receiver_and_current_class(act);
    let o =
        dispatch::kth_enclosing_object(&interp.heap, &self_value, defining_class.as_ref(), depth);
    let class = interp.class_of(&o);
    // A `LexicalSend` to a user-defined class's own slot must resolve here,
    // *before* the method/native lookups below -- `build_mixin` never puts a
    // slot's implicit getter/setter into the mixin's method table, so
    // `lookup_defined_for`/`lookup_protected` would never find one.
    // Milestone 1 known gap: unlike a method send, this is never
    // access-controlled by the slot's own declared modifier -- revisit if a
    // fixture needs it.
    if let Some((name, is_setter)) = slot_selector_shape(selector) {
        if let Some(index) = dispatch::lookup_slot(&interp.heap, &class, name) {
            if let Value::Obj(obj) = &o {
                if is_setter {
                    let value = args.first().copied().unwrap_or(Value::Nil);
                    interp.heap.set_instance_field(obj, index, value);
                    return Ok(o);
                } else if let Some(value) = interp.heap.instance_field(obj, index) {
                    return Ok(value);
                }
            }
        }
    }
    // §6.3: a class object's class side is the behaviour of that class
    // object, so a send written inside a class-side method with no explicit
    // receiver has to search the metaclass before the class object's own
    // (native `Object`) behaviour -- the same order `Interpreter::send`
    // gives an explicit-receiver send. Without it a class side reaches only
    // what `lookup_public` on the metaclass happens to answer once the send
    // has fallen all the way through to `send`: its `protected` and
    // `private` members are unreachable, and any selector `Object` answers
    // natively silently wins over the class side's own declaration.
    let metaclass = match &o {
        Value::Obj(obj) if interp.heap.is_class(obj) => interp.metaclass_of(obj),
        _ => None,
    };
    // A metaclass that does not declare the selector falls through to the
    // class object's own behaviour, exactly as in `send`. Only a real
    // declaration stops the search: a metaclass chain carries no builtin
    // tag, so `Lookup::Native` can only come from the class-object chain
    // below and means the same thing either way.
    let lookup = metaclass
        .as_ref()
        .map(|metaclass| lexical_lookup(&interp.heap, metaclass, selector))
        .filter(|found| matches!(found, Lookup::Method(..) | Lookup::NestedClass(..)))
        .unwrap_or_else(|| lexical_lookup(&interp.heap, &class, selector));
    match lookup {
        Lookup::Method(method, defining_class) => {
            interp.invoke_method(o, method, defining_class, args, bindings.clone())
        }
        Lookup::NestedClass(mixin, declaring_class) => {
            crate::interpreter::nested::nested_class(interp, &o, &mixin, &declaring_class, bindings)
        }
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(interp, kind, o, selector, &args) {
                Some(result) => result,
                None => interp.send(o, selector, args, bindings),
            }
        }
        Lookup::NotFound => interp.send(o, selector, args, bindings),
    }
}

/// `newspeak-spec.pdf` §5.10: starts at the superclass of the *current
/// activation's* `current_class`, not `self`'s own class. On success, the
/// new activation's `current_class` is `definingClass(selector, superclass)`
/// -- `invoke_method`'s `defining_class` argument (from `lookup_defined_for`)
/// already carries that.
pub(crate) fn dispatch_super_send(
    interp: &mut Interpreter,
    act: &ActivationRef,
    selector: Symbol,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let Some(current_class) = interp.heap.activation_current_class(act) else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: interp.heap.activation_receiver(act),
            message:
                "super send with no enclosing current_class (used outside a method activation)"
                    .to_string(),
        }));
    };
    let superclass = interp.heap.class_superclass(&current_class);
    let Some(superclass) = superclass else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: interp.heap.activation_receiver(act),
            message: "super send with no superclass (current class is Object or Top)".to_string(),
        }));
    };
    let self_value = interp.heap.activation_receiver(act);
    match dispatch::lookup_defined_for(&interp.heap, &superclass, selector) {
        Lookup::Method(method, defining_class) => {
            interp.invoke_method(self_value, method, defining_class, args, bindings.clone())
        }
        // `super N` for a nested class the superclass declares. The
        // receiver stays `self` (§5.10), which is also the enclosing object
        // the accessor caches against -- so `super N` and `N` name the same
        // class object exactly when they resolve to the same declaration,
        // and different ones when a subclass redeclares `N`: the accessor
        // caches against the declaration's own hidden slot
        // (`MixinApplication::nested_slot`), not against its name.
        Lookup::NestedClass(mixin, declaring_class) => crate::interpreter::nested::nested_class(
            interp,
            &self_value,
            &mixin,
            &declaring_class,
            bindings,
        ),
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(interp, kind, self_value, selector, &args) {
                Some(result) => result,
                None => interp.send(self_value, selector, args, bindings),
            }
        }
        Lookup::NotFound => interp.send(self_value, selector, args, bindings),
    }
}

/// Invokes a closure with `args`. Called by `builtins::block` for `value`,
/// `value:`, `value:value:` and so on, and by every native that runs a block
/// argument, through `builtins::call_block`.
///
/// The invoking activation is not passed: a live activation's sender is the
/// one below it on the activation stack.
pub fn invoke_closure(
    interp: &mut Interpreter,
    closure: &Rc<ClosureData>,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    // A wrong argument count is refused before it can reach a register: the
    // window is laid out from the arguments actually given, so too few would
    // address registers past its end -- a panic -- and too many would
    // silently shift every temporary. Never panic on this
    // Newspeak-program-input path.
    if args.len() != closure.params.len() {
        let receiver = interp.heap.frame_receiver(&closure.captured_frame);
        return Err(Signal::Error(NewspeakError {
            selector: Symbol::intern("value"),
            receiver,
            message: format!(
                "wrong number of block arguments: block takes {}, got {}",
                closure.params.len(),
                args.len()
            ),
        }));
    }
    let receiver = interp.heap.frame_receiver(&closure.captured_frame);
    let current_class = interp.heap.frame_current_class(&closure.captured_frame);
    let code = &closure.code;
    let act = interp.heap.push_activation(
        ActivationHeader {
            receiver,
            current_class,
            kind: FrameKind::Closure(closure.clone()),
            slot_base: 0, // a closure body never contains InitSlot
        },
        &args,
        (code.num_registers - code.num_args) as usize,
    );
    let result = crate::vm::run_code(interp, &act, code, bindings);
    // Popped on every path out. A closure activation catches no return: its
    // `^` targets its home.
    interp.heap.pop_activation(&act);
    result
}

#[cfg(test)]
mod tests {
    use super::parse_int_literal;

    #[test]
    fn plain_decimal_literals_still_parse() {
        assert_eq!(parse_int_literal("42"), Ok(42));
        assert_eq!(parse_int_literal("-42"), Ok(-42));
        assert_eq!(parse_int_literal("0"), Ok(0));
    }

    #[test]
    fn radix_literals_parse_in_every_accepted_shape() {
        assert_eq!(parse_int_literal("16rFF"), Ok(255));
        assert_eq!(parse_int_literal("16r1F"), Ok(31));
        assert_eq!(parse_int_literal("2r1010"), Ok(10));
        assert_eq!(parse_int_literal("36rZ"), Ok(35));
        // `-` on the digits, and `-` on the radix, both mean "negative".
        assert_eq!(parse_int_literal("16r-1F"), Ok(-31));
        assert_eq!(parse_int_literal("-16rFF"), Ok(-255));
    }

    #[test]
    fn a_decimal_literal_out_of_range_says_so() {
        let err = parse_int_literal("99999999999999999999").expect_err("out of range");
        assert!(err.contains("out of i64 range"), "got: {err}");
    }

    #[test]
    fn an_out_of_range_radix_is_rejected_honestly() {
        let err = parse_int_literal("1rZ").expect_err("radix 1");
        assert!(err.contains("must be 2..36"), "got: {err}");
        let err = parse_int_literal("40rZ").expect_err("radix 40");
        assert!(err.contains("must be 2..36"), "got: {err}");
    }

    #[test]
    fn a_digit_outside_the_radix_is_rejected() {
        let err = parse_int_literal("2r1012").expect_err("digit 2 is not base-2");
        assert!(err.contains("malformed"), "got: {err}");
    }

    #[test]
    fn a_fractional_radix_literal_says_it_is_unsupported() {
        let err = parse_int_literal("16r1.8").expect_err("fractional radix");
        assert!(
            err.contains("fractional radix literals are not supported"),
            "got: {err}"
        );
    }
}
