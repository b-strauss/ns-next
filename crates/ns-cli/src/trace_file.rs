//! `ns-next run --trace-file=<path>` (`docs/design/developer-tool.md`,
//! decisions 17 to 20, "The trace file"): every event, listing and body
//! description the runtime delivers, written as one JSON object a line
//! into a file the JetBrains plugin reads while the program runs, in
//! place of the lines on standard error. A record's keys are the text
//! trace's names, every body is named by its number, and every position
//! is a range of lines and UTF-16 columns of its module's text.

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};

use ns_runtime::ModuleName;
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
const FORMAT: &str = "ns-next-trace";

/// The format's version, raised with every change a reader of the old
/// one would misread.
const VERSION: u32 = 1;

/// The architecture the file's `code` records are instructions of: the
/// one this binary's JIT emits for, the targets `ns-runtime`'s `build.rs`
/// sets `cfg(ns_jit)` for; `None` where it has no JIT and so writes no
/// `code` record.
fn architecture() -> Option<&'static str> {
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

/// A module's text with its line index, made when a body of the module
/// is first described.
struct Text {
    text: String,
    lines: Option<LineIndex>,
}

impl Text {
    fn new(text: String) -> Text {
        Text { text, lines: None }
    }

    /// `span` as `[startLine, startColumn, endLine, endColumn]`.
    fn range(&mut self, span: Span) -> Value {
        let text = &self.text;
        let lines = self.lines.get_or_insert_with(|| LineIndex::new(text));
        let start = lines.position(text, span.start);
        let end = lines.position(text, span.end);
        json!([start.line, start.column, end.line, end.column])
    }
}

/// A program's module the trace may describe a body of.
struct ProgramText {
    module: ModuleName,
    path: Option<PathBuf>,
    text: Text,
}

/// A file of the core library the trace may describe a body of.
struct LibraryText {
    path: &'static str,
    text: Text,
}

/// The texts a body's positions are counted in: the program's modules by
/// their names, with the files they were read from, and the core
/// library's files by their paths.
pub(crate) struct ModuleTexts {
    program: Vec<ProgramText>,
    library: Vec<LibraryText>,
}

impl ModuleTexts {
    /// The texts of `modules`, as `program::debug_modules` gives them, and
    /// of every file of the core library; a module without text has no
    /// positions.
    pub(crate) fn new(modules: Vec<ProgramModule>) -> ModuleTexts {
        ModuleTexts {
            program: modules
                .into_iter()
                .filter_map(|module| {
                    Some(ProgramText {
                        module: module.module,
                        path: module.path,
                        text: Text::new(module.text?),
                    })
                })
                .collect(),
            library: ns_runtime::shipped_source_files()
                .into_iter()
                .map(|file| LibraryText {
                    path: file.path,
                    text: Text::new(file.text.to_string()),
                })
                .collect(),
        }
    }

    /// Where `description`'s body's source is, as a `body` record's
    /// `source` gives it, and its module's text; `None` for a text this
    /// host does not hold.
    fn source_of(&mut self, description: &BodyDescription) -> (Value, Option<&mut Text>) {
        if let Some(shipped_path) = &description.shipped_path {
            let text = self
                .library
                .iter_mut()
                .find(|library| library.path == shipped_path)
                .map(|library| &mut library.text);
            return (json!({ "library": shipped_path }), text);
        }
        match self
            .program
            .iter_mut()
            .find(|program| program.module == description.module)
        {
            Some(program) => {
                let source = match &program.path {
                    Some(path) => json!({ "file": path.display().to_string() }),
                    None => Value::Null,
                };
                (source, Some(&mut program.text))
            }
            None => (Value::Null, None),
        }
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
/// block's family root, its module, where its source is, its range there, its checksum and its
/// statements and sends, every range counted in `texts`' text of its
/// module; each range `null` where `texts` holds no such text.
pub(crate) fn body_record(description: &BodyDescription, texts: &mut ModuleTexts) -> Value {
    let BodyDescription {
        body,
        family,
        module,
        shipped_path: _,
        source,
        checksum,
        statements,
        sends,
    } = description;
    let (source_record, mut text) = texts.source_of(description);
    let mut range = |span: Span| text.as_mut().map_or(Value::Null, |text| text.range(span));
    let range_record = range(*source);
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
/// and the scope and program counter it is inlined at.
fn scopes_record(scopes: &[ListedScope]) -> Vec<Value> {
    scopes
        .iter()
        .map(|scope| {
            let ListedScope {
                number: scope_number,
                body,
                parent,
                instructions: _,
            } = scope;
            json!({
                "scope": scope_number,
                "body": number(body),
                "parent": parent.map(|(parent, program_counter)| json!([parent, program_counter])),
            })
        })
        .collect()
}

/// The record of `listing`: a `bytecode`, `code` or `inlining` record.
pub(crate) fn listing_record(listing: &Listing) -> Value {
    match listing {
        Listing::Bytecode(bytecode) => bytecode_record(bytecode),
        Listing::Code(code) => code_record(code),
        Listing::Inlining(inlining) => inlining_record(inlining),
    }
}

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
            json!({ "body": number(&listed.body), "instructions": instructions })
        })
        .collect();
    json!({ "kind": "bytecode", "body": number(body), "bodies": bodies })
}

/// A `code` record: `code`'s instructions decoded as its text listing
/// decodes them, each with the scope and program counter of the stretch
/// it stands in and what a call or jump out of it reaches.
fn code_record(code: &CodeListing) -> Value {
    let CodeListing {
        body,
        tier,
        key,
        architecture,
        // An address of the run: two runs of one program differ in it
        // alone, and nothing in the record is placed by it.
        base_address: _,
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
                reached: _,
                position,
                annotations,
            } = annotated;
            let (scope, program_counter) = match position {
                Some(Position::Instruction {
                    scope,
                    program_counter,
                }) => (Some(scope), Some(program_counter)),
                Some(Position::OutOfLine) | None => (None, None),
            };
            // Two annotations on one instruction, which the text listing
            // writes one after the other, are joined as it separates them.
            let annotation = (!annotations.is_empty()).then(|| {
                annotations
                    .into_iter()
                    .map(listing::annotation_text)
                    .collect::<Vec<_>>()
                    .join("; ")
            });
            json!({
                "offset": instruction.offset,
                "text": instruction.text,
                "scope": scope,
                "at": program_counter,
                "annotation": annotation,
            })
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
        "bytes": bytes.len(),
        "instructions": instructions,
        "data": data,
        "scopes": scopes_record(scopes),
        "mark": mark,
    })
}

fn inlining_record(inlining: &InliningListing) -> Value {
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
        "scopes": scopes_record(scopes),
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
    modules: ModuleTexts,
}

/// Creates the trace file at `path`, replacing one there, and writes its
/// header, naming the `categories` traced and the listings `printing`
/// prints; refuses a path no file can be created at, naming it, so that
/// the run is refused before anything of it runs rather than trace into
/// nothing.
pub(crate) fn create(
    path: &Path,
    modules: ModuleTexts,
    categories: TraceCategories,
    printing: Printing,
) -> Result<TraceFileSink, String> {
    let file = File::create(path)
        .map_err(|error| format!("--trace-file: cannot create `{}`: {error}", path.display()))?;
    let mut sink = TraceFileSink {
        file: BufWriter::new(file),
        modules,
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
        self.write(&listing_record(listing));
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
        Annotation, Architecture, CodeTier, DataWord, DescribedSend, DescribedStatement,
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

    /// The texts of one program module, `Demo`, read from `/work/Demo.ns`.
    fn demo_texts(text: &str) -> ModuleTexts {
        ModuleTexts::new(vec![ProgramModule {
            module: ModuleName::named("Demo"),
            path: Some(PathBuf::from("/work/Demo.ns")),
            identity: None,
            text: Some(text.to_string()),
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
            source: Span::new(start("run"), start(" )\n)") + 2),
            checksum: 0x0123_4567_89ab_cdef,
            statements: vec![DescribedStatement {
                instructions: 0..3,
                span: send,
            }],
            sends: vec![DescribedSend {
                index: 1,
                site: Some(0),
                call: send,
                selector: vec![Span::new(start("size"), start("size") + 4)],
            }],
        };
        let record = body_record(&description, &mut demo_texts(text));
        // `'ä😀' ` is six UTF-16 units: the emoji counts two.
        assert_eq!(record["sends"][0]["selector"][0], json!([1, 16, 1, 20]));
        assert_eq!(record["sends"][0]["call"], json!([1, 10, 1, 20]));
        assert_eq!(record["sends"][0]["at"], 1);
        assert_eq!(record["sends"][0]["site"], 0);
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
            source: Span::new(0, 4),
            checksum: 1,
            statements: Vec::new(),
            sends: Vec::new(),
        };
        let record = body_record(&description, &mut demo_texts("Demo"));
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
            source: Span::new(0, 4),
            checksum: 1,
            statements: Vec::new(),
            sends: Vec::new(),
        };
        let record = body_record(&description, &mut demo_texts(""));
        assert_eq!(
            record["source"],
            json!({ "library": "platform/TimeFactory.ns" })
        );
        assert!(record["range"].is_array(), "{record}");
        assert_eq!(record["checksum"], "0000000000000001");
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
}
