//! `ns-next run --trace-file=<path>` (`docs/design/developer-tool.md`,
//! decisions 17 to 20, "The trace file"): every event, listing and body
//! description the runtime delivers, written as one JSON object a line
//! into a file the JetBrains plugin reads while the program runs, in
//! place of the lines on standard error. A record's keys are the text
//! trace's names, every body is named by its number, and every position
//! is a range of lines and UTF-16 columns of its module's text.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ns_runtime::ModuleName;
use ns_runtime::trace_events::descriptions::source_checksum;
use ns_runtime::trace_events::{
    ArmKind, BodyDescription, BodyName, BytecodeListing, CodeListing, InliningListing,
    InliningOutcome, ListedScope, Listing, Position,
};
use ns_runtime::{Printing, TraceCategories, TraceCategory, TraceEvent, TraceEventKind, TraceSink};
use ns_syntax::Span;
use ns_tooling::lines::LineIndex;
use serde_json::{Map, Value, json};

use crate::debug::ProgramModule;
use crate::listing::{self, AnnotatedInstruction};

/// The format's name, which the header gives so that a reader refuses a
/// file that is no trace.
pub(crate) const FORMAT: &str = "ns-next-trace";

/// The format's version, raised with every change a reader of the old
/// one would misread.
pub(crate) const VERSION: u32 = 1;

/// The architecture the file's `code` records are instructions of: the
/// one this binary's JIT emits for, the targets `ns-runtime`'s `build.rs`
/// sets `cfg(ns_jit)` for; `None` where it has no JIT and so writes no
/// `code` record.
pub(crate) fn architecture() -> Option<&'static str> {
    if cfg!(target_arch = "x86_64") {
        Some("x64")
    } else if cfg!(all(
        target_arch = "aarch64",
        any(target_os = "linux", target_os = "macos")
    )) {
        Some("aarch64")
    } else {
        None
    }
}

/// How many module texts keep their line index: a run describes the
/// bodies of a few modules at a time, and a text an edit replaced is
/// asked for again only by the bodies described from it before.
const KEPT_LINE_INDEXES: usize = 8;

/// A program's module the trace may describe a body of: the file it was
/// read from and its text as read, with that text's line index once a
/// body is placed in it.
struct ProgramSource {
    module: ModuleName,
    path: Option<PathBuf>,
    text: Option<String>,
    lines: Option<LineIndex>,
}

/// Which text a `body` record's ranges count in, as its `placed` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placed {
    /// The text the record's `source` names: the program's file as read,
    /// or the core library's file.
    Source,
    /// The runtime's current text of the module, which an edit made and
    /// no file holds, so that a reader holding the file places nothing of
    /// the body there.
    Current,
}

impl Placed {
    /// The record's word for it.
    fn word(self) -> &'static str {
        match self {
            Placed::Source => "source",
            Placed::Current => "current",
        }
    }
}

/// Where a family root was placed, which its blocks follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootPlace {
    /// In the program file of that index, its source moved by `offset`
    /// bytes from its own span: each block stands as far inside it.
    File { index: usize, offset: i64 },
    /// In the runtime's current text the description hands, the shipped
    /// library's among them.
    Text,
    /// In no text: nor are its blocks.
    Nowhere,
}

/// The text a body is placed in, its line index, where the body's source
/// starts in it and which text it is.
struct Placing<'sources> {
    text: &'sources str,
    lines: &'sources LineIndex,
    start: u32,
    placed: Placed,
}

/// The program's modules, by whose files a `body` record names its source
/// and in whose texts as read the bodies they hold are placed, and the
/// line indexes of the runtime's module texts a description hands.
pub(crate) struct ModuleSources {
    program: Vec<ProgramSource>,
    /// The runtime's texts most recently counted in, the latest last:
    /// the runtime hands every description made from one text the same
    /// `Rc`, and a text it makes again after an install, alike, finds its
    /// index by its contents.
    lines: Vec<(Rc<str>, LineIndex)>,
    /// Where each family root described was placed, by its number, which
    /// its blocks are placed by.
    roots: HashMap<u32, RootPlace>,
}

impl ModuleSources {
    /// The files and texts of `modules`, as `program::debug_modules` gives
    /// them.
    pub(crate) fn new(modules: Vec<ProgramModule>) -> ModuleSources {
        ModuleSources {
            program: modules
                .into_iter()
                .map(|module| ProgramSource {
                    module: module.module,
                    path: module.path,
                    text: module.text,
                    lines: None,
                })
                .collect(),
            lines: Vec::new(),
            roots: HashMap::new(),
        }
    }

    /// The text `description`'s body is placed in, with its line index,
    /// where the body's source starts in it, and which text that is: the
    /// program file's text as read where it holds the body's own source at
    /// its span -- the body's version was read in it, whatever an edit did
    /// around it -- or where the runtime's current text holds it, since
    /// that is the text a reader shows; else the runtime's current text,
    /// where the description says it holds that source; else none. A block
    /// whose family's root was described is placed only where the root
    /// was: in the file at its distance inside the root there, in the
    /// runtime's current text, or nowhere.
    fn placing<'sources>(
        &'sources mut self,
        description: &'sources BodyDescription,
    ) -> Option<Placing<'sources>> {
        let holds_at = |text: &str, span: Span| {
            text.as_bytes()
                .get(span.start as usize..span.end as usize)
                .is_some_and(|bytes| source_checksum(bytes) == description.checksum)
        };
        // A block stands where its family's root was placed, at its distance
        // inside the root, or nowhere: its bytes alone may stand in the file
        // where the file holds another version of the method, which an edit
        // replaced around it, or at another place altogether.
        let root = description
            .family
            .as_ref()
            .and_then(|family| self.roots.get(&family.number).copied());
        let held = match root {
            Some(RootPlace::Nowhere) => return None,
            Some(RootPlace::Text) => None,
            Some(RootPlace::File { index, offset }) => {
                let start = i64::from(description.source.start) + offset;
                let start = u32::try_from(start).ok()?;
                let span = Span::new(
                    start,
                    start + (description.source.end - description.source.start),
                );
                let text = self.program[index].text.as_deref()?;
                if !holds_at(text, span) {
                    return None;
                }
                Some((index, start))
            }
            None => self.held_in_file(description, holds_at),
        };
        if description.family.is_none() {
            let place = match held {
                Some((index, start)) => RootPlace::File {
                    index,
                    offset: i64::from(start) - i64::from(description.source.start),
                },
                None if description.current.is_none() => RootPlace::Nowhere,
                None => RootPlace::Text,
            };
            self.roots.insert(description.body.number, place);
        }
        match held {
            Some((index, start)) => {
                let ProgramSource { text, lines, .. } = &mut self.program[index];
                let text = text.as_deref()?;
                let lines = lines.get_or_insert_with(|| LineIndex::new(text));
                Some(Placing {
                    text,
                    lines,
                    start,
                    placed: Placed::Source,
                })
            }
            None => {
                let current = description.current.as_ref()?;
                let start = current.source.start;
                // A shipped module's current text is the library's own,
                // which no edit changes: the text its `source` names.
                let placed = if description.shipped_path.is_some() {
                    Placed::Source
                } else {
                    Placed::Current
                };
                Some(Placing {
                    text: &current.text,
                    lines: self.lines_of(&current.text),
                    start,
                    placed,
                })
            }
        }
    }

    /// Where a program file holds `description`'s body's own source, by
    /// `holds_at`: the file's index and the source's start in it, at the
    /// body's own span or at the span the runtime's current text holds it
    /// at; `None` for a shipped body, whose text is the library's.
    fn held_in_file(
        &self,
        description: &BodyDescription,
        holds_at: impl Fn(&str, Span) -> bool,
    ) -> Option<(usize, u32)> {
        if description.shipped_path.is_some() {
            return None;
        }
        let spans = [
            Some(description.source),
            description.current.as_ref().map(|current| current.source),
        ];
        self.program
            .iter()
            .enumerate()
            .find_map(|(index, program)| {
                let text = program.text.as_deref()?;
                if program.module != description.module {
                    return None;
                }
                spans
                    .iter()
                    .flatten()
                    .find(|span| holds_at(text, **span))
                    .map(|span| (index, span.start))
            })
    }

    /// Where `description`'s body's source is, as a `body` record's
    /// `source` gives it.
    fn source_of(&self, description: &BodyDescription) -> Value {
        if let Some(shipped_path) = &description.shipped_path {
            return json!({ "library": shipped_path });
        }
        match self
            .program
            .iter()
            .find(|program| program.module == description.module)
        {
            Some(ProgramSource {
                path: Some(path), ..
            }) => json!({ "file": path.display().to_string() }),
            Some(_) | None => Value::Null,
        }
    }

    /// The line index of `text`, made on its first use.
    fn lines_of(&mut self, text: &Rc<str>) -> &LineIndex {
        match self
            .lines
            .iter()
            .position(|(known, _)| Rc::ptr_eq(known, text) || **known == **text)
        {
            Some(index) => {
                let kept = self.lines.remove(index);
                self.lines.push(kept);
            }
            None => {
                if self.lines.len() == KEPT_LINE_INDEXES {
                    self.lines.remove(0);
                }
                self.lines.push((Rc::clone(text), LineIndex::new(text)));
            }
        }
        let Some((_, lines)) = self.lines.last() else {
            unreachable!("internal: the text's index was just kept");
        };
        lines
    }
}

/// `body`'s number, as every record names a body. Number 0 names a body
/// outside the trace, which only a frame's code the debugger answers
/// holds, never a delivery.
fn number(body: &BodyName) -> u32 {
    debug_assert_ne!(body.number, 0, "a body the trace did not number");
    body.number
}

/// A module's name as a record gives it.
fn module_record(module: &ModuleName) -> Value {
    json!({ "id": module.id, "version": module.version })
}

/// The `body` record of `description`: the body's number and name, a
/// block's family root, its module, where its source is, its range there,
/// its checksum and its statements and sends, every range counted in the
/// text that holds the body as it was compiled (`ModuleSources::placing`)
/// and each `null` where none does, and which text that is, `placed`.
pub(crate) fn body_record(description: &BodyDescription, sources: &mut ModuleSources) -> Value {
    let BodyDescription {
        body,
        family,
        module,
        shipped_path: _,
        // Read by `placing`.
        current: _,
        source,
        checksum,
        statements,
        sends,
    } = description;
    let source_record = sources.source_of(description);
    let lines = sources.placing(description);
    // A span moves with the body's source to where the text holds it. Only
    // the body's source is known to stand there, by its checksum, so a span
    // outside it -- in either text -- stays unplaced rather than land on
    // text that may differ; and a span the runtime has no place for has none.
    let range = |span: Option<Span>| match (&lines, span) {
        (
            Some(Placing {
                text, lines, start, ..
            }),
            Some(span),
        ) => {
            if span.start < source.start || span.end > source.end {
                return Value::Null;
            }
            let (begin, end) = (
                span.start - source.start + start,
                span.end - source.start + start,
            );
            let begin = lines.position(text, begin);
            let end = lines.position(text, end);
            json!([begin.line, begin.column, end.line, end.column])
        }
        _ => Value::Null,
    };
    let range_record = range(Some(*source));
    let statements: Vec<Value> = statements
        .iter()
        .map(|statement| {
            json!({
                "instructions": [statement.instructions.start, statement.instructions.end],
                "range": range(statement.span),
            })
        })
        .collect();
    let sends: Vec<Value> = sends
        .iter()
        .map(|send| {
            let selector: Vec<Value> = send.selector.iter().map(|token| range(*token)).collect();
            json!({
                "at": send.index,
                "site": send.site,
                "call": range(send.call),
                "selector": selector,
                "name": send.name,
            })
        })
        .collect();
    json!({
        "kind": "body",
        "body": number(body),
        "name": body.name,
        "family": family.as_ref().map(number),
        "module": module_record(module),
        "source": source_record,
        "range": range_record,
        "placed": lines.as_ref().map(|placing| placing.placed.word()),
        "checksum": format!("{checksum:016x}"),
        "statements": statements,
        "sends": sends,
    })
}

/// An event record's fields, under the names its text line gives them.
struct Fields(Map<String, Value>);

impl Fields {
    fn value(&mut self, field: &str, value: impl Into<Value>) {
        self.0.insert(field.to_string(), value.into());
    }

    fn body(&mut self, field: &str, body: &BodyName) {
        self.value(field, number(body));
    }

    /// A customization key's fields, `null` where the event has none.
    fn key(&mut self, key: &Option<String>, entered: &Option<String>) {
        self.value("key", key.clone());
        self.value("entered", entered.clone());
    }
}

/// The `event` record of `event`: its category, its body's number, its
/// name as `TraceEventKind::name` answers it, and every value its text
/// line writes, each a field under the line's name for it.
pub(crate) fn event_record(event: &TraceEvent) -> Value {
    let mut fields = Fields(Map::new());
    fields.value("kind", "event");
    fields.value("category", event.category.name());
    fields.value("body", event.body.as_ref().map(number));
    fields.value("event", event.kind.name());
    match &event.kind {
        TraceEventKind::Compiled { trigger, bytes } => {
            fields.value("trigger", trigger.name());
            fields.value("bytes", *bytes);
        }
        TraceEventKind::CompiledTwin { bytes } => fields.value("bytes", *bytes),
        TraceEventKind::CompileRefused { trigger, reason } => {
            fields.value("trigger", trigger.name());
            fields.value("reason", reason.name());
        }
        TraceEventKind::CompileHeld { trigger, reason } => {
            fields.value("trigger", trigger.name());
            fields.value("reason", reason.name());
        }
        TraceEventKind::TwinHeld { reason } => fields.value("reason", reason.name()),
        TraceEventKind::Abandoned { exits } => fields.value("exits", *exits),
        TraceEventKind::Revived { revival } => fields.value("by", revival.name()),
        TraceEventKind::RefusedForItsTwin => {}
        TraceEventKind::AbandonedRefused { reason } => fields.value("reason", reason.name()),
        TraceEventKind::OsrCompiled { loop_head } | TraceEventKind::OsrEntered { loop_head } => {
            fields.value("at", *loop_head);
        }
        TraceEventKind::OsrRefused { loop_head, reason } => {
            fields.value("at", *loop_head);
            fields.value("reason", reason.name());
        }
        TraceEventKind::OptimizationRequested {
            spent_by,
            walk_end,
            key,
            entered,
        }
        | TraceEventKind::OptimizationNotDue {
            spent_by,
            walk_end,
            key,
            entered,
        } => {
            fields.key(key, entered);
            fields.body("spent-by", spent_by);
            fields.value("walk-end", walk_end.name());
        }
        TraceEventKind::OptimizationDropped {
            key,
            entered,
            cause,
        } => {
            fields.key(key, entered);
            fields.value("cause", cause.name());
        }
        TraceEventKind::OptimizationDeferred {
            key,
            entered,
            reason,
        } => {
            fields.key(key, entered);
            fields.value("reason", reason.name());
        }
        TraceEventKind::Optimized {
            key,
            entered,
            bytes,
            inlined_scopes,
            inlined_sites,
            type_case_arms,
            eliminated_blocks,
        } => {
            fields.key(key, entered);
            fields.value("bytes", *bytes);
            fields.value("inlined-scopes", *inlined_scopes);
            fields.value("inlined-sites", *inlined_sites);
            fields.value("type-case-arms", *type_case_arms);
            fields.value("eliminated-blocks", *eliminated_blocks);
        }
        TraceEventKind::OptimizationRefused {
            key,
            entered,
            reason,
        } => {
            fields.key(key, entered);
            fields.value("reason", reason.name());
        }
        TraceEventKind::Inlined {
            key,
            entered,
            in_body,
            program_counter,
            callee,
        } => {
            fields.key(key, entered);
            fields.body("in-body", in_body);
            fields.body("callee", callee);
            fields.value("at", *program_counter);
        }
        TraceEventKind::InlineDeclined {
            key,
            entered,
            in_body,
            program_counter,
            callee,
            reason,
        } => {
            fields.key(key, entered);
            fields.body("in-body", in_body);
            fields.body("callee", callee);
            fields.value("reason", reason.name());
            fields.value("at", *program_counter);
        }
        TraceEventKind::InlineDeclinedNoBody {
            key,
            entered,
            reasons,
        } => {
            fields.key(key, entered);
            fields.value(
                "count",
                reasons.iter().map(|&(_, count)| count).sum::<u32>(),
            );
            // An array rather than an object, so that the reasons keep
            // the order the line writes them in.
            let reasons: Vec<Value> = reasons
                .iter()
                .map(|(reason, count)| json!({ "reason": reason.name(), "count": count }))
                .collect();
            fields.value("reasons", reasons);
        }
        TraceEventKind::Retired {
            key,
            entered,
            reason,
            version,
        } => {
            fields.key(key, entered);
            fields.value("reason", reason.name());
            fields.value("version", *version);
        }
        TraceEventKind::Invalidated {
            key,
            entered,
            cause,
        } => {
            fields.key(key, entered);
            fields.value("cause", cause.name());
        }
        TraceEventKind::Compacted {
            freeing,
            bodies_freed,
        } => {
            fields.value("freeing", freeing.name());
            fields.value("bodies-freed", *bodies_freed);
        }
        TraceEventKind::Freed {
            key,
            entered,
            tier,
            cause,
        } => {
            fields.key(key, entered);
            fields.value("tier", tier.name());
            fields.value("cause", cause.name());
        }
        TraceEventKind::ZoneReplaced { bodies_freed } => {
            fields.value("bodies-freed", *bodies_freed);
        }
        TraceEventKind::CacheTransition {
            before,
            after,
            selector,
            site,
            receiver,
        } => {
            // The line spells the two states as its event word; the
            // record gives each a field, and the selector without the
            // `#` the line writes it with.
            fields.value("before", before.name());
            fields.value("after", after.name());
            fields.value("selector", selector.as_str());
            fields.value("site", *site);
            fields.value("receiver", receiver.as_str());
        }
        TraceEventKind::CachesVacated { cause } => fields.value("cause", cause.name()),
    }
    Value::Object(fields.0)
}

/// A customization key's two names, `null` where the listing has none.
fn key_record(key: Option<&(String, Option<String>)>) -> (Value, Value) {
    match key {
        Some((receiver, entered)) => (json!(receiver), json!(entered)),
        None => (Value::Null, Value::Null),
    }
}

/// The scopes of a code or inlining listing, each with its body's number
/// and the scope and program counter it is inlined at, and, for a body
/// `listed` says no earlier record lists the bytecode of, its instruction
/// lines (`ListedBodies`).
fn scopes_record(scopes: &[ListedScope], listed: &dyn Fn(u32) -> bool) -> Vec<Value> {
    scopes
        .iter()
        .map(|scope| {
            let ListedScope {
                number: scope_number,
                body,
                parent,
                instructions,
            } = scope;
            let mut record = json!({
                "scope": scope_number,
                "body": number(body),
                "parent": parent.map(|(parent, program_counter)| json!([parent, program_counter])),
            });
            if !listed(number(body)) {
                record["lines"] = json!(instructions);
            }
            record
        })
        .collect()
}

/// The record of `listing`: a `bytecode`, `code` or `inlining` record, as
/// a file whose bytecode records list every body writes it.
#[cfg(test)]
pub(crate) fn listing_record(listing: &Listing) -> Value {
    listing_record_given(listing, &|_| true)
}

/// The record of `listing`, `listed` saying whether an earlier record of
/// the file lists a body's bytecode.
fn listing_record_given(listing: &Listing, listed: &dyn Fn(u32) -> bool) -> Value {
    match listing {
        Listing::Bytecode(bytecode) => bytecode_record(bytecode),
        Listing::Code(code) => code_record(code, listed),
        Listing::Inlining(inlining) => inlining_record(inlining, listed),
    }
}

/// Which bodies' instruction lines a file's code and inlining records
/// carry in their scopes. A file that lists bytecode names its positions
/// as the text listing does, with each instruction, from the bytecode
/// records it holds; where none lists a scope's body -- one no mixin
/// holds, a factory a host's instantiation built for itself, which has no
/// bytecode listing ever, or one the print filter leaves out -- the scope
/// carries its body's lines, so that the file writes every position as
/// the text listing does. A file that lists no bytecode carries none: its
/// positions name their program counters alone, and its code records
/// stay free of every scope's bytecode.
pub(crate) struct ListedBodies {
    /// Whether the file lists bytecode at all.
    bytecode: bool,
    /// The bodies its bytecode records listed so far.
    bodies: HashSet<u32>,
}

impl ListedBodies {
    /// The bodies of a file that lists bytecode where `bytecode` says so.
    pub(crate) fn new(bytecode: bool) -> ListedBodies {
        ListedBodies {
            bytecode,
            bodies: HashSet::new(),
        }
    }

    /// The record of `listing` as the file it follows writes it, its
    /// bodies noted when it lists their bytecode.
    pub(crate) fn record(&mut self, listing: &Listing) -> Value {
        if let Listing::Bytecode(bytecode) = listing {
            self.bodies
                .extend(bytecode.bodies.iter().map(|listed| number(&listed.body)));
        }
        if !self.bytecode {
            return listing_record_given(listing, &|_| true);
        }
        let listed = &self.bodies;
        listing_record_given(listing, &|body| listed.contains(&body))
    }
}

/// A `bytecode` record: each body of `bytecode`, the root first, with what
/// the text listing's line above its instructions writes -- its kind, its
/// arguments and its registers --, a block's `in` and `at`, the number
/// of the body that makes it and the index of the `MakeClosure` it is
/// nested under there, and its instructions.
fn bytecode_record(bytecode: &BytecodeListing) -> Value {
    let BytecodeListing {
        body,
        text: _,
        bodies,
    } = bytecode;
    let bodies: Vec<Value> = bodies
        .iter()
        .map(|listed| {
            let instructions: Vec<Value> = listed
                .instructions
                .iter()
                .map(|instruction| {
                    json!({
                        "index": instruction.index,
                        "statement": instruction.statement,
                        "text": instruction.text,
                    })
                })
                .collect();
            json!({
                "body": number(&listed.body),
                "kind": listed.kind,
                "in": listed.made_at.map(|made_at| made_at.maker),
                "at": listed.made_at.map(|made_at| made_at.index),
                "arguments": listed.argument_count,
                "registers": listed.register_count,
                "instructions": instructions,
            })
        })
        .collect();
    json!({ "kind": "bytecode", "body": number(body), "bodies": bodies })
}

/// The separator of an instruction's annotations in its record.
const ANNOTATION_SEPARATOR: &str = "; ";

/// Two annotations on one instruction, which the text listing writes one
/// after the other, joined as it separates them. A reader -- the plugin
/// -- splits the text at the separator to write each after its own
/// `  ; `, so that no annotation's own text may hold it, or the reader
/// would write one annotation as two.
fn joined_annotations(texts: impl Iterator<Item = String>) -> String {
    let texts: Vec<String> = texts.collect();
    debug_assert!(
        texts
            .iter()
            .all(|text| !text.contains(ANNOTATION_SEPARATOR)),
        "an annotation holds the separator its record joins them with: {texts:?}"
    );
    texts.join(ANNOTATION_SEPARATOR)
}

/// A `code` record: `code`'s instructions decoded as its text listing
/// decodes them, each with the scope and program counter of the stretch
/// it stands in, or that it stands out of line, and what a call or jump
/// out of it reaches; and the address the code was installed at, as the
/// text listing's `base=` writes it.
fn code_record(code: &CodeListing, listed: &dyn Fn(u32) -> bool) -> Value {
    let CodeListing {
        body,
        tier,
        key,
        architecture,
        base_address,
        bytes,
        code_end: _,
        words,
        // Placed on the instructions by `listing::annotated`.
        positions: _,
        annotations: _,
        scopes,
        mark,
    } = code;
    let instructions: Vec<Value> = listing::annotated(code)
        .into_iter()
        .map(|annotated| {
            let AnnotatedInstruction {
                instruction,
                reached,
                position,
                annotations,
            } = annotated;
            // An out-of-line stretch has no scope or program counter, as
            // the code before the first position has none: the flag tells
            // the two apart, so that a reader names the first stretch out
            // of line as the text listing does.
            let (scope, program_counter, out_of_line) = match position {
                Some(Position::Instruction {
                    scope,
                    program_counter,
                }) => (Some(scope), Some(program_counter), false),
                Some(Position::OutOfLine) => (None, None, true),
                None => (None, None, false),
            };
            let annotation = (!annotations.is_empty())
                .then(|| joined_annotations(annotations.into_iter().map(listing::annotation_text)));
            let mut record = json!({
                "offset": instruction.offset,
                "text": instruction.text,
                "scope": scope,
                "at": program_counter,
                "out-of-line": out_of_line,
                "annotation": annotation,
            });
            // The positions an instruction that emitted no code left
            // beside this one's, which the text listing names each: only
            // where there are some, since most instructions have none.
            if let Some((_, passed)) = reached.split_last()
                && !passed.is_empty()
            {
                record["passed"] = passed
                    .iter()
                    .map(|position| position_record(*position))
                    .collect();
            }
            record
        })
        .collect();
    // Offsets from the first data word, as the instructions' texts name
    // the words.
    let data_start = words.first().map_or(0, |(offset, _)| *offset);
    let data: Vec<Value> = words
        .iter()
        .map(|(offset, word)| {
            json!({ "offset": offset - data_start, "word": listing::word_text(*word) })
        })
        .collect();
    let (key, entered) = key_record(key.as_ref());
    json!({
        "kind": "code",
        "body": number(body),
        "tier": tier.name(),
        "key": key,
        "entered": entered,
        "architecture": architecture.name(),
        "base": base_address,
        "bytes": bytes.len(),
        "instructions": instructions,
        "data": data,
        "scopes": scopes_record(scopes, listed),
        "mark": mark,
    })
}

/// A position a `code` record's `passed` names: `{"scope":s,"at":pc}`, or
/// `{"out-of-line":true}`.
fn position_record(position: Position) -> Value {
    match position {
        Position::Instruction {
            scope,
            program_counter,
        } => json!({ "scope": scope, "at": program_counter }),
        Position::OutOfLine => json!({ "out-of-line": true }),
    }
}

fn inlining_record(inlining: &InliningListing, listed: &dyn Fn(u32) -> bool) -> Value {
    let InliningListing {
        body,
        key,
        scopes,
        arms,
        declined,
        outcome,
    } = inlining;
    let arms: Vec<Value> = arms
        .iter()
        .map(|arm| {
            let inlined = match arm.inlined {
                ArmKind::Scope(scope) => json!({ "scope": scope }),
                ArmKind::Always(kind) => json!({ "always": kind }),
            };
            json!({
                "scope": arm.scope,
                "at": arm.program_counter,
                "receiver": arm.receiver,
                "inlined": inlined,
            })
        })
        .collect();
    let declined: Vec<Value> = declined
        .iter()
        .map(|decline| {
            json!({
                "scope": decline.scope,
                "at": decline.program_counter,
                "callee": decline.callee.as_ref().map(number),
                "reason": decline.reason.name(),
            })
        })
        .collect();
    let outcome = match outcome {
        InliningOutcome::Installed { bytes } => json!({ "installed": bytes }),
        InliningOutcome::Refused(refusal) => json!({ "refused": refusal.name() }),
    };
    let (key, entered) = key_record(key.as_ref());
    json!({
        "kind": "inlining",
        "body": number(body),
        "key": key,
        "entered": entered,
        "scopes": scopes_record(scopes, listed),
        "arms": arms,
        "declined": declined,
        "outcome": outcome,
    })
}

/// The `header` record, the file's first line: besides the format, the
/// categories traced and the listings printed, so that a reader knows
/// which records can arrive -- an optimizing compile's outcome, which
/// says whether its inlining decisions stand, comes only as an
/// `optimize` event or an inlining listing.
fn header_record(categories: TraceCategories, printing: Printing) -> Value {
    let categories: Vec<&str> = TraceCategory::ALL
        .into_iter()
        .filter(|category| categories.contains(*category))
        .map(TraceCategory::name)
        .collect();
    let Printing {
        bytecode,
        code,
        inlining,
    } = printing;
    let listings: Vec<&str> = [
        (bytecode, "bytecode"),
        (code, "code"),
        (inlining, "inlining"),
    ]
    .into_iter()
    .filter_map(|(printed, kind)| printed.then_some(kind))
    .collect();
    json!({
        "kind": "header",
        "format": FORMAT,
        "version": VERSION,
        "architecture": architecture(),
        "categories": categories,
        "listings": listings,
    })
}

/// Writes each delivery as its record, one line each, into the trace
/// file.
pub(crate) struct TraceFileSink {
    file: BufWriter<File>,
    modules: ModuleSources,
    listed: ListedBodies,
}

/// Creates the trace file at `path`, replacing one there, and writes its
/// header, naming the `categories` traced and the listings `printing`
/// prints; refuses a path no file can be created at, naming it, so that
/// the run is refused before anything of it runs rather than trace into
/// nothing.
pub(crate) fn create(
    path: &Path,
    modules: ModuleSources,
    categories: TraceCategories,
    printing: Printing,
) -> Result<TraceFileSink, String> {
    let file = File::create(path)
        .map_err(|error| format!("--trace-file: cannot create `{}`: {error}", path.display()))?;
    let mut sink = TraceFileSink {
        file: BufWriter::new(file),
        modules,
        listed: ListedBodies::new(printing.bytecode),
    };
    sink.write(&header_record(categories, printing));
    // A reader that opens the file at once finds it a trace.
    sink.flushed();
    Ok(sink)
}

impl TraceFileSink {
    /// `record` and its newline in one write, so that a reader following
    /// the file never sees half a record once a flush has written it. A
    /// file that cannot be written to loses the trace, never the run.
    fn write(&mut self, record: &Value) {
        let mut line = record.to_string();
        line.push('\n');
        let _ = self.file.write_all(line.as_bytes());
    }
}

impl TraceSink for TraceFileSink {
    fn event(&mut self, event: &TraceEvent) {
        self.write(&event_record(event));
    }

    fn listing(&mut self, listing: &Listing) {
        let record = self.listed.record(listing);
        self.write(&record);
    }

    fn body(&mut self, description: &BodyDescription) {
        let record = body_record(description, &mut self.modules);
        self.write(&record);
    }

    fn flushed(&mut self) {
        let _ = self.file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::tests::{every_event, split_outside_braces};
    use ns_runtime::trace_events::{
        Annotation, Architecture, CodeTier, CurrentText, DataWord, DescribedSend,
        DescribedStatement, ListedBody, ListedInstruction, MadeAt,
    };

    /// The fields whose values name a body, by the line's names.
    const BODY_FIELDS: [&str; 3] = ["in-body", "callee", "spent-by"];

    #[test]
    fn every_event_has_a_record() {
        for (event, line) in every_event() {
            let record = event_record(&event);
            assert_eq!(record["kind"], "event", "{record}");
            assert_eq!(record["category"], event.category.name(), "{record}");
            assert_eq!(record["event"], event.kind.name(), "{record}");
            match &event.body {
                Some(body) => assert_eq!(record["body"], body.number, "{record}"),
                None => assert!(record["body"].is_null(), "{record}"),
            }
            let parts = split_outside_braces(line);
            if let TraceEventKind::CacheTransition { before, after, .. } = &event.kind {
                assert_eq!(parts[2], format!("{}->{}", before.name(), after.name()));
                assert_eq!(record["before"], before.name(), "{record}");
                assert_eq!(record["after"], after.name(), "{record}");
            }
            for field in &parts[3..] {
                let (name, value) = field.split_once('=').unwrap();
                let written = record
                    .get(name)
                    .unwrap_or_else(|| panic!("no `{name}` in {record} for {line}"));
                if BODY_FIELDS.contains(&name) {
                    assert!(written.is_u64(), "`{name}` in {record}");
                } else if let Some(name_text) = value
                    .strip_prefix('{')
                    .and_then(|rest| rest.strip_suffix('}'))
                {
                    assert_eq!(written, name_text, "`{name}` in {record}");
                } else if let Some(number) = written.as_u64() {
                    assert_eq!(number.to_string(), value, "`{name}` in {record}");
                } else if let Some(text) = written.as_str() {
                    assert_eq!(value.trim_start_matches('#'), text, "`{name}` in {record}");
                }
            }
        }
    }

    /// The plugin's fixture of every event (`EventsFixtureTest`), from the
    /// repository's root: each event's record and the line the text trace
    /// writes of it, which the plugin's Events tab is held to.
    const EVENTS_FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/jetbrains/src/test/resources/devtool/events.jsonl"
    );

    /// One line of the fixture per event of `every_event`: the bodies it
    /// names, each named body numbered apart by its first place in the
    /// line -- `every_event` numbers every body 1 --, the event's record
    /// naming them by those numbers, and its text line.
    fn events_fixture() -> String {
        let mut fixture = String::new();
        for (event, line) in every_event() {
            let mut record = event_record(&event);
            let parts = split_outside_braces(line);
            let mut names: Vec<String> = Vec::new();
            let mut number_of = |name: &str| {
                let position = names
                    .iter()
                    .position(|known| known == name)
                    .unwrap_or_else(|| {
                        names.push(name.to_string());
                        names.len() - 1
                    });
                position + 1
            };
            let body = parts[1]
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
                .expect("a line names its body in braces");
            if !body.is_empty() {
                record["body"] = json!(number_of(body));
            }
            for field in &parts[3..] {
                let (name, value) = field.split_once('=').expect("a field is name=value");
                if BODY_FIELDS.contains(&name) {
                    let named = value
                        .strip_prefix('{')
                        .and_then(|rest| rest.strip_suffix('}'))
                        .expect("a body field names its body in braces");
                    record[name] = json!(number_of(named));
                }
            }
            let bodies: Vec<Value> = names
                .iter()
                .enumerate()
                .map(|(index, name)| json!({ "body": index + 1, "name": name }))
                .collect();
            let entry = json!({ "bodies": bodies, "record": record, "line": line });
            fixture.push_str(&entry.to_string());
            fixture.push('\n');
        }
        fixture
    }

    /// The fixture holds every event as this build writes it: a change to
    /// an event's record or line fails here until the fixture is written
    /// again by `regenerate_the_events_fixture`, which the plugin's test
    /// then holds the Events tab to.
    #[test]
    fn the_events_fixture_is_current() {
        let written = std::fs::read_to_string(EVENTS_FIXTURE)
            .unwrap_or_else(|error| panic!("{EVENTS_FIXTURE}: {error}"));
        // A checkout may have given the file CRLF line ends.
        assert!(
            written.replace("\r\n", "\n") == events_fixture(),
            "{EVENTS_FIXTURE} is stale: run `cargo test -p ns-cli --bins regenerate_the_events_fixture -- --ignored`"
        );
    }

    #[test]
    #[ignore = "regenerates the plugin's events fixture; run deliberately"]
    fn regenerate_the_events_fixture() {
        std::fs::write(EVENTS_FIXTURE, events_fixture()).expect("write the events fixture");
    }

    #[test]
    fn a_record_is_one_line() {
        for (event, _) in every_event() {
            assert!(!event_record(&event).to_string().contains('\n'));
        }
        let listing = Listing::Bytecode(BytecodeListing {
            body: BodyName {
                number: 1,
                name: "Demo>>run".to_string(),
            },
            text: "method Demo>>run\n   0   stmt 0  Return r0\n".to_string(),
            bodies: Vec::new(),
        });
        assert!(!listing_record(&listing).to_string().contains('\n'));
        assert!(
            !header_record(TraceCategories::default(), Printing::default())
                .to_string()
                .contains('\n')
        );
    }

    /// A bytecode record gives each body what the text listing's line of
    /// it writes, so that a reader writes the line as the text does.
    #[test]
    fn a_bytecode_record_gives_each_body_its_line() {
        let listing = Listing::Bytecode(BytecodeListing {
            body: BodyName {
                number: 1,
                name: "Demo>>run".to_string(),
            },
            text: String::new(),
            bodies: vec![
                ListedBody {
                    body: BodyName {
                        number: 1,
                        name: "Demo>>run".to_string(),
                    },
                    kind: "method",
                    made_at: None,
                    argument_count: 1,
                    register_count: 16,
                    instructions: vec![ListedInstruction {
                        index: 0,
                        statement: Some(0),
                        text: "Return r0".to_string(),
                    }],
                },
                ListedBody {
                    body: BodyName {
                        number: 2,
                        name: "[] in Demo>>run".to_string(),
                    },
                    kind: "block",
                    made_at: Some(MadeAt { maker: 1, index: 0 }),
                    argument_count: 0,
                    register_count: 14,
                    instructions: Vec::new(),
                },
            ],
        });
        let record = listing_record(&listing);
        assert_eq!(record["kind"], "bytecode");
        assert_eq!(
            record["bodies"],
            json!([
                {
                    "body": 1,
                    "kind": "method",
                    "in": null,
                    "at": null,
                    "arguments": 1,
                    "registers": 16,
                    "instructions": [{"index": 0, "statement": 0, "text": "Return r0"}],
                },
                {"body": 2, "kind": "block", "in": 1, "at": 0, "arguments": 0, "registers": 14, "instructions": []},
            ])
        );
    }

    /// The header names the categories traced and the listings printed,
    /// each in its documented order.
    #[test]
    fn the_header_names_what_is_traced() {
        let categories = TraceCategories::default()
            .with(TraceCategory::Caches)
            .with(TraceCategory::Compile);
        let printing = Printing {
            bytecode: false,
            code: true,
            inlining: true,
        };
        let header = header_record(categories, printing);
        assert_eq!(header["kind"], "header");
        assert_eq!(header["format"], FORMAT);
        assert_eq!(header["version"], VERSION);
        assert_eq!(header["categories"], json!(["compile", "caches"]));
        assert_eq!(header["listings"], json!(["code", "inlining"]));
        let nothing = header_record(TraceCategories::default(), Printing::default());
        assert_eq!(nothing["categories"], json!([]));
        assert_eq!(nothing["listings"], json!([]));
    }

    /// The sources of one program module, `Demo`, read from
    /// `/work/Demo.ns`.
    fn demo_sources() -> ModuleSources {
        ModuleSources::new(vec![ProgramModule {
            module: ModuleName::named("Demo"),
            path: Some(PathBuf::from("/work/Demo.ns")),
            identity: None,
            text: Some("Demo = ()".to_string()),
        }])
    }

    #[test]
    fn ranges_count_utf16_columns() {
        let text = "Demo = (\n  run = ( 'ä😀' size )\n)";
        let start = |part: &str| u32::try_from(text.find(part).unwrap()).unwrap();
        let send = Span::new(start("'ä"), start("size") + 4);
        let description = BodyDescription {
            body: BodyName {
                number: 3,
                name: "Demo>>run".to_string(),
            },
            family: None,
            module: ModuleName::named("Demo"),
            shipped_path: None,
            current: current_at(text, Span::new(start("run"), start(" )\n)") + 2)),
            source: Span::new(start("run"), start(" )\n)") + 2),
            checksum: 0x0123_4567_89ab_cdef,
            statements: vec![DescribedStatement {
                instructions: 0..3,
                span: Some(send),
            }],
            sends: vec![DescribedSend {
                index: 1,
                site: Some(0),
                call: Some(send),
                selector: vec![Some(Span::new(start("size"), start("size") + 4))],
                name: Some("size".to_string()),
            }],
        };
        let record = body_record(&description, &mut demo_sources());
        // `'ä😀' ` is six UTF-16 units: the emoji counts two.
        assert_eq!(record["sends"][0]["selector"][0], json!([1, 16, 1, 20]));
        assert_eq!(record["sends"][0]["call"], json!([1, 10, 1, 20]));
        assert_eq!(record["sends"][0]["at"], 1);
        assert_eq!(record["sends"][0]["site"], 0);
        // The selector's text, as the compiled send names it.
        assert_eq!(record["sends"][0]["name"], "size");
        assert_eq!(record["range"], json!([1, 2, 1, 22]));
        assert_eq!(record["statements"][0]["instructions"], json!([0, 3]));
        assert_eq!(record["statements"][0]["range"], json!([1, 10, 1, 20]));
        assert_eq!(record["checksum"], "0123456789abcdef");
        assert_eq!(record["source"], json!({ "file": "/work/Demo.ns" }));
        assert_eq!(record["module"], json!({ "id": "Demo", "version": null }));
        assert_eq!(record["name"], "Demo>>run");
        assert_eq!(record["body"], 3);
        assert!(record["family"].is_null(), "{record}");
    }

    /// A block's record names its family's root, whose number the cache
    /// transitions of the block's sites carry.
    #[test]
    fn a_block_names_its_family() {
        let description = BodyDescription {
            body: BodyName {
                number: 4,
                name: "[] in Demo>>run".to_string(),
            },
            family: Some(BodyName {
                number: 3,
                name: "Demo>>run".to_string(),
            }),
            module: ModuleName::named("Demo"),
            shipped_path: None,
            current: current_at("Demo", Span::new(0, 4)),
            source: Span::new(0, 4),
            checksum: 1,
            statements: Vec::new(),
            sends: Vec::new(),
        };
        let record = body_record(&description, &mut demo_sources());
        assert_eq!(record["family"], 3, "{record}");
        assert_eq!(record["body"], 4, "{record}");
    }

    #[test]
    fn a_shipped_body_points_at_the_library() {
        let description = BodyDescription {
            body: BodyName {
                number: 1,
                name: "newspeak.core.TimeFactory>>new".to_string(),
            },
            family: None,
            module: ModuleName::named("newspeak.core.TimeFactory"),
            shipped_path: Some("platform/TimeFactory.ns".to_string()),
            current: current_at("Time", Span::new(0, 4)),
            source: Span::new(0, 4),
            checksum: 1,
            statements: Vec::new(),
            sends: Vec::new(),
        };
        let record = body_record(&description, &mut demo_sources());
        assert_eq!(
            record["source"],
            json!({ "library": "platform/TimeFactory.ns" })
        );
        assert!(record["range"].is_array(), "{record}");
        // The library's text is the shipped source no edit changes.
        assert_eq!(record["placed"], "source", "{record}");
        assert_eq!(record["checksum"], "0000000000000001");
    }

    /// A body an edit moved is placed in the file where the file holds
    /// its source at the span the runtime's current text holds it at --
    /// the file read after the edit was saved -- since the file is the
    /// text a reader shows.
    #[test]
    fn the_file_places_a_moved_body_where_it_holds_it() {
        let edited = "Demo = (\n  helper = ( )\n\n  run = ( )\n)";
        let standing = edited.find("run").unwrap();
        let checksum = source_checksum(&edited.as_bytes()[standing..standing + 3]);
        let mut sources = ModuleSources::new(vec![ProgramModule {
            module: ModuleName::named("Demo"),
            path: Some(PathBuf::from("/work/Demo.ns")),
            identity: None,
            text: Some(edited.to_string()),
        }]);
        let standing = u32::try_from(standing).unwrap();
        let description = BodyDescription {
            source: Span::new(40, 43),
            current: current_at(edited, Span::new(standing, standing + 3)),
            checksum,
            ..run_in(edited, 1)
        };
        let record = body_record(&description, &mut sources);
        assert_eq!(record["range"], json!([3, 2, 3, 5]), "{record}");
        assert_eq!(record["placed"], "source", "{record}");
    }

    /// The runtime's current text `text`, holding the body at `source`.
    fn current_at(text: &str, source: Span) -> Option<CurrentText> {
        Some(CurrentText {
            text: Rc::from(text),
            source,
        })
    }

    /// A description of `Demo>>run` whose source is `run` in `text`.
    fn run_in(text: &str, number: u32) -> BodyDescription {
        let start = u32::try_from(text.find("run").unwrap()).unwrap();
        BodyDescription {
            body: BodyName {
                number,
                name: "Demo>>run".to_string(),
            },
            family: None,
            module: ModuleName::named("Demo"),
            shipped_path: None,
            current: current_at(text, Span::new(start, start + 3)),
            source: Span::new(start, start + 3),
            checksum: 1,
            statements: vec![DescribedStatement {
                instructions: 0..1,
                span: Some(Span::new(start, start + 3)),
            }],
            sends: Vec::new(),
        }
    }

    /// A placed body with a statement and a send the runtime has no place
    /// for -- a full initializer's statement of a clause its version's text
    /// no longer holds -- writes `null` for those ranges and places the
    /// body and its other statement: every statement is listed, so that the
    /// instructions keep their statements' numbers, and nothing unplaced is
    /// given a place.
    #[test]
    fn a_statement_without_a_place_writes_no_range() {
        let read = "Demo = (\n  run = ( )\n)";
        let mut sources = demo_sources();
        let mut description = run_in(read, 1);
        description.statements.insert(
            0,
            DescribedStatement {
                instructions: 0..1,
                span: None,
            },
        );
        description.sends.push(DescribedSend {
            index: 0,
            site: None,
            call: None,
            selector: vec![None, description.statements[1].span],
            name: None,
        });
        // And one placed outside the body's source, at the class's name, as a
        // class that writes no superclass clause places its superclass's.
        description.statements.push(DescribedStatement {
            instructions: 1..2,
            span: Some(Span::new(0, 4)),
        });
        let record = body_record(&description, &mut sources);
        assert_eq!(record["statements"][2]["range"], Value::Null, "{record}");
        assert_eq!(record["range"], json!([1, 2, 1, 5]), "{record}");
        assert_eq!(record["statements"][0]["range"], Value::Null, "{record}");
        assert_eq!(
            record["statements"][1]["range"],
            json!([1, 2, 1, 5]),
            "{record}"
        );
        assert_eq!(record["sends"][0]["call"], Value::Null, "{record}");
        assert_eq!(
            record["sends"][0]["selector"],
            json!([null, [1, 2, 1, 5]]),
            "{record}"
        );
        // A send whose instruction names no selector has no name.
        assert_eq!(record["sends"][0]["name"], Value::Null, "{record}");
    }

    /// Where the file's text does not hold the body at its span -- a body
    /// an edit installed during the run compiled -- the ranges count in the
    /// runtime's text the description hands with it.
    #[test]
    fn ranges_count_in_the_text_the_body_was_placed_in() {
        let read = "Demo = (\n  run = ( )\n)";
        let mut sources = demo_sources();
        let before = body_record(&run_in(read, 1), &mut sources);
        assert_eq!(before["range"], json!([1, 2, 1, 5]), "{before}");
        let edited = "Demo = (\n  helper = ( )\n\n  run = ( )\n)";
        let after = body_record(&run_in(edited, 2), &mut sources);
        assert_eq!(after["range"], json!([3, 2, 3, 5]), "{after}");
        assert_eq!(
            after["statements"][0]["range"],
            json!([3, 2, 3, 5]),
            "{after}"
        );
        assert_eq!(after["source"], json!({ "file": "/work/Demo.ns" }));
        // The ranges are the edited text's, which the file named does not
        // hold, and the record says so.
        assert_eq!(after["placed"], "current", "{after}");
    }

    /// A body whose own text neither the file nor the runtime holds -- one
    /// edit compiled it, a later one moved it -- is placed where the
    /// runtime's current text holds its source, every span within it
    /// moving with it.
    #[test]
    fn a_moved_body_is_placed_where_the_current_text_holds_it() {
        let edited = "Demo = (\n  helper = ( )\n\n  run = ( )\n)";
        let standing = u32::try_from(edited.find("run").unwrap()).unwrap();
        let description = BodyDescription {
            source: Span::new(40, 43),
            current: current_at(edited, Span::new(standing, standing + 3)),
            statements: vec![DescribedStatement {
                instructions: 0..1,
                span: Some(Span::new(41, 43)),
            }],
            ..run_in(edited, 1)
        };
        let record = body_record(&description, &mut demo_sources());
        assert_eq!(record["range"], json!([3, 2, 3, 5]), "{record}");
        assert_eq!(
            record["statements"][0]["range"],
            json!([3, 3, 3, 5]),
            "{record}"
        );
    }

    /// The file's text as read places a body it holds at its span, its
    /// own source's checksum telling, before the runtime's text: a body
    /// compiled from the file stands there, whatever an edit made of the
    /// module's text around it, and the file is the text a reader shows.
    #[test]
    fn the_file_places_a_body_it_holds() {
        let file = "Demo = (\n  run = ( )\n)";
        let start = file.find("run").unwrap();
        let checksum = source_checksum(&file.as_bytes()[start..start + 3]);
        let mut sources = ModuleSources::new(vec![ProgramModule {
            module: ModuleName::named("Demo"),
            path: Some(PathBuf::from("/work/Demo.ns")),
            identity: None,
            text: Some(file.to_string()),
        }]);
        // The runtime's text, moved by an edit: `run` stands elsewhere.
        let edited = "Demo = (\n  helper = ( )\n\n  run = ( )\n)";
        let description = BodyDescription {
            current: current_at(edited, Span::new(13, 16)),
            checksum,
            ..run_in(file, 1)
        };
        let record = body_record(&description, &mut sources);
        assert_eq!(record["range"], json!([1, 2, 1, 5]), "{record}");
        assert_eq!(record["placed"], "source", "{record}");
        // The same body with no runtime text is placed by the file alone.
        let unhanded = BodyDescription {
            current: None,
            ..description
        };
        let record = body_record(&unhanded, &mut sources);
        assert_eq!(record["range"], json!([1, 2, 1, 5]), "{record}");
    }

    /// A span outside the body's source is placed in neither text, since
    /// only the body's source is known to stand where the text holds it.
    #[test]
    fn a_span_outside_the_source_is_never_placed() {
        let file = "Demo = (
  run = ( )
)";
        let start = file.find("run").unwrap();
        let checksum = source_checksum(&file.as_bytes()[start..start + 3]);
        let mut sources = ModuleSources::new(vec![ProgramModule {
            module: ModuleName::named("Demo"),
            path: Some(PathBuf::from("/work/Demo.ns")),
            identity: None,
            text: Some(file.to_string()),
        }]);
        let outside = |description: BodyDescription| BodyDescription {
            statements: vec![DescribedStatement {
                instructions: 0..1,
                span: Some(Span::new(0, 4)),
            }],
            ..description
        };
        let in_file = outside(BodyDescription {
            checksum,
            ..run_in(file, 1)
        });
        let record = body_record(&in_file, &mut sources);
        assert_eq!(record["range"], json!([1, 2, 1, 5]), "{record}");
        assert!(record["statements"][0]["range"].is_null(), "{record}");
        let in_current = outside(run_in(file, 2));
        let record = body_record(&in_current, &mut demo_sources());
        assert!(record["range"].is_array(), "{record}");
        assert!(record["statements"][0]["range"].is_null(), "{record}");
    }

    /// A body no text holds has every range `null`, and keeps its
    /// checksum, so that a reader decorates nothing of it.
    #[test]
    fn a_body_placed_in_no_text_has_no_ranges() {
        let text = "Demo = (\n  run = ( )\n)";
        let description = BodyDescription {
            current: None,
            ..run_in(text, 1)
        };
        let record = body_record(&description, &mut demo_sources());
        assert!(record["range"].is_null(), "{record}");
        assert!(record["statements"][0]["range"].is_null(), "{record}");
        assert!(record["placed"].is_null(), "{record}");
        assert_eq!(record["checksum"], "0000000000000001");
        assert_eq!(record["source"], json!({ "file": "/work/Demo.ns" }));
    }

    /// A block of a method an edit compiled is placed where its root is,
    /// in the runtime's current text, even where its own bytes stand in
    /// the file at its span -- inside the file's other version of the
    /// method, which the edit replaced around the block -- and a block of
    /// a root placed in no text in none.
    #[test]
    fn a_block_is_placed_in_its_roots_text() {
        let file = "Demo = (\n  run = ( ^[:x | x] )\n)";
        let edited = "Demo = (\n  run = ( ^[:x | x] (* longer *) )\n)";
        let block_start = u32::try_from(file.find("[:x").unwrap()).unwrap();
        let block = Span::new(block_start, block_start + 8);
        // The block stands at the same span in both versions of `run`.
        let bytes = block_start as usize..block.end as usize;
        assert_eq!(&edited[bytes.clone()], &file[bytes]);
        let mut sources = ModuleSources::new(vec![ProgramModule {
            module: ModuleName::named("Demo"),
            path: Some(PathBuf::from("/work/Demo.ns")),
            identity: None,
            text: Some(file.to_string()),
        }]);
        // The edit's `run`, whose source the file does not hold.
        let root = run_in(edited, 1);
        let record = body_record(&root, &mut sources);
        assert_eq!(record["placed"], "current", "{record}");
        let block_of = |number: u32, root: u32| BodyDescription {
            body: BodyName {
                number,
                name: "[] in Demo>>run".to_string(),
            },
            family: Some(BodyName {
                number: root,
                name: "Demo>>run".to_string(),
            }),
            current: current_at(edited, block),
            source: block,
            checksum: source_checksum(&file.as_bytes()[block.start as usize..block.end as usize]),
            statements: Vec::new(),
            ..run_in(edited, number)
        };
        let record = body_record(&block_of(2, 1), &mut sources);
        assert_eq!(record["placed"], "current", "{record}");
        // A root no text holds leaves its block unplaced too.
        let unplaced = BodyDescription {
            current: None,
            ..run_in(edited, 3)
        };
        assert!(body_record(&unplaced, &mut sources)["placed"].is_null());
        let record = body_record(&block_of(4, 3), &mut sources);
        assert!(record["placed"].is_null(), "{record}");
        assert!(record["range"].is_null(), "{record}");
    }

    /// A block of a root the file holds only where the runtime's current
    /// text holds it -- not at the root's own span -- is placed at its
    /// distance inside the root there, even where the file holds the
    /// block's own bytes at the block's own span, which lies outside it.
    #[test]
    fn a_block_stands_at_its_distance_inside_its_root() {
        let file = "Demo = (\n  x = ( [:y | y] )\n  run = ( [:y | y] )\n)";
        let offset_of = |part: &str| u32::try_from(file.find(part).unwrap()).unwrap();
        let root_source = "run = ( [:y | y] )";
        let root_length = u32::try_from(root_source.len()).unwrap();
        let run = offset_of("run");
        // The root's own span, as an edit left it, where the file holds
        // `x`'s block eight bytes in, and so the block's own span is `x`'s.
        let own = offset_of("[:y") - 8;
        let mut sources = ModuleSources::new(vec![ProgramModule {
            module: ModuleName::named("Demo"),
            path: Some(PathBuf::from("/work/Demo.ns")),
            identity: None,
            text: Some(file.to_string()),
        }]);
        let root = BodyDescription {
            source: Span::new(own, own + root_length),
            current: current_at(file, Span::new(run, run + root_length)),
            checksum: source_checksum(root_source.as_bytes()),
            statements: Vec::new(),
            ..run_in(file, 1)
        };
        let record = body_record(&root, &mut sources);
        assert_eq!(record["range"], json!([2, 2, 2, 20]), "{record}");
        let block = Span::new(own + 8, own + 16);
        let described = BodyDescription {
            body: BodyName {
                number: 2,
                name: "[] in Demo>>run".to_string(),
            },
            family: Some(BodyName {
                number: 1,
                name: "Demo>>run".to_string(),
            }),
            source: block,
            current: current_at(file, Span::new(run + 8, run + 16)),
            checksum: source_checksum(b"[:y | y]"),
            statements: Vec::new(),
            ..run_in(file, 2)
        };
        let record = body_record(&described, &mut sources);
        assert_eq!(record["range"], json!([2, 10, 2, 18]), "{record}");
        assert_eq!(record["placed"], "source", "{record}");
    }

    /// A block of a shipped root is placed in the library's text, as its
    /// root is.
    #[test]
    fn a_shipped_block_is_placed_in_the_library() {
        let text = "Time = ( now = ( ^[:x | x] ) )";
        let offset_of = |part: &str| u32::try_from(text.find(part).unwrap()).unwrap();
        let shipped = |number: u32, family: Option<u32>, span: Span| BodyDescription {
            body: BodyName {
                number,
                name: "newspeak.core.TimeFactory>>now".to_string(),
            },
            family: family.map(|number| BodyName {
                number,
                name: "newspeak.core.TimeFactory>>now".to_string(),
            }),
            module: ModuleName::named("newspeak.core.TimeFactory"),
            shipped_path: Some("platform/TimeFactory.ns".to_string()),
            current: current_at(text, span),
            source: span,
            checksum: 1,
            statements: Vec::new(),
            sends: Vec::new(),
        };
        let mut sources = demo_sources();
        let root = body_record(
            &shipped(1, None, Span::new(offset_of("now"), offset_of(" )") + 2)),
            &mut sources,
        );
        assert_eq!(root["placed"], "source", "{root}");
        let block = body_record(
            &shipped(
                2,
                Some(1),
                Span::new(offset_of("[:x"), offset_of("[:x") + 8),
            ),
            &mut sources,
        );
        assert_eq!(block["placed"], "source", "{block}");
        assert_eq!(block["range"], json!([0, 18, 0, 26]), "{block}");
    }

    /// A body's code on the architecture this binary decodes, as the
    /// text listing's own tests write it: a branch, a call out of the
    /// body and a load of a data word; with the offset past the code and
    /// those of the second and third instructions.
    #[cfg(target_arch = "x86_64")]
    fn native_code() -> (Architecture, Vec<u8>, u32, [u32; 2]) {
        let bytes = vec![
            0xEB, 0x02, // jmp short to the mov at 0x04
            0xFF, 0xD0, // call rax
            0x48, 0x8B, 0x05, 0x01, 0x00, 0x00, 0x00, // mov rax, [rip+1], the word at 0x0c
            0xC3, // ret
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // the data word
        ];
        (Architecture::X64, bytes, 0x0c, [2, 4])
    }

    #[cfg(target_arch = "aarch64")]
    fn native_code() -> (Architecture, Vec<u8>, u32, [u32; 2]) {
        let bytes = [
            0x1400_0002_u32, // b to 0x8
            0xD63F_0200,     // blr x16
            0x5800_0040,     // ldr x0, the literal at 0x10
            0xD65F_03C0,     // ret
            0,
            0,
        ]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
        (Architecture::Aarch64, bytes, 0x10, [4, 8])
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn machine_code_is_decoded_per_instruction() {
        let (architecture, bytes, code_end, [second, third]) = native_code();
        let scope = ListedScope {
            number: 0,
            body: BodyName {
                number: 2,
                name: "Demo>>answer".to_string(),
            },
            parent: None,
            instructions: vec![
                "   0   stmt 0  LoadSmallInteger r1, 42".to_string(),
                "   1   stmt 0  Return r1".to_string(),
            ],
        };
        let code = CodeListing {
            body: scope.body.clone(),
            tier: CodeTier::Baseline,
            key: None,
            architecture,
            base_address: 0x4000,
            bytes,
            code_end,
            words: vec![(code_end, DataWord::Budget)],
            positions: vec![
                (
                    second,
                    Position::Instruction {
                        scope: 0,
                        program_counter: 0,
                    },
                ),
                (
                    third,
                    Position::Instruction {
                        scope: 0,
                        program_counter: 1,
                    },
                ),
            ],
            annotations: vec![(second, Annotation::Leaf("miss"))],
            scopes: vec![scope],
            mark: None,
        };
        let listing = Listing::Code(code);
        // The text listing's instruction lines, `    +0x<offset>  <text>`
        // and the annotation after `  ; `.
        let text = listing::text(&listing);
        let lines: Vec<(u64, &str, Option<&str>)> = text
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("    +0x")?;
                let (offset, instruction) = rest.split_once("  ")?;
                let (instruction, annotation) = match instruction.split_once("  ; ") {
                    Some((instruction, annotation)) => (instruction, Some(annotation)),
                    None => (instruction, None),
                };
                Some((
                    u64::from_str_radix(offset, 16).unwrap(),
                    instruction,
                    annotation,
                ))
            })
            .collect();
        let record = listing_record(&listing);
        let instructions = record["instructions"].as_array().unwrap();
        assert_eq!(instructions.len(), lines.len(), "{text}\n{record}");
        for (instruction, (offset, line, annotation)) in instructions.iter().zip(&lines) {
            assert_eq!(instruction["offset"], *offset, "{record}");
            assert_eq!(instruction["text"], *line, "{record}");
            assert_eq!(instruction["annotation"], json!(annotation), "{record}");
        }
        assert!(instructions[0]["scope"].is_null(), "{record}");
        assert_eq!(instructions[0]["out-of-line"], false, "{record}");
        assert_eq!(instructions[1]["out-of-line"], false, "{record}");
        assert_eq!(record["base"], 0x4000, "{record}");
        assert_eq!(instructions[1]["annotation"], "leaf miss", "{record}");
        assert_eq!(instructions[1]["at"], 0, "{record}");
        assert_eq!(instructions[2]["at"], 1, "{record}");
        assert_eq!(instructions[2]["scope"], 0, "{record}");
        assert_eq!(
            record["data"],
            json!([{ "offset": 0, "word": "budget word" }])
        );
        assert_eq!(record["kind"], "code");
        assert_eq!(record["tier"], "baseline");
        assert_eq!(record["architecture"], architecture.name());
        assert_eq!(
            record["scopes"],
            json!([{ "scope": 0, "body": 2, "parent": null }])
        );
        assert!(record["mark"].is_null() && record["key"].is_null());
    }

    /// An out-of-line stretch is told apart from the code before the first
    /// position, which both have no scope and no program counter: the
    /// first stretch out of line, which the text listing names, is so in
    /// the record too.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn an_out_of_line_stretch_is_told_from_no_position() {
        let (architecture, bytes, code_end, [second, third]) = native_code();
        let body = BodyName {
            number: 2,
            name: "Demo>>answer".to_string(),
        };
        let code = CodeListing {
            body: body.clone(),
            tier: CodeTier::Baseline,
            key: None,
            architecture,
            base_address: 0x4000,
            bytes,
            code_end,
            words: vec![(code_end, DataWord::Budget)],
            positions: vec![
                (second, Position::OutOfLine),
                (
                    third,
                    Position::Instruction {
                        scope: 0,
                        program_counter: 1,
                    },
                ),
            ],
            annotations: Vec::new(),
            scopes: vec![ListedScope {
                number: 0,
                body,
                parent: None,
                instructions: Vec::new(),
            }],
            mark: None,
        };
        let listing = Listing::Code(code);
        let text = listing::text(&listing);
        assert!(text.contains("  out of line\n"), "{text}");
        let record = listing_record(&listing);
        let instructions = record["instructions"].as_array().unwrap();
        assert_eq!(instructions[0]["out-of-line"], false, "{record}");
        assert_eq!(instructions[1]["out-of-line"], true, "{record}");
        assert!(instructions[1]["at"].is_null(), "{record}");
        assert_eq!(instructions[2]["out-of-line"], false, "{record}");
        assert_eq!(instructions[2]["at"], 1, "{record}");
    }

    /// An instruction's annotations are joined by the separator a reader
    /// splits them at.
    #[test]
    fn annotations_are_joined_by_their_separator() {
        let texts = [Annotation::Leaf("miss"), Annotation::Cell { site: 0 }]
            .iter()
            .map(listing::annotation_text);
        assert_eq!(joined_annotations(texts), "leaf miss; cell site 0");
    }

    /// An annotation whose own text holds the separator would be read
    /// back as two: a debug build refuses to write it.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "an annotation holds the separator its record joins them with")]
    fn an_annotation_holding_the_separator_is_refused() {
        let texts = [Annotation::Leaf("miss"), Annotation::Target("one; two")]
            .iter()
            .map(listing::annotation_text);
        joined_annotations(texts);
    }
}
