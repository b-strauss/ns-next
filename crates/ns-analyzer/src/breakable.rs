//! The lines a line breakpoint can stand on, and the block bodies that
//! begin on each, for the custom request `newspeak/breakableLines`: the
//! JetBrains plugin decides `canPutAt` and a line breakpoint's variants
//! from them before any debug session runs, when no agent can be asked
//! (`docs/design/editor-support.md`, "Breakable lines";
//! `docs/design/debugger.md`, "A breakpoint names a source position").

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use lsp_types::{Range, Uri};
use ns_runtime::{BreakableStatement, Interpreter, SourceOrigin};
use ns_syntax::ast;
use serde_json::{Value, json};

use crate::analysis::panic_detail;
use crate::documents::LineIndex;
use crate::file_uri::file_path_of;

/// The method of the request.
pub const METHOD: &str = "newspeak/breakableLines";

/// The most sends a path through a module's tree may nest -- a chain of
/// messages, `1 + 1 + …`, nests one per message -- for its breakable
/// lines to be compiled; past it the answer is `null`. The compiler
/// recurses along a chain, and a chain is the one nesting the parser does
/// not bound (`docs/design/frontend.md`, "Recovery", rule 7), so without
/// the cap a long enough chain overflows the stack, which aborts the
/// server, and the editor's support for every file with it, where
/// `catch_unwind` cannot help. `ns-next run`'s 8 MiB thread overflows
/// compiling a body of one chain past about 14,900 messages in a release
/// build and 10,000 in a debug one, so every module `ns-next` compiles
/// today is under the cap and gets its exact answer; the worker's
/// `WORKER_STACK_BYTES` hold a chain of this length in a debug build with
/// room to spare. The cap goes when the compiler takes a chain link by
/// link (`docs/OPEN-WORK.md`, "A valid method whose body is one long
/// chain of messages aborts `ns-next run`").
pub const MAX_NESTED_SENDS: usize = 20_000;

/// The worker's stack: reserved, not committed, so its size costs address
/// space only. Eight times the 8 MiB `ns-next` compiles on, so that a
/// module of `MAX_NESTED_SENDS` compiles in a debug build, whose frames
/// are larger.
const WORKER_STACK_BYTES: usize = 64 * 1024 * 1024;

/// A request to the worker: a document's text and the origin it names.
struct Job {
    text: String,
    origin: PathBuf,
}

/// The thread every document's breakable statements are compiled on: its
/// stack is what `MAX_NESTED_SENDS` is measured against, and the server's
/// own thread keeps the analyzer's 8 MiB. It owns its interpreter, since
/// an interpreter and the symbol interner are bound to the thread that
/// made them; nothing but spans comes back.
struct Worker {
    jobs: Option<Sender<Job>>,
    answers: Receiver<Option<Vec<BreakableStatement>>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn() -> Option<Worker> {
        let (jobs, job_receiver) = channel::<Job>();
        let (answer_sender, answers) = channel();
        let thread = std::thread::Builder::new()
            .name("ns-next-analyzer breakable lines".to_string())
            .stack_size(WORKER_STACK_BYTES)
            .spawn(move || serve(&job_receiver, &answer_sender))
            .ok()?;
        Some(Worker {
            jobs: Some(jobs),
            answers,
            thread: Some(thread),
        })
    }

    /// The worker's answer for `job`, or `None` for a worker that has
    /// gone.
    fn ask(&self, job: Job) -> Option<Option<Vec<BreakableStatement>>> {
        self.jobs.as_ref()?.send(job).ok()?;
        self.answers.recv().ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the channel ends the worker's loop.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The worker's loop. The interpreter is bootstrapped by the first module
/// that needs one rather than at the server's start, so that an editor
/// that never asks pays nothing, and kept, since a bootstrap per request
/// would cost more than the compile. It instantiates the shipped core
/// library and nothing else: `Interpreter::breakable_statements` compiles
/// the document's module without loading or running it, and
/// `Interpreter::shipped_breakable_statements` a shipped module's, which
/// stays unbuilt if it is a lazy one. A panic in the
/// compile is an ns-next defect; it answers `None`, and drops the
/// interpreter, whose state it may have left half changed, for a new one
/// at the next request.
fn serve(jobs: &Receiver<Job>, answers: &Sender<Option<Vec<BreakableStatement>>>) {
    let mut interpreter: Option<Interpreter> = None;
    for job in jobs {
        let answered = catch_unwind(AssertUnwindSafe(|| {
            // A document holding a shipped module's text, byte for byte,
            // is the shipped module: compiled from the binary's own text
            // and resolution, as the VM compiles it, so that the answer is
            // where a breakpoint set in it lands. The document's text only
            // selects the module, so no text a user writes is ever
            // resolved as shipped code.
            if let Some(name) = ns_runtime::shipped_module_named_by_text(&job.text) {
                let interpreter = interpreter.get_or_insert_with(Interpreter::bootstrap);
                return interpreter.shipped_breakable_statements(name);
            }
            let module = ns_syntax::parse(&job.text).ok()?;
            if nested_sends(&module.root_class) > MAX_NESTED_SENDS {
                return None;
            }
            let resolved = ns_runtime::resolve(
                module,
                job.text.as_str().into(),
                SourceOrigin::File(job.origin.clone()),
            )
            .ok()?;
            let interpreter = interpreter.get_or_insert_with(Interpreter::bootstrap);
            Some(interpreter.breakable_statements(&resolved))
        }));
        let answer = answered.unwrap_or_else(|payload| {
            interpreter = None;
            eprintln!(
                "internal error in ns-next-analyzer answering {METHOD} for {}: {}; this is an ns-next defect, please report it",
                job.origin.display(),
                panic_detail(payload.as_ref())
            );
            None
        });
        if answers.send(answer).is_err() {
            return;
        }
    }
}

/// The server's end of the worker, which it spawns at the first request.
#[derive(Default)]
pub struct Breakable {
    worker: Option<Worker>,
}

impl Breakable {
    /// The `lines` of the request's answer for `text`: one entry per line
    /// a breakable statement starts on, in order, each with its variants
    /// -- the block literal's range and its first statement's position,
    /// for each statement starting on the line that is a block's first --
    /// or `null` for a text that does not parse or resolve, or nests more
    /// than `MAX_NESTED_SENDS` sends, every line of which the plugin
    /// allows, and for a worker that could not be started or has gone.
    pub fn lines(&mut self, uri: &Uri, text: &str, line_index: &LineIndex) -> Value {
        if self.worker.is_none() {
            self.worker = Worker::spawn();
        }
        let Some(worker) = &self.worker else {
            return Value::Null;
        };
        let job = Job {
            text: text.to_string(),
            origin: origin_of(uri),
        };
        match worker.ask(job) {
            Some(Some(statements)) => encode(&statements, text, line_index),
            Some(None) => Value::Null,
            None => {
                // Spawned afresh at the next request.
                self.worker = None;
                Value::Null
            }
        }
    }
}

/// The origin a module compiled from `uri` names: the file, or the URI
/// itself for one that names no file. Nothing of the compile reads it
/// but a message, and nothing is loaded.
fn origin_of(uri: &Uri) -> PathBuf {
    file_path_of(uri).unwrap_or_else(|| PathBuf::from(uri.as_str()))
}

/// The most sends any path through `root`'s declaration and every
/// declaration nested in it nests, each message of a chain one: what the
/// compiler's recursion along receivers and arguments goes as deep as.
/// A worklist rather than a recursion, each chain taken from
/// `Send::receiver_chain`, since a chain is as deep as the source is
/// long; every other nesting is the parser's to bound.
pub fn nested_sends(root: &ast::ClassDeclaration) -> usize {
    let mut expressions: Vec<(&ast::Expression, usize)> = Vec::new();
    let mut declarations = vec![root];
    while let Some(declaration) = declarations.pop() {
        declarations.extend(declaration.instance_side.nested_classes());
        let clauses = declaration
            .superclass
            .iter()
            .chain(&declaration.mixin_applications);
        for clause in clauses {
            let depth = clause.messages.len();
            expressions.extend(
                clause
                    .messages
                    .iter()
                    .flat_map(|message| &message.arguments)
                    .map(|argument| (argument, depth)),
            );
        }
        let sides = std::iter::once(&declaration.instance_side).chain(&declaration.class_side);
        for side in sides {
            expressions.extend(
                side.slots()
                    .filter_map(|slot| slot.initializer.as_ref())
                    .map(|initializer| (initializer, 0)),
            );
            expressions.extend(
                side.initializer_expressions
                    .iter()
                    .map(|statement| (statement_expression(statement), 0)),
            );
            for method in side.methods() {
                push_body(&mut expressions, &method.body, 0);
            }
        }
    }
    let mut deepest = 0;
    while let Some((expression, depth)) = expressions.pop() {
        match &expression.kind {
            ast::ExpressionKind::Identifier(_) | ast::ExpressionKind::Literal(_) => {}
            ast::ExpressionKind::Send(send) => {
                let (links, receiver) = send.receiver_chain();
                // The outermost link nests one send, the receiver all of
                // them.
                for (index, link) in links.iter().enumerate() {
                    let link_depth = depth + index + 1;
                    expressions.extend(
                        link.message
                            .arguments
                            .iter()
                            .map(|argument| (argument, link_depth)),
                    );
                }
                let chain_depth = depth + links.len();
                deepest = deepest.max(chain_depth);
                expressions.push((receiver, chain_depth));
            }
            ast::ExpressionKind::Cascade(cascade) => {
                expressions.push((&cascade.receiver, depth));
                let messages = std::iter::once(&cascade.first).chain(&cascade.rest);
                deepest = deepest.max(depth + 1);
                expressions.extend(
                    messages
                        .flat_map(|message| &message.arguments)
                        .map(|argument| (argument, depth + 1)),
                );
            }
            ast::ExpressionKind::Assign { value, .. } => expressions.push((value, depth)),
            ast::ExpressionKind::Block(block) => push_body(&mut expressions, &block.body, depth),
            ast::ExpressionKind::DynamicArray { elements, .. } => {
                expressions.extend(elements.iter().map(|element| (element, depth)));
            }
        }
    }
    deepest
}

/// The expressions of `body`, at `depth`: its temporaries' initializers
/// and its statements'.
fn push_body<'module>(
    expressions: &mut Vec<(&'module ast::Expression, usize)>,
    body: &'module ast::CodeBody,
    depth: usize,
) {
    expressions.extend(
        body.temporaries
            .iter()
            .filter_map(|temporary| temporary.initializer.as_ref())
            .map(|initializer| (initializer, depth)),
    );
    expressions.extend(
        body.statements
            .iter()
            .map(|statement| (statement_expression(statement), depth)),
    );
}

fn statement_expression(statement: &ast::Statement) -> &ast::Expression {
    match &statement.kind {
        ast::StatementKind::Expression(expression) | ast::StatementKind::Return(expression) => {
            expression
        }
    }
}

/// `statements`, sorted by start, grouped by the line each starts on.
fn encode(statements: &[BreakableStatement], text: &str, line_index: &LineIndex) -> Value {
    let mut lines: Vec<(u32, Vec<Value>)> = Vec::new();
    for statement in statements {
        let position = line_index.position(text, statement.statement.start);
        if lines.last().is_none_or(|(line, _)| *line != position.line) {
            lines.push((position.line, Vec::new()));
        }
        if let (Some(block), Some((_, variants))) = (statement.block, lines.last_mut()) {
            let range = Range::new(
                line_index.position(text, block.start),
                line_index.position(text, block.end),
            );
            variants.push(json!({ "range": range, "position": position }));
        }
    }
    Value::Array(
        lines
            .into_iter()
            .map(|(line, variants)| json!({ "line": line, "variants": variants }))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::nested_sends;

    fn nested_sends_of(text: &str) -> usize {
        let module = ns_syntax::parse(text).expect("the module parses");
        nested_sends(&module.root_class)
    }

    /// A chain nests a send per message, an argument's sends nest below
    /// the link that takes the argument, and a block's below the send it
    /// is an argument of.
    #[test]
    fn nested_sends_count_every_message_along_the_deepest_path() {
        let text = indoc::indoc! {"
            class Module = Object new () (
              public flat = ( ^1 + 1 + 1 + 1 )
              public nested = ( ^1 + (2 * (3 - [4 negated abs] value)) )
            ) : ()
        "};
        // `nested`: `+`, `*`, `-`, `value`, then `abs` and `negated`.
        assert_eq!(nested_sends_of(text), 6);
        let chain = indoc::formatdoc! {"
            class Module = Object new () (
              public chain = ( ^1{links} )
            ) : ()
            ",
            links = " + 1".repeat(50_000),
        };
        assert_eq!(nested_sends_of(&chain), 50_000);
    }
}
