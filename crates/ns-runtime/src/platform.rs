//! The transcript's output and the monotonic clock, the two parts of the
//! platform that reach Rust. The platform is an instance of the shipped
//! `PlatformFactory.ns` module's nested `Platform`
//! (`docs/design/core-library.md`, "The platform"): its members are lazy
//! slots holding values of the module's nested classes and the facet of
//! the shipped `CollectionsFactory.ns` module, whose `List` is Newspeak and
//! needs no leaf; that module is reached through the mirror's
//! `moduleInstanceNamed:` (`builtins::module`). What is left here is
//! `show:` and `newLine`, the forwarders of `Transcript`'s two methods, and
//! the clock `Stopwatch` reads, nanoseconds since one origin per process,
//! so that two interpreters in one process agree.
//!
//! The leaves read nothing off their receiver; `show:` reads only its string
//! argument. The capability is the VM mirror the forwarder sends to, which only shipped
//! source reaches (`docs/design/core-library.md`, "The VM mirror is the
//! whole Rust boundary"), not the receiver's class: `Transcript` is no
//! builtin, so the VM could not recognize its class anyway, and a user
//! subclass of it inherits a working `show:`.
//!
//! `show:` writes a string and nothing else: any other argument is an
//! ordinary Newspeak error rather than nothing written, and `print:`, a
//! Newspeak method beside it, is the message that writes any object's
//! `printString` (`docs/design/core-library.md`, "The platform").
//!
//! The first write that finds the standard output closed -- a pipeline's
//! consumer gone, as `ns-next run Spin.ns | head -1` leaves it -- ends the
//! run as `terminate` does, every pending `ensure:` block run, and every
//! later write of the process is dropped: nobody reads, so a program that
//! went on would compute for nobody, and an `Error` a program catches
//! would let it. The host reports it as every terminated run, with status
//! 1, which `set -o pipefail` sees as it sees a producer a SIGPIPE ended.

use crate::interpreter::{Arguments, Signal};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::arbitrary_integer::{high_digit, low_digit};
use crate::builtins::first_argument;
use crate::builtins::primitive::PlatformPrimitive;
use crate::types::*;
use crate::{ArbitraryInteger, Mutator, Symbol};

/// Runs `primitive` for whatever receiver the forwarder passed, which it
/// answers. It never declines: an argument `show:` cannot write is an
/// error of its own, so the forwarder's failure block never runs, and a
/// closed standard output is a termination.
///
/// Never inlined: `builtins::execute`, which every leaf enters, would
/// take on the stack traffic of the writes and of the termination's boxed
/// error, wide loads on the hottest leaves' path for a send that is an
/// exit anyway (`builtins::execute`, whose budget `xtask/src/codegen.rs`
/// holds).
#[inline(never)]
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, Signal<'gc>>> {
    Some(stream(mutator, primitive, receiver, selector, arguments))
}

/// Set by the first write that found the standard output closed, and never
/// cleared: every later write of the process is dropped, so that neither
/// the `ensure:` blocks the termination runs nor a later run in the
/// process can raise it again. Process-wide, as the stream is.
static STANDARD_OUTPUT_CLOSED: AtomicBool = AtomicBool::new(false);

/// The message of the termination a closed standard output answers.
const STANDARD_OUTPUT_CLOSED_MESSAGE: &str = "the standard output is closed";

/// Writes `bytes` to the standard output and flushes it, unless it was
/// found closed before. A closed stream -- `BrokenPipe`, which Windows'
/// error 232 maps to as well -- ends the run; any other error is ignored,
/// as a failed flush always was.
///
/// Machine code runs both transcript rows through an exit, never as a leaf
/// (`builtins/primitive.rs`, the rows `transcript:show:ifFail:` and
/// `transcriptNewLine:ifFail:`): a leaf's signal hands the send to the
/// interpreter, which runs it again from the start, and the send run again
/// would find the flag set and be dropped, so the run would never end.
fn write_standard_output<'gc>(
    bytes: &[u8],
    receiver: Value<'gc>,
    selector: Symbol,
) -> Result<Value<'gc>, Signal<'gc>> {
    if STANDARD_OUTPUT_CLOSED.load(Ordering::Relaxed) {
        return Ok(receiver);
    }
    match write_and_flush(bytes) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
            STANDARD_OUTPUT_CLOSED.store(true, Ordering::Relaxed);
            Err(Signal::Terminate(Box::new(NewspeakError::error(
                selector,
                receiver,
                STANDARD_OUTPUT_CLOSED_MESSAGE,
            ))))
        }
        _ => Ok(receiver),
    }
}

/// The one write of `bytes` and its flush on the locked standard output.
fn write_and_flush(bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    if tests::STANDARD_OUTPUT_CLOSED_FOR_TESTING.get() {
        return Err(std::io::ErrorKind::BrokenPipe.into());
    }
    let mut output = std::io::stdout().lock();
    output.write_all(bytes)?;
    output.flush()
}

/// The instant every monotonic reading is measured from, taken on the
/// first reading. One per process, so two interpreters' stopwatches agree.
static CLOCK_ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn stream<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Result<Value<'gc>, Signal<'gc>> {
    match primitive {
        PlatformPrimitive::Show => {
            let text = arguments
                .first(&mutator.heap)
                .and_then(|value| value.as_object())
                .and_then(|string| mutator.heap().str_value(string));
            let Some(text) = text else {
                return Err(Signal::Error(NewspeakError::argument_error(
                    selector,
                    receiver,
                    first_argument(&mutator.heap, arguments),
                    format!(
                        "{selector} requires a String, got {}",
                        mutator
                            .heap()
                            .describe_argument(arguments.first(&mutator.heap))
                    ),
                )));
            };
            write_standard_output(text.as_bytes(), receiver, selector)
        }
        PlatformPrimitive::NewLine => write_standard_output(b"\n", receiver, selector),
        // Nanoseconds since the process's origin, exactly: a `SmallInteger`
        // holds 36 years of them, and an integer is unbounded beyond, so
        // the `u128` is converted whole, as its two `u64` digits, rather than
        // saturated.
        PlatformPrimitive::MonotonicNanoseconds => {
            let elapsed = CLOCK_ORIGIN.get_or_init(std::time::Instant::now).elapsed();
            let nanoseconds = elapsed.as_nanos();
            Ok(mutator.new_integer(ArbitraryInteger::from_parts(
                false,
                vec![low_digit(nanoseconds), high_digit(nanoseconds)],
            )))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::heap::ValueKind;
    use crate::{HostSignal, Interpreter, SourceOrigin};
    use indoc::indoc;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::rc::Rc;

    thread_local! {
        /// Makes every write of this thread's interpreter find the
        /// standard output closed, which the test process's own stream
        /// never is.
        pub(crate) static STANDARD_OUTPUT_CLOSED_FOR_TESTING: Cell<bool> =
            const { Cell::new(false) };
    }

    /// Writes for ever inside an `ensure:` whose block writes again and
    /// records that it ran to its end.
    const ENSURING_WRITER: &str = indoc! {"
        class EnsuringWriter = Object new (
          | ensureRan ::= false. |
        ) (
          public run: platform = (
            [[true] whileTrue: [platform transcript show: 'line'; newLine]]
              ensure: [
                platform transcript show: 'ensured'; newLine.
                ensureRan:: true].
          )

          public ensured = (
            ^ensureRan
          )
        ) : ()
    "};

    /// The first write that finds the standard output closed terminates
    /// the run, the pending `ensure:` block runs to its end, and its own
    /// write is dropped rather than raising the termination again.
    #[test]
    fn a_closed_standard_output_terminates_the_run_after_its_ensure_blocks() {
        let module = ns_syntax::parse(ENSURING_WRITER).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(ENSURING_WRITER),
            SourceOrigin::File(PathBuf::from("EnsuringWriter.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let instance = interpreter
            .instantiate(&class, &[])
            .ok()
            .expect("an EnsuringWriter");
        let platform = interpreter.platform();
        STANDARD_OUTPUT_CLOSED_FOR_TESTING.set(true);
        let result = interpreter.send(&instance, Symbol::intern("run:"), &[platform]);
        STANDARD_OUTPUT_CLOSED_FOR_TESTING.set(false);
        let closed = STANDARD_OUTPUT_CLOSED.swap(false, Ordering::Relaxed);
        match result {
            Err(HostSignal::Error(error)) => {
                assert_eq!(&*error.message, STANDARD_OUTPUT_CLOSED_MESSAGE);
            }
            _ => panic!("the run was not terminated"),
        }
        assert!(closed, "the closed stream was recorded");
        let ensured = match interpreter.send(&instance, Symbol::intern("ensured"), &[]) {
            Ok(ensured) => ensured,
            Err(_) => panic!("ensured failed"),
        };
        let ensured = interpreter
            .enter(|mutator| matches!(mutator.fetch(&ensured).kind(), ValueKind::Boolean(true)));
        assert!(ensured, "the ensure: block ran to its end");
    }
}
