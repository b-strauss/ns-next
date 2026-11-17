//! The exceptions `Kernel.ns` builds over the activation leaves, where a
//! golden cannot reach: the debugger's resumption of an unhandled exception,
//! which `Interpreter::resume_unhandled_exceptions` stands in for
//! (`docs/design/core-library.md`, "Exceptions").

use indoc::indoc;
use ns_runtime::{HostError, HostResult, HostSignal, Interpreter, Lowering, Root, Symbol};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

fn expect_error(result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(error)) => error,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Ok(_) => panic!("{what}: answered, where it should have ended in an error"),
    }
}

const PROBE: &str = indoc! {"
    class Probe = Object new () (
        public probe = ( ^self absent )
        public outOfRange = ( ^{1. 2} at: 5 )
    ) : ()
"};

/// A `Probe` in an interpreter compiling with `lowering`.
fn a_probe(lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(PROBE).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        PROBE.into(),
        ns_runtime::SourceOrigin::File("probe.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    let probe = expect_ok(interpreter.instantiate(&class, &[]), "Probe new");
    (interpreter, probe)
}

ns_runtime::for_each_lowering! {
    /// Squeak's `doesNotUnderstand:`: when the default handler resumed the
    /// `MessageNotUnderstood` -- the debugger, once the missing method exists --
    /// the message is sent again. Here the method still does not exist, so the
    /// re-send fails again, and with the hook spent that second failure ends
    /// the run with the error the first would have ended it with. The hook's
    /// count reaching zero is what shows the first one was resumed and the
    /// message sent again, rather than the run ending at the first.
    fn an_unhandled_message_not_understood_is_resent_when_resumed(lowering) {
        let (mut interpreter, probe) = a_probe(lowering);
        interpreter.resume_unhandled_exceptions(1);
        let error = expect_error(
            interpreter.send(&probe, Symbol::intern("probe"), &[]),
            "probe",
        );
        assert_eq!(
            error.message, "an object does not understand #absent",
            "{lowering:?}"
        );
        assert_eq!(error.selector, Symbol::intern("absent"), "{lowering:?}");
        assert_eq!(
            interpreter.unhandled_exceptions_left_to_resume(),
            0,
            "{lowering:?}: the first doesNotUnderstand: was resumed and the message sent again"
        );
    }
}

ns_runtime::for_each_lowering! {
    /// Without the hook, the first `doesNotUnderstand:` ends the run: nothing
    /// is sent again.
    fn an_unhandled_message_not_understood_ends_the_run(lowering) {
        let (mut interpreter, probe) = a_probe(lowering);
        let error = expect_error(
            interpreter.send(&probe, Symbol::intern("probe"), &[]),
            "probe",
        );
        assert_eq!(
            error.message, "an object does not understand #absent",
            "{lowering:?}"
        );
        let receiver_is_the_probe = interpreter.enter(|mutator| {
            match (
                mutator.fetch(&error.receiver).as_object(),
                mutator.fetch(&probe).as_object(),
            ) {
                (Some(receiver), Some(expected)) => {
                    receiver.same(expected)
                }
                _ => false,
            }
        });
        assert!(
            receiver_is_the_probe,
            "{lowering:?}: the host receives the receiver of the send nothing understood"
        );
    }
}

ns_runtime::for_each_lowering! {
    /// A VM error becomes an `Error`, which is not resumable: when the
    /// default handler resumes it anyway, `signal` returns into the
    /// continuation the VM pushed it with, and the run ends with the error as
    /// it would have.
    fn a_resumed_vm_error_still_ends_the_run(lowering) {
        let (mut interpreter, probe) = a_probe(lowering);
        interpreter.resume_unhandled_exceptions(1);
        let error = expect_error(
            interpreter.send(&probe, Symbol::intern("outOfRange"), &[]),
            "outOfRange",
        );
        assert_eq!(
            error.message, "index 5 out of range: valid indices are 1..2 (size 2)",
            "{lowering:?}"
        );
        assert_eq!(error.selector, Symbol::intern("at:"), "{lowering:?}");
        assert_eq!(
            interpreter.unhandled_exceptions_left_to_resume(),
            0,
            "{lowering:?}: the error reached the default handler once"
        );
    }
}

const DEEP: &str = indoc! {"
    class Deep usingPlatform: thePlatform = Object new (
        | private Error = thePlatform kernel Error.
          private Message = thePlatform kernel Message.
          public deepest ::= 0.
          public reserveDeepest ::= 0.
          public passes ::= 0. |
    ) (
        public probe: n = ( deepest:: n. ^self probe: n + 1 )
        reserveProbe: n = ( reserveDeepest:: n. ^self reserveProbe: n + 1 )
        public descend: n to: bottom = (
            n = bottom ifTrue: [^{1} at: 5].
            ^self descend: n + 1 to: bottom
        )
        public notUnderstood: n to: bottom = (
            n = bottom ifTrue: [^nil foo].
            ^self notUnderstood: n + 1 to: bottom
        )
        public unnamedNotUnderstood: n to: bottom = (
            ^self send: (Message new selector: ('fro' , 'bnicated') asSymbol; arguments: {}) at: n to: bottom
        )
        public unnamedNotUnderstoodInTheReserveAt: bottom = (
            | message = Message new selector: ('fro' , 'bnicated') asSymbol; arguments: {}. |
            ^[[self probe: 1] on: Error do: [:e | self send: message at: 1 to: bottom]] on: Error do: [:e | 7]
        )
        send: message at: n to: bottom = (
            n = bottom ifTrue: [^message sendTo: nil].
            ^self send: message at: n + 1 to: bottom
        )
        descendPassing: n to: bottom = (
            n = bottom ifTrue: [^[{1} at: 5] on: Error do: [:e | passes:: passes + 1. e pass]].
            ^self descendPassing: n + 1 to: bottom
        )
        public passInTheReserveAt: bottom = (
            ^[[self probe: 1] on: Error do: [:e | self descendPassing: 1 to: bottom]] on: Error do: [:e | 7]
        )
        public measureTheReserve = (
            ^[[self probe: 1] on: Error do: [:e | self reserveProbe: 1]] on: Error do: [:e | 7]
        )
        public failInTheReserveAt: bottom = (
            ^[[self probe: 1] on: Error do: [:e | self descend: 1 to: bottom]] on: Error do: [:e | 7]
        )
    ) : ()
"};

/// A `Deep` in an interpreter compiling with `lowering`.
fn a_deep(lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(DEEP).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        DEEP.into(),
        ns_runtime::SourceOrigin::File("deep.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    let platform = interpreter.platform();
    let deep = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "Deep usingPlatform:",
    );
    (interpreter, deep)
}

/// Sends `selector` to `receiver` with the small integers `arguments`.
fn send_integers(
    interpreter: &mut Interpreter,
    receiver: &Root,
    selector: &str,
    arguments: &[i64],
) -> HostResult {
    let arguments: Vec<Root> = arguments
        .iter()
        .map(|&argument| interpreter.stash(ns_runtime::Value::small_integer(argument)))
        .collect();
    interpreter.send(receiver, Symbol::intern(selector), &arguments)
}

/// The small integer the slot accessor `selector` answers.
fn integer_slot(interpreter: &mut Interpreter, receiver: &Root, selector: &str) -> i64 {
    let value = expect_ok(
        interpreter.send(receiver, Symbol::intern(selector), &[]),
        selector,
    );
    interpreter.enter(|mutator| match mutator.fetch(&value).kind() {
        ns_runtime::ValueKind::SmallInteger(integer) => integer,
        _ => panic!("{selector} answers a small integer"),
    })
}

const INDEX_ERROR: &str = "index 5 out of range: valid indices are 1..1 (size 1)";
const DEPTH_ERROR: &str = "stack overflow: recursion too deep";

ns_runtime::for_each_lowering! {
    /// An error raised a few activations below the depth limit, or at it, ends
    /// the run as itself when nothing handles it: its `signal`, the handler
    /// search and the default action run in the reserve above the limit, so
    /// none of them meets the limit and replaces the error with the limit's.
    /// `descend:to:` reaches the depth `probe:` measured, level for level; one
    /// level further it is the limit that fails.
    fn an_unhandled_error_just_below_the_depth_limit_ends_the_run_as_itself(lowering) {
        let (mut interpreter, deep) = a_deep(lowering);
        let error = expect_error(
            send_integers(&mut interpreter, &deep, "probe:", &[1]),
            "probe:",
        );
        assert_eq!(error.message, DEPTH_ERROR, "{lowering:?}");
        let deepest = integer_slot(&mut interpreter, &deep, "deepest");
        for (bottom, expected) in [
            (deepest - 3, INDEX_ERROR),
            (deepest - 1, INDEX_ERROR),
            (deepest, INDEX_ERROR),
            (deepest + 1, DEPTH_ERROR),
        ] {
            let error = expect_error(
                send_integers(&mut interpreter, &deep, "descend:to:", &[1, bottom]),
                "descend:to:",
            );
            assert_eq!(
                error.message,
                expected,
                "{lowering:?}: an error {} levels below the limit",
                deepest - bottom
            );
            if expected == INDEX_ERROR {
                assert_eq!(error.selector, Symbol::intern("at:"), "{lowering:?}");
            }
        }
    }
}

ns_runtime::for_each_lowering! {
    /// While the reserve is held, an error raised by a handler is signalled in
    /// what is left of it. With room, it is looked for outside the handler's
    /// `on:do:` and caught there. So near the reserve's end that its own
    /// `signal` or handler search cannot run, the run ends with that error --
    /// the one being signalled -- and not with the depth limit its signalling
    /// met; a handler that itself recurses past the reserve ends the run with
    /// the depth limit's error.
    fn an_error_whose_signalling_overruns_the_reserve_ends_the_run_as_itself(lowering) {
        let (mut interpreter, deep) = a_deep(lowering);
        let error = expect_error(
            interpreter.send(&deep, Symbol::intern("measureTheReserve"), &[]),
            "measureTheReserve",
        );
        assert_eq!(
            error.message, DEPTH_ERROR,
            "{lowering:?}: a handler recursing past the reserve"
        );
        let reserve_deepest = integer_slot(&mut interpreter, &deep, "reserveDeepest");
        let caught = expect_ok(
            send_integers(
                &mut interpreter,
                &deep,
                "failInTheReserveAt:",
                &[reserve_deepest - 500],
            ),
            "failInTheReserveAt: with room",
        );
        let caught_outside = interpreter
            .enter(|mutator| mutator.fetch(&caught).as_small_integer() == Some(7));
        assert!(
            caught_outside,
            "{lowering:?}: with room, the outer handler catches the error"
        );
        for bottom in [reserve_deepest - 3, reserve_deepest - 2, reserve_deepest - 1] {
            let error = expect_error(
                send_integers(&mut interpreter, &deep, "failInTheReserveAt:", &[bottom]),
                "failInTheReserveAt:",
            );
            assert_eq!(
                error.message,
                INDEX_ERROR,
                "{lowering:?}: an error {} levels below the reserve's end",
                reserve_deepest - bottom
            );
            assert_eq!(error.selector, Symbol::intern("at:"), "{lowering:?}");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// §5.5 makes a message nothing understood the VM's error, so it takes the
    /// reserve as any VM error does: raised a few levels below the depth limit,
    /// or at it, and unhandled, it ends the run as itself, with the selector and
    /// receiver of the send nothing understood.
    fn an_unhandled_does_not_understand_just_below_the_depth_limit_ends_the_run_as_itself(lowering) {
        let (mut interpreter, deep) = a_deep(lowering);
        let error = expect_error(
            send_integers(&mut interpreter, &deep, "probe:", &[1]),
            "probe:",
        );
        assert_eq!(error.message, DEPTH_ERROR, "{lowering:?}");
        let deepest = integer_slot(&mut interpreter, &deep, "deepest");
        for (bottom, expected) in [
            (deepest - 3, "an object does not understand #foo"),
            (deepest - 1, "an object does not understand #foo"),
            (deepest, "an object does not understand #foo"),
            (deepest + 1, DEPTH_ERROR),
        ] {
            let error = expect_error(
                send_integers(&mut interpreter, &deep, "notUnderstood:to:", &[1, bottom]),
                "notUnderstood:to:",
            );
            assert_eq!(
                error.message,
                expected,
                "{lowering:?}: a doesNotUnderstand: {} levels below the limit",
                deepest - bottom
            );
            if expected != DEPTH_ERROR {
                assert_eq!(error.selector, Symbol::intern("foo"), "{lowering:?}");
            }
        }
    }
}

ns_runtime::for_each_lowering! {
    /// The search a `pass` starts is the kernel's, as the first one is: an
    /// overrun of the reserve during it ends the run with the error being
    /// passed, not the depth limit. Near the reserve's end the passing
    /// handler runs (`passes` counts it) and its `pass` cannot finish; at some
    /// such depth the run must end with the index error the handler passed.
    fn a_pass_that_overruns_the_reserve_ends_the_run_with_the_passed_error(lowering) {
        let (mut interpreter, deep) = a_deep(lowering);
        let error = expect_error(
            interpreter.send(&deep, Symbol::intern("measureTheReserve"), &[]),
            "measureTheReserve",
        );
        assert_eq!(error.message, DEPTH_ERROR, "{lowering:?}");
        let reserve_deepest = integer_slot(&mut interpreter, &deep, "reserveDeepest");
        // From the reserve's end outwards, stopping at the first depth that
        // shows it: nearer the end the handler never runs, further out the
        // `pass` finds the outer handler.
        let overran_while_passing = (0..60).any(|offset| {
            let passes_before = integer_slot(&mut interpreter, &deep, "passes");
            let result = send_integers(
                &mut interpreter,
                &deep,
                "passInTheReserveAt:",
                &[reserve_deepest - offset],
            );
            let passed = integer_slot(&mut interpreter, &deep, "passes") > passes_before;
            matches!(result, Err(HostSignal::Error(error)) if passed && error.message == INDEX_ERROR)
        });
        assert!(
            overran_while_passing,
            "{lowering:?}: some depth ends the run with the passed error once the handler ran"
        );
    }
}

const ESCAPER: &str = indoc! {"
    class Escaper = Object new () (
        escapingBlock = ( ^[:value | ^value] )
        public escape = ( ^self escapingBlock value: 3 )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A block's `^` whose home has returned -- its frame no longer married
    /// to a stack frame, nor suspended -- is still §7.2's `cannotReturn`:
    /// only a running home, live or suspended, is unwound to.
    fn a_return_into_a_returned_home_is_still_cannot_return(lowering) {
        let module = ns_syntax::parse(ESCAPER).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            ESCAPER.into(),
            ns_runtime::SourceOrigin::File("escaper.ns".into()),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let escaper = expect_ok(interpreter.instantiate(&class, &[]), "Escaper new");
        let error = expect_error(
            interpreter.send(&escaper, Symbol::intern("escape"), &[]),
            "escape",
        );
        assert_eq!(
            error.message, "cannotReturn: the enclosing method activation has already returned",
            "{lowering:?}"
        );
    }
}

const SHAPES: &str = indoc! {"
    class Shapes = Object new () (
        class Shape = Object new () (
            public area = ( ^self subclassResponsibility )
        )
        class Circle = Shape new () (
        )
        public circleArea = ( ^Circle new area )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `subclassResponsibility` names the abstract method through the
    /// activation leaf `activationMethodName:ifFail:`: the class declaring
    /// the method that sent it and its selector, read off the sending
    /// activation, then the class of the receiver.
    fn subclass_responsibility_names_the_sending_method(lowering) {
        let module = ns_syntax::parse(SHAPES).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            SHAPES.into(),
            ns_runtime::SourceOrigin::File("shapes.ns".into()),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let shapes = expect_ok(interpreter.instantiate(&class, &[]), "Shapes new");
        let error = expect_error(
            interpreter.send(&shapes, Symbol::intern("circleArea"), &[]),
            "circleArea",
        );
        assert_eq!(
            error.message, "Shape>>area is abstract: Circle must implement it",
            "{lowering:?}"
        );
    }
}

const SIGNALLER: &str = indoc! {"
    class Signaller = Object new () (
        public catchFrom: depth = (
            ^[self descend: depth] on: Error do: [:error | error return: 1]
        )
        descend: depth = (
            depth = 0 ifTrue: [^Error new signal].
            ^self descend: depth - 1
        )
        public returnFrom: depth = (
            ^[self quiet: depth] on: Error do: [:error | error return: 1]
        )
        quiet: depth = (
            depth = 0 ifTrue: [^1].
            ^self quiet: depth - 1
        )
        public catchInHandler = (
            ^[[Error new signal] on: Error do: [:error | Error new signal]]
                on: Error
                do: [:error | error return: 2]
        )
    ) : ()
"};

/// A `Signaller` in an interpreter compiling with `lowering`, with every
/// stress mode that allocates or frees at a safepoint turned off: a
/// collection or a scavenge would change the count of objects in the
/// space, a suspension reifies every frame it suspends, and the pause
/// stress mode's evaluations reify the frame they stand in and make their
/// closures.
fn a_signaller(lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(SIGNALLER).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        SIGNALLER.into(),
        ns_runtime::SourceOrigin::File("signaller.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    // The stress modes exist only in a debug build.
    #[cfg(debug_assertions)]
    {
        interpreter.set_stress_collect(false);
        interpreter.set_stress_scavenge(false);
        interpreter.set_stress_suspend(false);
        interpreter.set_stress_pause(false);
    }
    let class = interpreter.load_module(&resolved);
    let signaller = expect_ok(interpreter.instantiate(&class, &[]), "Signaller new");
    (interpreter, signaller)
}

/// The small integer `result` holds.
fn small_integer(interpreter: &mut Interpreter, result: &Root, what: &str) -> i64 {
    interpreter.enter(|mutator| match mutator.fetch(result).kind() {
        ns_runtime::ValueKind::SmallInteger(integer) => integer,
        _ => panic!("{what} answers a small integer"),
    })
}

/// How many objects one send of `selector` with `depth` adds to the
/// space, and what it answers.
fn objects_made_by(
    interpreter: &mut Interpreter,
    signaller: &Root,
    selector: &str,
    depth: i64,
) -> (usize, i64) {
    let before = interpreter.heap_statistics().live_objects;
    let result = expect_ok(
        send_integers(interpreter, signaller, selector, &[depth]),
        selector,
    );
    let after = interpreter.heap_statistics().live_objects;
    (
        after
            .checked_sub(before)
            .expect("no collection ran during the measured send"),
        small_integer(interpreter, &result, selector),
    )
}

/// How many more objects a signal caught `depth` activations down adds
/// than the same descent returning normally: what the signal and its
/// handler search allocate, without what the descent allocates itself --
/// under the reference lowering a closure and a reified frame per
/// activation, for the `ifTrue:` it does not inline.
fn objects_a_signal_makes(interpreter: &mut Interpreter, signaller: &Root, depth: i64) -> usize {
    let (signalling, caught) = objects_made_by(interpreter, signaller, "catchFrom:", depth);
    let (returning, returned) = objects_made_by(interpreter, signaller, "returnFrom:", depth);
    assert_eq!((caught, returned), (1, 1), "both sends answer 1");
    signalling - returning
}

ns_runtime::for_each_lowering! {
    /// The handler search walks the frames itself and reifies only the
    /// `on:do:` activation it answers, so what a signal allocates does not
    /// grow with the frames between it and its handler.
    fn a_handler_search_reifies_no_frame_it_passes(lowering) {
        let (mut interpreter, signaller) = a_signaller(lowering);
        // Fills every cache the sends below take, so neither measured
        // send allocates anything the other does not.
        objects_a_signal_makes(&mut interpreter, &signaller, 5);
        let objects_at_10 = objects_a_signal_makes(&mut interpreter, &signaller, 10);
        let objects_at_200 = objects_a_signal_makes(&mut interpreter, &signaller, 200);
        assert_eq!(
            objects_at_200, objects_at_10,
            "{lowering:?}: a signal's allocation grows with the frames its search passes"
        );
    }
}

ns_runtime::for_each_lowering! {
    /// An `on:do:` whose handler is running is disabled, so a signal its
    /// handler raises passes it and reaches the one outside.
    fn a_handler_search_skips_a_disabled_handler(lowering) {
        let (mut interpreter, signaller) = a_signaller(lowering);
        let result = expect_ok(
            interpreter.send(&signaller, Symbol::intern("catchInHandler"), &[]),
            "catchInHandler",
        );
        assert_eq!(
            small_integer(&mut interpreter, &result, "catchInHandler"),
            2,
            "{lowering:?}: the outer handler answers"
        );
    }
}

ns_runtime::for_each_lowering! {
    /// Under the suspend stress mode every activation below the running
    /// one lives in its frame at the safepoints the schedule picks, so the
    /// handler search walks from a live frame into suspended ones and
    /// finds its handler there, an enabled one and past a disabled one.
    fn a_handler_search_finds_a_handler_in_a_suspended_frame(lowering) {
        let (mut interpreter, signaller) = a_signaller(lowering);
        // The stress modes exist only in a debug build; a release build
        // runs the same sends unsuspended.
        #[cfg(debug_assertions)]
        interpreter.set_stress_suspend(true);
        let caught = expect_ok(
            send_integers(&mut interpreter, &signaller, "catchFrom:", &[10]),
            "catchFrom:",
        );
        assert_eq!(
            small_integer(&mut interpreter, &caught, "catchFrom:"),
            1,
            "{lowering:?}: the handler in a suspended frame answers"
        );
        let outer = expect_ok(
            interpreter.send(&signaller, Symbol::intern("catchInHandler"), &[]),
            "catchInHandler",
        );
        assert_eq!(
            small_integer(&mut interpreter, &outer, "catchInHandler"),
            2,
            "{lowering:?}: the search passes the disabled handler in a suspended frame"
        );
    }
}

ns_runtime::for_each_lowering! {
    /// An unhandled `doesNotUnderstand:` of a program-made selector whose
    /// text nothing has interned reaches the host with `doesNotUnderstand:`
    /// as its selector and the text in its message, whether the kernel's
    /// `defaultAction` ends the run (far from the depth limit) or the depth
    /// limit's report does (at the reserve's end, where the message's
    /// signalling overruns it) -- and neither interns the text.
    fn an_unhandled_unnamed_selector_is_reported_as_does_not_understand_on_both_paths(lowering) {
        let (mut interpreter, deep) = a_deep(lowering);
        let expected = "an object does not understand #frobnicated";
        let error = expect_error(
            send_integers(&mut interpreter, &deep, "unnamedNotUnderstood:to:", &[1, 1]),
            "unnamedNotUnderstood:to:",
        );
        assert_eq!(error.message, expected, "{lowering:?}: defaultAction");
        assert_eq!(
            error.selector,
            Symbol::DOES_NOT_UNDERSTAND,
            "{lowering:?}: defaultAction"
        );
        let _ = expect_error(
            interpreter.send(&deep, Symbol::intern("measureTheReserve"), &[]),
            "measureTheReserve",
        );
        let reserve_deepest = integer_slot(&mut interpreter, &deep, "reserveDeepest");
        for bottom in [reserve_deepest - 3, reserve_deepest - 2, reserve_deepest - 1] {
            let error = expect_error(
                send_integers(
                    &mut interpreter,
                    &deep,
                    "unnamedNotUnderstoodInTheReserveAt:",
                    &[bottom],
                ),
                "unnamedNotUnderstoodInTheReserveAt:",
            );
            assert_eq!(error.message, expected, "{lowering:?}: the depth limit's report");
            assert_eq!(
                error.selector,
                Symbol::DOES_NOT_UNDERSTAND,
                "{lowering:?}: the depth limit's report"
            );
        }
        assert_eq!(Symbol::lookup("frobnicated"), None, "{lowering:?}");
    }
}
