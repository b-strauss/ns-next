//! The index of a traced session's trace file, which the MCP server's
//! trace tools answer from (`docs/design/debugger.md`, "The MCP server"):
//! a follower thread reads the file the child writes through the server's
//! own handle as it grows, as the plugin's reader does
//! (`docs/design/developer-tool.md`, "The plugin's half"), and keeps of
//! each record what the answers decide by -- the header, each body's
//! description but its statements, each event compact, its texts
//! interned, each listing's place in the file and the fields its answers
//! choose by, and per body the counts the overview ranks by -- never a
//! listing's content, which a window reads again from the file.

#![expect(
    dead_code,
    reason = "the trace tools read the index's accessors as they arrive"
)]

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use ns_runtime::tool_names::KnownModule;
use ns_runtime::{ModuleName, TraceCategories, TraceCategory, TraceFilter};
use serde::Deserialize;
use serde_json::Value;

use super::trace_files::TraceFile;
use crate::trace_file::{FORMAT, VERSION};

/// How long the follower waits before it looks again right after the
/// file grew, and the longest it waits while it does not: each quiet look
/// doubles the wait up to it, the plugin's reader's two waits.
const POLL: Duration = Duration::from_millis(50);
const LONGEST_POLL: Duration = Duration::from_millis(500);

/// How many bytes one read takes from the file.
const READ_BYTES: usize = 64 * 1024;

/// The refusal of a file whose first line is no header of the format.
const NO_TRACE: &str = "the trace file is no ns-next trace";

/// A kind of listing record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListingKind {
    Bytecode,
    Code,
    Inlining,
}

impl ListingKind {
    /// The kind a record's `kind`, or `debug_start`'s `listings`, names.
    pub(crate) fn from_name(name: &str) -> Option<ListingKind> {
        match name {
            "bytecode" => Some(ListingKind::Bytecode),
            "code" => Some(ListingKind::Code),
            "inlining" => Some(ListingKind::Inlining),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            ListingKind::Bytecode => "bytecode",
            ListingKind::Code => "code",
            ListingKind::Inlining => "inlining",
        }
    }
}

/// The file's header: what the run traces and lists.
pub(crate) struct Header {
    /// The architecture its `code` records are instructions of; `None`
    /// for a binary without a JIT.
    pub(crate) architecture: Option<String>,
    pub(crate) categories: TraceCategories,
    pub(crate) listings: Vec<ListingKind>,
}

/// Where a body's source is, as its record says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BodySource {
    /// A program's file, by its path.
    File(String),
    /// A shipped module's file, by its path in the core library.
    Library(String),
    /// No file holds it.
    Nowhere,
}

/// A send of a body as its record describes it.
pub(crate) struct DescribedSend {
    /// Its instruction's index in the body.
    pub(crate) instruction: u32,
    /// Its cache site, numbered in the body's family.
    pub(crate) site: Option<u32>,
    /// The range of its call, lines and UTF-16 columns from 0.
    pub(crate) call: Option<[u32; 4]>,
    /// The selector's text, where the record gives it.
    pub(crate) name: Option<String>,
}

/// A `body` record but its statements.
pub(crate) struct BodyRecord {
    pub(crate) name: String,
    /// A block's family root.
    pub(crate) family: Option<u32>,
    pub(crate) module: ModuleName,
    pub(crate) source: BodySource,
    /// Lines and UTF-16 columns from 0, in the text `placed` names.
    pub(crate) range: Option<[u32; 4]>,
    pub(crate) placed: Option<String>,
    pub(crate) checksum: String,
    pub(crate) sends: Vec<DescribedSend>,
}

/// One event of the file, compact: its texts interned (`TraceIndex::text`).
pub(crate) struct IndexedEvent {
    /// Where its record begins in the file.
    pub(crate) offset: u64,
    pub(crate) category: TraceCategory,
    /// The record's event word: `transition` for a cache transition.
    pub(crate) event: u32,
    /// The word the text line writes: a transition's two states,
    /// `monomorphic->polymorphic`, the event word otherwise.
    pub(crate) word: u32,
    /// Its `reason`, `cause`, `by` or `walk-end`.
    pub(crate) reason: Option<u32>,
    /// `declined-no-body`'s reasons with their counts.
    pub(crate) reasons: Option<Box<[(u32, u32)]>>,
    pub(crate) body: Option<u32>,
    pub(crate) spent_by: Option<u32>,
    pub(crate) in_body: Option<u32>,
    pub(crate) callee: Option<u32>,
    pub(crate) key: Option<u32>,
    pub(crate) entered: Option<u32>,
    /// Its fields as the text line writes them after the word, each body
    /// field `{#<n>}`: ` trigger=call bytes=1216`, empty for none.
    pub(crate) fields: u32,
    /// The whole entry, `#<n> [<category>] <word><fields>`, the body's
    /// number left out for an event about none: two events with one text
    /// are the same event, which a fold compares by this number alone.
    pub(crate) text: u32,
}

/// What an inlining plan's compile came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanOutcome {
    /// Its copy was installed, of that many bytes.
    Installed(u64),
    /// It was refused, for the reason interned.
    Refused(u32),
}

/// One listing record of the file: its place and the fields its answers
/// choose by, never its content.
pub(crate) struct ListingEntry {
    pub(crate) kind: ListingKind,
    pub(crate) body: u32,
    /// A `code` record's tier, interned.
    pub(crate) tier: Option<u32>,
    pub(crate) key: Option<u32>,
    pub(crate) entered: Option<u32>,
    /// A `code` record's bytes.
    pub(crate) bytes: Option<u64>,
    /// An `inlining` record's outcome.
    pub(crate) outcome: Option<PlanOutcome>,
    pub(crate) offset: u64,
    /// The record's length in bytes, without its newline.
    pub(crate) length: u32,
}

/// Installs of machine code by tier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Installs {
    pub(crate) baseline: u32,
    pub(crate) twin: u32,
    pub(crate) optimized: u32,
}

impl Installs {
    pub(crate) fn total(self) -> u32 {
        self.baseline + self.twin + self.optimized
    }
}

/// An optimized copy's key, as its events give it: the receiver's
/// application and the one it was entered at, interned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CopyKey {
    pub(crate) key: Option<u32>,
    pub(crate) entered: Option<u32>,
}

/// The optimized copies of one key: how many were installed and the last
/// one's bytes.
pub(crate) struct Copies {
    pub(crate) key: CopyKey,
    pub(crate) count: u32,
    pub(crate) last_bytes: u64,
}

/// What the file says of one body, by its number: its description, the
/// counts the overview ranks by, and where its events and listings are.
/// Every count list holds its keys in the order they first came.
#[derive(Default)]
pub(crate) struct BodyTally {
    pub(crate) description: Option<BodyRecord>,
    /// Its installs as `compiled`, `compiled-twin` and `optimized`
    /// events announced them.
    pub(crate) announced: Installs,
    /// Its installs as its `code` records list them.
    pub(crate) listed: Installs,
    pub(crate) abandoned: u32,
    /// The exits its `abandoned` events counted, summed.
    pub(crate) exits: u64,
    /// Its `revived` events by their `by`.
    pub(crate) revived: Vec<(u32, u32)>,
    /// Its `retired` and `invalidated` events by event word and reason.
    pub(crate) deoptimizations: Vec<((u32, Option<u32>), u32)>,
    /// Its optimized copies by key, from its `optimized` events.
    pub(crate) copies: Vec<Copies>,
    /// The `declined` events naming it the callee, by reason.
    pub(crate) declined_as_callee: Vec<(u32, u32)>,
    /// The bodies whose compiles declined it, by number.
    pub(crate) declined_in: Vec<(u32, u32)>,
    /// The `declined` events of its own compiles, by reason.
    pub(crate) declined_as_compiler: Vec<(u32, u32)>,
    /// The bodies whose compiles inlined it, by number and key.
    pub(crate) inlined_into: Vec<((u32, Option<u32>), u32)>,
    /// The indexes of the events about it, in file order.
    pub(crate) events: Vec<u32>,
    /// The indexes of its `code` records, in file order.
    pub(crate) code: Vec<u32>,
    /// The indexes of its `inlining` records, in file order.
    pub(crate) inlining: Vec<u32>,
    /// The index of the `bytecode` record that lists it.
    pub(crate) bytecode: Option<u32>,
}

/// A send's cache as its site's last transition left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheState {
    /// The state after the transition, interned.
    pub(crate) after: u32,
    /// The receivers the site has seen since it was last vacant.
    pub(crate) receivers: usize,
    /// The index of the transition's event.
    pub(crate) event: u32,
}

/// What the trace says became of a send.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SendOutcome {
    /// The `inlined` or `declined` event of the last installed copy that
    /// still lives and planned it.
    pub(crate) inlining: Option<u32>,
    pub(crate) cache: Option<CacheState>,
}

/// An optimizing compile: its body and its copy's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CompileKey {
    body: u32,
    key: CopyKey,
}

/// A send by its body and instruction, or a cache site by its family's
/// root and number.
type SendKey = (u32, u32);

/// A cache site's last state and the receivers seen since it was vacant.
struct CacheHistory {
    after: u32,
    receivers: Vec<u32>,
    event: u32,
}

/// What became of the sends, kept as the plugin's `TraceRun` keeps it: a
/// plan's decisions stand only once its compile's copy is heard
/// installed, and go with the copy.
struct SendStates {
    /// Whether a compile's outcome can arrive: an `optimize` event or an
    /// inlining listing. Where neither is traced no decision is taken,
    /// since none would ever be heard installed.
    outcome_can_arrive: bool,
    open_compile: Option<CompileKey>,
    /// The open compile's decisions, each the event that made it.
    open_decisions: Vec<(SendKey, u32)>,
    /// The decisions of each installed copy that still lives, the one
    /// installed last at the end.
    live_copies: Vec<(CompileKey, Vec<(SendKey, u32)>)>,
    decisions: HashMap<SendKey, u32>,
    caches: HashMap<SendKey, CacheHistory>,
}

impl SendStates {
    fn new() -> SendStates {
        SendStates {
            outcome_can_arrive: true,
            open_compile: None,
            open_decisions: Vec::new(),
            live_copies: Vec::new(),
            decisions: HashMap::new(),
            caches: HashMap::new(),
        }
    }

    fn continue_compile(&mut self, key: CompileKey) {
        if self.open_compile == Some(key) {
            return;
        }
        self.leave_open_compile();
        self.open_compile = Some(key);
    }

    /// A record of no compile's stretch came: the open compile's outcome
    /// was withheld, and its decisions are dropped.
    fn leave_open_compile(&mut self) {
        self.open_compile = None;
        self.open_decisions.clear();
    }

    fn conclude(&mut self, key: CompileKey, installed: bool) {
        let open = self.open_compile;
        let decisions = std::mem::take(&mut self.open_decisions);
        self.leave_open_compile();
        if open != Some(key) || decisions.is_empty() || !installed {
            return;
        }
        // A key's new copy replaces its old one and is the one installed last.
        self.live_copies.retain(|(live, _)| *live != key);
        for &(send, event) in &decisions {
            self.decisions.insert(send, event);
        }
        self.live_copies.push((key, decisions));
    }

    fn copy_gone(&mut self, key: CompileKey) {
        let Some(position) = self.live_copies.iter().position(|(live, _)| *live == key) else {
            return;
        };
        self.live_copies.remove(position);
        self.decisions.clear();
        for (_, decisions) in &self.live_copies {
            for &(send, event) in decisions {
                self.decisions.insert(send, event);
            }
        }
    }
}

/// Texts kept once, each named by its number.
#[derive(Default)]
struct Interner {
    texts: Vec<Arc<str>>,
    numbers: HashMap<Arc<str>, u32>,
}

impl Interner {
    fn intern(&mut self, text: &str) -> u32 {
        if let Some(&number) = self.numbers.get(text) {
            return number;
        }
        let number = u32::try_from(self.texts.len()).expect("fewer than 2^32 texts");
        let text: Arc<str> = Arc::from(text);
        self.texts.push(Arc::clone(&text));
        self.numbers.insert(text, number);
        number
    }
}

/// Where the reading of the file stands.
struct LineReader {
    /// The bytes read past the last complete line, which wait for their
    /// `\n`: the writer's buffer may put a record's start on disk before
    /// its end.
    tail: Vec<u8>,
    /// Every byte read.
    read: u64,
    /// Where the line not ended yet begins: the bytes of the complete
    /// lines.
    line_start: u64,
    /// How many complete lines were taken.
    lines: u64,
}

/// The index of a session's trace file, which holds the file open and
/// deletes it when it is dropped or closed.
pub(crate) struct TraceIndex {
    /// The file; `None` once closed.
    file: Option<TraceFile>,
    reader: LineReader,
    contents: Contents,
    failure: Option<String>,
}

/// What the records read say.
struct Contents {
    header: Option<Header>,
    interner: Interner,
    events: Vec<IndexedEvent>,
    category_counts: [u64; TraceCategory::ALL.len()],
    listings: Vec<ListingEntry>,
    bodies: BTreeMap<u32, BodyTally>,
    /// `declined-no-body`'s reasons over the run, with their counts.
    no_body_declines: Vec<(u32, u32)>,
    sends: SendStates,
}

/// Counts one more `key` in `counts`.
fn count<T: PartialEq>(counts: &mut Vec<(T, u32)>, key: T, amount: u32) {
    match counts.iter_mut().find(|(known, _)| *known == key) {
        Some((_, total)) => *total += amount,
        None => counts.push((key, amount)),
    }
}

/// Locks `index`; a trace tool that panicked holding it left the index as
/// whole as any reading leaves it, every record taken or not.
pub(crate) fn lock(index: &Mutex<TraceIndex>) -> MutexGuard<'_, TraceIndex> {
    index.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TraceIndex {
    /// The index of `file`, which it holds from now on; nothing is read yet.
    pub(crate) fn over(file: TraceFile) -> TraceIndex {
        TraceIndex {
            file: Some(file),
            reader: LineReader {
                tail: Vec::new(),
                read: 0,
                line_start: 0,
                lines: 0,
            },
            contents: Contents {
                header: None,
                interner: Interner::default(),
                events: Vec::new(),
                category_counts: [0; TraceCategory::ALL.len()],
                listings: Vec::new(),
                bodies: BTreeMap::new(),
                no_body_declines: Vec::new(),
                sends: SendStates::new(),
            },
            failure: None,
        }
    }

    /// Reads the file from where the last reading stopped to its current
    /// end, taking every line ended by its `\n`; a line not ended yet
    /// waits for the next reading. A reading that failed stays failed.
    pub(crate) fn catch_up(&mut self) {
        if self.failure.is_some() {
            return;
        }
        let Some(trace_file) = &self.file else {
            return;
        };
        let mut file = trace_file.file();
        if let Err(error) = file.seek(SeekFrom::Start(self.reader.read)) {
            self.failure = Some(format!("the trace file cannot be read: {error}"));
            return;
        }
        let mut buffer = vec![0; READ_BYTES];
        loop {
            let read = match file.read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.failure = Some(format!("the trace file cannot be read: {error}"));
                    return;
                }
            };
            if let Err(failure) = self.reader.take(&buffer[..read], &mut self.contents) {
                self.failure = Some(failure);
                return;
            }
        }
    }

    /// Closes the file and deletes it; the index keeps what it read.
    pub(crate) fn close(&mut self) {
        self.file = None;
    }

    /// Whether the file is closed.
    fn closed(&self) -> bool {
        self.file.is_none()
    }

    /// The file's path, while it is open.
    pub(crate) fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(TraceFile::path)
    }

    /// The bytes of the complete lines read.
    pub(crate) fn bytes(&self) -> u64 {
        self.reader.line_start
    }

    pub(crate) fn event_count(&self) -> usize {
        self.contents.events.len()
    }

    /// How many bodies the file named, described or not.
    pub(crate) fn body_count(&self) -> usize {
        self.contents.bodies.len()
    }

    /// How many `bytecode`, `code` and `inlining` records were read.
    pub(crate) fn listing_count(&self) -> usize {
        self.contents.listings.len()
    }

    /// Why the reading ended before the file's end; every answer says it
    /// after what was read before.
    pub(crate) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// The header, once read.
    pub(crate) fn header(&self) -> Option<&Header> {
        self.contents.header.as_ref()
    }

    /// Whether the header says `category` is traced; `false` before it.
    pub(crate) fn traces(&self, category: TraceCategory) -> bool {
        self.header()
            .is_some_and(|header| header.categories.contains(category))
    }

    /// Whether the header says `kind` is listed; `false` before it.
    pub(crate) fn lists(&self, kind: ListingKind) -> bool {
        self.header()
            .is_some_and(|header| header.listings.contains(&kind))
    }

    /// The text interned as `number`.
    pub(crate) fn text(&self, number: u32) -> &str {
        &self.contents.interner.texts[number as usize]
    }

    /// The number `text` is interned as, if any event or listing gave it.
    pub(crate) fn interned(&self, text: &str) -> Option<u32> {
        self.contents.interner.numbers.get(text).copied()
    }

    /// Every event, in file order.
    pub(crate) fn events(&self) -> &[IndexedEvent] {
        &self.contents.events
    }

    /// How many events of `category` were read.
    pub(crate) fn category_count(&self, category: TraceCategory) -> u64 {
        TraceCategory::ALL
            .iter()
            .position(|known| *known == category)
            .map_or(0, |position| self.contents.category_counts[position])
    }

    /// Every listing record, in file order.
    pub(crate) fn listings(&self) -> &[ListingEntry] {
        &self.contents.listings
    }

    /// What the file says of the body `number`.
    pub(crate) fn body(&self, number: u32) -> Option<&BodyTally> {
        self.contents.bodies.get(&number)
    }

    /// Every body the file named, by number, the lowest first.
    pub(crate) fn bodies(&self) -> impl Iterator<Item = (u32, &BodyTally)> {
        self.contents
            .bodies
            .iter()
            .map(|(number, tally)| (*number, tally))
    }

    /// The name the body `number` is described by.
    pub(crate) fn body_name(&self, number: u32) -> Option<&str> {
        self.body(number)?
            .description
            .as_ref()
            .map(|description| description.name.as_str())
    }

    /// The `bytecode` record that lists the body `number`, or else its
    /// family root's, which lists every block of it.
    pub(crate) fn bytecode_of(&self, number: u32) -> Option<&ListingEntry> {
        let tally = self.body(number)?;
        let index = tally.bytecode.or_else(|| {
            let family = tally.description.as_ref()?.family?;
            self.body(family)?.bytecode
        })?;
        self.contents.listings.get(index as usize)
    }

    /// The body `number`'s installs of machine code by tier: those its
    /// events announced, and for a tier whose announcing category --
    /// `optimize` for optimized code, `compile` otherwise -- the header
    /// says is not traced, those its `code` records list.
    pub(crate) fn installs(&self, number: u32) -> Installs {
        let Some(tally) = self.body(number) else {
            return Installs::default();
        };
        let compile = self.traces(TraceCategory::Compile);
        let optimize = self.traces(TraceCategory::Optimize);
        Installs {
            baseline: if compile {
                tally.announced.baseline
            } else {
                tally.listed.baseline
            },
            twin: if compile {
                tally.announced.twin
            } else {
                tally.listed.twin
            },
            optimized: if optimize {
                tally.announced.optimized
            } else {
                tally.listed.optimized
            },
        }
    }

    /// `declined-no-body`'s reasons over the run, with their counts.
    pub(crate) fn no_body_declines(&self) -> &[(u32, u32)] {
        &self.contents.no_body_declines
    }

    /// What became of the send at `instruction` of the body `number`:
    /// its inlining where a copy's end -- `deoptimize` and `code-zone` --
    /// and its beginning can be heard, so that no decision stands for
    /// good, and its cache, by the site's number in the body's family.
    pub(crate) fn send_outcome(&self, number: u32, instruction: u32) -> SendOutcome {
        let sends = &self.contents.sends;
        let inlining_ends = sends.outcome_can_arrive
            && self.traces(TraceCategory::Deoptimize)
            && self.traces(TraceCategory::CodeZone);
        let inlining = inlining_ends
            .then(|| sends.decisions.get(&(number, instruction)).copied())
            .flatten();
        let description = self
            .body(number)
            .and_then(|tally| tally.description.as_ref());
        let root = description
            .and_then(|description| description.family)
            .unwrap_or(number);
        let cache = description
            .and_then(|description| {
                description
                    .sends
                    .iter()
                    .find(|send| send.instruction == instruction)
            })
            .and_then(|send| send.site)
            .and_then(|site| sends.caches.get(&(root, site)))
            .map(|history| CacheState {
                after: history.after,
                receivers: history.receivers.len(),
                event: history.event,
            });
        SendOutcome { inlining, cache }
    }

    /// The bodies `asked` names: `#<n>`, a body the file named, or a name
    /// in the tools' grammar, a trailing `...` making a prefix, matched as
    /// the trace filter matches -- a method's name its root and its blocks,
    /// every version and every edited body. A name of no module of the
    /// trace's or the binary's is refused as every tool refuses it, and a
    /// name of a known module the trace has no body of says so.
    pub(crate) fn bodies_named(&self, asked: &str) -> Result<Vec<u32>, String> {
        let asked = asked.trim();
        if let Some(number) = asked.strip_prefix('#') {
            return match number.parse::<u32>() {
                Ok(number) if self.contents.bodies.contains_key(&number) => Ok(vec![number]),
                _ => Err(format!("no body of the trace is named {asked}")),
            };
        }
        let filter = TraceFilter::parse(&[asked])?;
        let modules = crate::program::known_modules(
            self.contents
                .bodies
                .values()
                .filter_map(|tally| tally.description.as_ref())
                .map(|description| description.module.clone()),
        );
        filter.check(&|text: &str| KnownModule::resolve(text, &modules))?;
        let named: Vec<u32> = self
            .bodies()
            .filter(|(_, tally)| {
                tally
                    .description
                    .as_ref()
                    .is_some_and(|description| filter.matches(&description.name))
            })
            .map(|(number, _)| number)
            .collect();
        if named.is_empty() {
            return Err(format!("no body of the trace is named {asked}"));
        }
        Ok(named)
    }

    /// The record of `length` bytes at `offset`, read again through the
    /// server's own handle.
    pub(crate) fn read_record(&self, offset: u64, length: u32) -> Result<Value, String> {
        let Some(trace_file) = &self.file else {
            return Err("the trace file is closed".to_string());
        };
        let unreadable = |error: &dyn std::fmt::Display| {
            format!("the trace file no longer gives the record at byte {offset}: {error}")
        };
        let mut file = trace_file.file();
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| unreadable(&error))?;
        let mut bytes = vec![0; length as usize];
        file.read_exact(&mut bytes)
            .map_err(|error| unreadable(&error))?;
        serde_json::from_slice(&bytes).map_err(|error| unreadable(&error))
    }
}

/// Follows `index`'s file on a thread of its own, catching up 50 ms after
/// the file last grew, the wait doubling to 500 ms while it does not, so
/// that the first question after a long run finds the file read. The
/// thread holds the index weakly and ends once the session dropped it or
/// closed its file; `None` where no thread could be started, and the
/// tools then catch up alone.
pub(crate) fn spawn_follower(index: &Arc<Mutex<TraceIndex>>) -> Option<JoinHandle<()>> {
    let index = Arc::downgrade(index);
    std::thread::Builder::new()
        .name("ns-next mcp trace follower".to_string())
        .spawn(move || {
            let mut wait = POLL;
            loop {
                std::thread::sleep(wait);
                let Some(index) = index.upgrade() else {
                    return;
                };
                let mut index = lock(&index);
                if index.closed() || index.failure().is_some() {
                    return;
                }
                let before = index.reader.read;
                index.catch_up();
                wait = if index.reader.read > before {
                    POLL
                } else {
                    (wait * 2).min(LONGEST_POLL)
                };
            }
        })
        .ok()
}

impl LineReader {
    /// Takes `bytes`, the next read from the file, into `contents`, line
    /// by line; answers why a line ended the reading.
    fn take(&mut self, bytes: &[u8], contents: &mut Contents) -> Result<(), String> {
        let first = self.read;
        self.read += bytes.len() as u64;
        let mut start = 0;
        while let Some(found) = bytes[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + found;
            // Bytes, never characters: the offset is what a later read
            // seeks to.
            let newline_at = first + end as u64;
            let offset = self.line_start;
            let length = u32::try_from(newline_at - offset).map_err(|_| {
                format!(
                    "line {} of the trace file is no record: longer than 4 GiB",
                    self.lines + 1
                )
            })?;
            let mut tail = std::mem::take(&mut self.tail);
            let line: &[u8] = if tail.is_empty() {
                &bytes[start..end]
            } else {
                tail.extend_from_slice(&bytes[start..end]);
                &tail
            };
            self.lines += 1;
            contents.take_line(line, self.lines, offset, length)?;
            tail.clear();
            self.tail = tail;
            self.line_start = newline_at + 1;
            start = end + 1;
        }
        self.tail.extend_from_slice(&bytes[start..]);
        Ok(())
    }
}

/// A module as a record names it.
#[derive(Deserialize)]
struct ModuleHead {
    id: String,
    version: Option<String>,
}

/// A `body` record's `source`.
#[derive(Deserialize)]
struct SourceHead {
    file: Option<String>,
    library: Option<String>,
}

/// A `body` record's send.
#[derive(Deserialize)]
struct SendHead {
    #[serde(rename = "at")]
    instruction: u32,
    site: Option<u32>,
    call: Option<[u32; 4]>,
    name: Option<String>,
}

/// A body a `bytecode` record lists.
#[derive(Deserialize)]
struct ListedBodyHead {
    body: u32,
}

/// One of `declined-no-body`'s reasons.
#[derive(Deserialize)]
struct ReasonHead<'line> {
    #[serde(borrow)]
    reason: Cow<'line, str>,
    count: u32,
}

/// An `inlining` record's outcome.
#[derive(Deserialize)]
struct OutcomeHead<'line> {
    installed: Option<u64>,
    #[serde(borrow)]
    refused: Option<Cow<'line, str>>,
}

/// The fields of a record the index keeps, of every kind at once: a field
/// a record does not have stays `None`, and every field the index does
/// not keep -- a listing's instructions, a body's statements -- is
/// skipped unbuilt.
#[derive(Deserialize)]
struct RecordHead<'line> {
    #[serde(borrow)]
    kind: Cow<'line, str>,
    // The header's.
    #[serde(borrow)]
    format: Option<Cow<'line, str>>,
    /// The header's format version, and a `retired` event's version.
    version: Option<u64>,
    architecture: Option<String>,
    #[serde(borrow)]
    categories: Option<Vec<Cow<'line, str>>>,
    #[serde(borrow)]
    listings: Option<Vec<Cow<'line, str>>>,
    // A body's.
    body: Option<u32>,
    name: Option<String>,
    family: Option<u32>,
    module: Option<ModuleHead>,
    source: Option<SourceHead>,
    range: Option<[u32; 4]>,
    placed: Option<String>,
    checksum: Option<String>,
    sends: Option<Vec<SendHead>>,
    // A bytecode listing's.
    bodies: Option<Vec<ListedBodyHead>>,
    // A code listing's, beside `body`, `key`, `entered` and `bytes`.
    #[serde(borrow)]
    tier: Option<Cow<'line, str>>,
    // An inlining listing's.
    #[serde(borrow)]
    outcome: Option<OutcomeHead<'line>>,
    // An event's.
    #[serde(borrow)]
    category: Option<Cow<'line, str>>,
    #[serde(borrow)]
    event: Option<Cow<'line, str>>,
    #[serde(borrow)]
    trigger: Option<Cow<'line, str>>,
    bytes: Option<u64>,
    #[serde(borrow)]
    reason: Option<Cow<'line, str>>,
    exits: Option<u64>,
    #[serde(rename = "by", borrow)]
    revival: Option<Cow<'line, str>>,
    /// An event's program counter or loop head.
    #[serde(rename = "at")]
    program_counter: Option<u32>,
    #[serde(borrow)]
    key: Option<Cow<'line, str>>,
    #[serde(borrow)]
    entered: Option<Cow<'line, str>>,
    #[serde(rename = "spent-by")]
    spent_by: Option<u32>,
    #[serde(rename = "walk-end", borrow)]
    walk_end: Option<Cow<'line, str>>,
    #[serde(borrow)]
    cause: Option<Cow<'line, str>>,
    #[serde(rename = "inlined-scopes")]
    inlined_scopes: Option<u64>,
    #[serde(rename = "inlined-sites")]
    inlined_sites: Option<u64>,
    #[serde(rename = "type-case-arms")]
    type_case_arms: Option<u64>,
    #[serde(rename = "eliminated-blocks")]
    eliminated_blocks: Option<u64>,
    #[serde(rename = "in-body")]
    in_body: Option<u32>,
    callee: Option<u32>,
    count: Option<u64>,
    #[serde(borrow)]
    reasons: Option<Vec<ReasonHead<'line>>>,
    #[serde(borrow)]
    freeing: Option<Cow<'line, str>>,
    #[serde(rename = "bodies-freed")]
    bodies_freed: Option<u64>,
    #[serde(borrow)]
    before: Option<Cow<'line, str>>,
    #[serde(borrow)]
    after: Option<Cow<'line, str>>,
    #[serde(borrow)]
    selector: Option<Cow<'line, str>>,
    site: Option<u32>,
    #[serde(borrow)]
    receiver: Option<Cow<'line, str>>,
}

/// An event field's value as its text line spells it.
enum Spelled<'head> {
    /// A word or a number, as it is.
    Plain(String),
    /// A class's name, in braces.
    Name(&'head str),
    /// A body's number, `{#<n>}`.
    Body(u32),
}

/// The fields the text line writes after `word`, in its order
/// (`trace::line`); a field the record leaves `null` -- an event's absent
/// key -- is not written.
fn field_order(word: &str) -> &'static [&'static str] {
    match word {
        "compiled" => &["trigger", "bytes"],
        "compiled-twin" => &["bytes"],
        // A compile's refusal has a trigger, an optimizing compile's a key.
        "refused" => &["key", "entered", "trigger", "reason"],
        "held" => &["trigger", "reason"],
        "held-twin" | "refused-abandoned" => &["reason"],
        "abandoned" => &["exits"],
        "revived" => &["by"],
        "osr-compiled" | "osr-entered" => &["at"],
        "osr-refused" => &["at", "reason"],
        "requested" | "not-due" => &["key", "entered", "spent-by", "walk-end"],
        "dropped" | "invalidated" => &["key", "entered", "cause"],
        "deferred" => &["key", "entered", "reason"],
        "optimized" => &[
            "key",
            "entered",
            "bytes",
            "inlined-scopes",
            "inlined-sites",
            "type-case-arms",
            "eliminated-blocks",
        ],
        "inlined" => &["key", "entered", "in-body", "callee", "at"],
        "declined" => &["key", "entered", "in-body", "callee", "reason", "at"],
        "declined-no-body" => &["key", "entered", "count", "reasons"],
        "retired" => &["key", "entered", "reason", "version"],
        "compacted" => &["freeing", "bodies-freed"],
        "freed" => &["key", "entered", "tier", "cause"],
        "replaced" => &["bodies-freed"],
        "transition" => &["selector", "site", "receiver"],
        "vacated" => &["cause"],
        _ => &[],
    }
}

impl RecordHead<'_> {
    /// The event field `field` as the text line spells it; `None` where
    /// the record has none.
    fn spelled(&self, field: &str) -> Option<Spelled<'_>> {
        let plain = |text: &Option<Cow<'_, str>>| {
            text.as_ref().map(|text| Spelled::Plain(text.to_string()))
        };
        let number = |value: Option<u64>| value.map(|value| Spelled::Plain(value.to_string()));
        match field {
            "trigger" => plain(&self.trigger),
            "bytes" => number(self.bytes),
            "reason" => plain(&self.reason),
            "exits" => number(self.exits),
            "by" => plain(&self.revival),
            "at" => number(self.program_counter.map(u64::from)),
            "key" => self.key.as_deref().map(Spelled::Name),
            "entered" => self.entered.as_deref().map(Spelled::Name),
            "spent-by" => self.spent_by.map(Spelled::Body),
            "walk-end" => plain(&self.walk_end),
            "cause" => plain(&self.cause),
            "inlined-scopes" => number(self.inlined_scopes),
            "inlined-sites" => number(self.inlined_sites),
            "type-case-arms" => number(self.type_case_arms),
            "eliminated-blocks" => number(self.eliminated_blocks),
            "in-body" => self.in_body.map(Spelled::Body),
            "callee" => self.callee.map(Spelled::Body),
            "count" => number(self.count),
            "reasons" => self.reasons.as_ref().map(|reasons| {
                let spelled: Vec<String> = reasons
                    .iter()
                    .map(|reason| format!("{}:{}", reason.reason, reason.count))
                    .collect();
                Spelled::Plain(spelled.join(","))
            }),
            // A retirement past every copy compiled without traps has none.
            "version" => Some(Spelled::Plain(
                self.version
                    .map_or_else(|| "none".to_string(), |version| version.to_string()),
            )),
            "freeing" => plain(&self.freeing),
            "bodies-freed" => number(self.bodies_freed),
            "tier" => plain(&self.tier),
            "selector" => self
                .selector
                .as_ref()
                .map(|selector| Spelled::Plain(format!("#{selector}"))),
            "site" => number(self.site.map(u64::from)),
            "receiver" => self.receiver.as_deref().map(Spelled::Name),
            _ => None,
        }
    }

    /// The fields of the event `word` as its text line writes them after
    /// the word, each body field `{#<n>}`.
    fn fields_text(&self, word: &str) -> String {
        let mut text = String::new();
        for field in field_order(word) {
            match self.spelled(field) {
                Some(Spelled::Plain(value)) => {
                    let _ = write!(text, " {field}={value}");
                }
                Some(Spelled::Name(name)) => {
                    let _ = write!(text, " {field}={{{name}}}");
                }
                Some(Spelled::Body(number)) => {
                    let _ = write!(text, " {field}={{#{number}}}");
                }
                None => {}
            }
        }
        text
    }
}

/// The field `field` a record of `kind` cannot do without.
fn required<T>(value: Option<T>, kind: &str, field: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("a `{kind}` record without `{field}`"))
}

impl Contents {
    /// Takes the line `number`, of `length` bytes at `offset`.
    fn take_line(
        &mut self,
        line: &[u8],
        number: u64,
        offset: u64,
        length: u32,
    ) -> Result<(), String> {
        let head = serde_json::from_slice::<RecordHead<'_>>(line);
        if number == 1 {
            let Ok(head) = head else {
                return Err(NO_TRACE.to_string());
            };
            return self.take_header(&head);
        }
        let head =
            head.map_err(|error| format!("line {number} of the trace file is no record: {error}"))?;
        let taken = match head.kind.as_ref() {
            "body" => self.take_body(head),
            "event" => self.take_event(&head, offset),
            "bytecode" | "code" | "inlining" => self.take_listing(&head, offset, length),
            // A kind this reader does not know -- the header again among
            // them -- is skipped, as the plugin's reader skips it.
            _ => Ok(()),
        };
        taken.map_err(|error| format!("line {number} of the trace file is no record: {error}"))
    }

    fn take_header(&mut self, head: &RecordHead<'_>) -> Result<(), String> {
        if head.kind != "header" || head.format.as_deref() != Some(FORMAT) {
            return Err(NO_TRACE.to_string());
        }
        let version = head.version.unwrap_or(0);
        if version != u64::from(VERSION) {
            return Err(format!(
                "the trace file has version {version}; this server reads version {VERSION}"
            ));
        }
        let mut categories = TraceCategories::default();
        for name in head.categories.iter().flatten() {
            let category = TraceCategory::from_name(name)
                .ok_or_else(|| format!("line 1 of the trace file names no category `{name}`"))?;
            categories = categories.with(category);
        }
        let listings: Vec<ListingKind> = head
            .listings
            .iter()
            .flatten()
            .filter_map(|name| ListingKind::from_name(name))
            .collect();
        self.sends.outcome_can_arrive = categories.contains(TraceCategory::Optimize)
            || listings.contains(&ListingKind::Inlining);
        self.header = Some(Header {
            architecture: head.architecture.clone(),
            categories,
            listings,
        });
        Ok(())
    }

    /// The tally of the body `number`, made when the file first names it.
    fn tally(&mut self, number: u32) -> &mut BodyTally {
        self.bodies.entry(number).or_default()
    }

    fn take_body(&mut self, head: RecordHead<'_>) -> Result<(), String> {
        let number = required(head.body, "body", "body")?;
        let module = required(head.module, "body", "module")?;
        let source = match head.source {
            Some(SourceHead {
                file: Some(file), ..
            }) => BodySource::File(file),
            Some(SourceHead {
                library: Some(library),
                ..
            }) => BodySource::Library(library),
            _ => BodySource::Nowhere,
        };
        let sends = head
            .sends
            .unwrap_or_default()
            .into_iter()
            .map(|send| DescribedSend {
                instruction: send.instruction,
                site: send.site,
                call: send.call,
                name: send.name,
            })
            .collect();
        let description = BodyRecord {
            name: required(head.name, "body", "name")?,
            family: head.family,
            module: ModuleName {
                id: module.id,
                version: module.version,
            },
            source,
            range: head.range,
            placed: head.placed,
            checksum: head.checksum.unwrap_or_default(),
            sends,
        };
        if let Some(family) = description.family {
            self.tally(family);
        }
        self.tally(number).description = Some(description);
        Ok(())
    }

    fn intern_optional(&mut self, text: Option<&str>) -> Option<u32> {
        text.map(|text| self.interner.intern(text))
    }

    fn take_listing(
        &mut self,
        head: &RecordHead<'_>,
        offset: u64,
        length: u32,
    ) -> Result<(), String> {
        let kind = ListingKind::from_name(&head.kind).expect("a listing's kind");
        let body = required(head.body, &head.kind, "body")?;
        let index = u32::try_from(self.listings.len()).expect("fewer than 2^32 listings");
        let key = CopyKey {
            key: self.intern_optional(head.key.as_deref()),
            entered: self.intern_optional(head.entered.as_deref()),
        };
        let tier = self.intern_optional(head.tier.as_deref());
        let outcome = match &head.outcome {
            Some(OutcomeHead {
                installed: Some(bytes),
                ..
            }) => Some(PlanOutcome::Installed(*bytes)),
            Some(OutcomeHead {
                refused: Some(reason),
                ..
            }) => Some(PlanOutcome::Refused(self.interner.intern(reason))),
            _ => None,
        };
        match kind {
            ListingKind::Bytecode => {
                self.tally(body).bytecode = Some(index);
                for listed in head.bodies.iter().flatten() {
                    self.tally(listed.body).bytecode = Some(index);
                }
            }
            ListingKind::Code => {
                self.sends.leave_open_compile();
                let tally = self.tally(body);
                tally.code.push(index);
                match head.tier.as_deref() {
                    Some("optimized") => tally.listed.optimized += 1,
                    Some("twin") => tally.listed.twin += 1,
                    _ => tally.listed.baseline += 1,
                }
            }
            ListingKind::Inlining => {
                self.tally(body).inlining.push(index);
                // The plan's own outcome, for a trace without `optimize`.
                self.sends.conclude(
                    CompileKey { body, key },
                    matches!(outcome, Some(PlanOutcome::Installed(_))),
                );
            }
        }
        self.listings.push(ListingEntry {
            kind,
            body,
            tier,
            key: key.key,
            entered: key.entered,
            bytes: if kind == ListingKind::Code {
                head.bytes
            } else {
                None
            },
            outcome,
            offset,
            length,
        });
        Ok(())
    }

    fn take_event(&mut self, head: &RecordHead<'_>, offset: u64) -> Result<(), String> {
        let category_name = required(head.category.as_deref(), "event", "category")?;
        let category = TraceCategory::from_name(category_name)
            .ok_or_else(|| format!("no category `{category_name}`"))?;
        let word = required(head.event.as_deref(), "event", "event")?;
        let index = u32::try_from(self.events.len()).expect("fewer than 2^32 events");
        let line_word = match (word, &head.before, &head.after) {
            ("transition", Some(before), Some(after)) => format!("{before}->{after}"),
            _ => word.to_string(),
        };
        let fields = head.fields_text(word);
        let prefix = head
            .body
            .map(|body| format!("#{body} "))
            .unwrap_or_default();
        let text = format!("{prefix}[{category_name}] {line_word}{fields}");
        let reason_text = head
            .reason
            .as_deref()
            .or(head.cause.as_deref())
            .or(head.revival.as_deref())
            .or(head.walk_end.as_deref());
        let reasons = head.reasons.as_ref().map(|reasons| {
            reasons
                .iter()
                .map(|reason| (self.interner.intern(&reason.reason), reason.count))
                .collect::<Box<[(u32, u32)]>>()
        });
        let event = IndexedEvent {
            offset,
            category,
            event: self.interner.intern(word),
            word: self.interner.intern(&line_word),
            reason: self.intern_optional(reason_text),
            reasons,
            body: head.body,
            spent_by: head.spent_by,
            in_body: head.in_body,
            callee: head.callee,
            key: self.intern_optional(head.key.as_deref()),
            entered: self.intern_optional(head.entered.as_deref()),
            fields: self.interner.intern(&fields),
            text: self.interner.intern(&text),
        };
        if let Some(position) = TraceCategory::ALL
            .iter()
            .position(|known| *known == category)
        {
            self.category_counts[position] += 1;
        }
        for named in [event.body, event.spent_by, event.in_body, event.callee]
            .into_iter()
            .flatten()
        {
            self.tally(named);
        }
        self.tally_event(head, &event, index);
        self.follow_sends(head, &event, index);
        self.events.push(event);
        Ok(())
    }

    /// Counts `event`, the `index`th, into its bodies' tallies.
    fn tally_event(&mut self, head: &RecordHead<'_>, event: &IndexedEvent, index: u32) {
        let word = self.interner.texts[event.event as usize].clone();
        let copy_key = CopyKey {
            key: event.key,
            entered: event.entered,
        };
        if let Some(body) = event.body {
            let tally = self.tally(body);
            tally.events.push(index);
            match &*word {
                "compiled" => tally.announced.baseline += 1,
                "compiled-twin" => tally.announced.twin += 1,
                "optimized" => {
                    tally.announced.optimized += 1;
                    let bytes = head.bytes.unwrap_or(0);
                    match tally
                        .copies
                        .iter_mut()
                        .find(|copies| copies.key == copy_key)
                    {
                        Some(copies) => {
                            copies.count += 1;
                            copies.last_bytes = bytes;
                        }
                        None => tally.copies.push(Copies {
                            key: copy_key,
                            count: 1,
                            last_bytes: bytes,
                        }),
                    }
                }
                "abandoned" => {
                    tally.abandoned += 1;
                    tally.exits += head.exits.unwrap_or(0);
                }
                "revived" => {
                    if let Some(revival) = event.reason {
                        count(&mut tally.revived, revival, 1);
                    }
                }
                "retired" | "invalidated" => {
                    count(&mut tally.deoptimizations, (event.event, event.reason), 1);
                }
                "declined" => {
                    if let Some(reason) = event.reason {
                        count(&mut tally.declined_as_compiler, reason, 1);
                    }
                }
                _ => {}
            }
        }
        match &*word {
            "declined" => {
                if let Some(callee) = event.callee {
                    let tally = self.tally(callee);
                    if let Some(reason) = event.reason {
                        count(&mut tally.declined_as_callee, reason, 1);
                    }
                    if let Some(body) = event.body {
                        count(&mut tally.declined_in, body, 1);
                    }
                }
            }
            "inlined" => {
                if let (Some(callee), Some(body)) = (event.callee, event.body) {
                    count(&mut self.tally(callee).inlined_into, (body, event.key), 1);
                }
            }
            "declined-no-body" => {
                for &(reason, reason_count) in event.reasons.iter().flatten() {
                    count(&mut self.no_body_declines, reason, reason_count);
                }
            }
            _ => {}
        }
    }

    /// Follows what `event`, the `index`th, says of the sends, as the
    /// plugin's `TraceRun` does.
    fn follow_sends(&mut self, head: &RecordHead<'_>, event: &IndexedEvent, index: u32) {
        let word = self.interner.texts[event.event as usize].clone();
        let key = CopyKey {
            key: event.key,
            entered: event.entered,
        };
        if event.category == TraceCategory::Inlining {
            // A compile's inlining events, whatever their kind, belong to
            // its stretch.
            if let Some(body) = event.body {
                self.sends.continue_compile(CompileKey { body, key });
            }
            if matches!(&*word, "inlined" | "declined")
                && event.body.is_some()
                && self.sends.outcome_can_arrive
                && let (Some(in_body), Some(program_counter)) =
                    (event.in_body, head.program_counter)
            {
                self.sends
                    .open_decisions
                    .push(((in_body, program_counter), index));
            }
            return;
        }
        let concluded = event.body.is_some()
            && event.category == TraceCategory::Optimize
            && matches!(&*word, "optimized" | "refused" | "deferred");
        if !concluded {
            self.sends.leave_open_compile();
        }
        match &*word {
            "transition" => {
                if let (Some(body), Some(site), Some(after)) = (event.body, head.site, &head.after)
                {
                    let after = self.interner.intern(after);
                    let receiver = self.intern_optional(head.receiver.as_deref());
                    let history =
                        self.sends
                            .caches
                            .entry((body, site))
                            .or_insert_with(|| CacheHistory {
                                after,
                                receivers: Vec::new(),
                                event: index,
                            });
                    if head.before.as_deref() == Some("vacant") {
                        history.receivers.clear();
                    }
                    if let Some(receiver) = receiver
                        && !history.receivers.contains(&receiver)
                    {
                        history.receivers.push(receiver);
                    }
                    history.after = after;
                    history.event = index;
                }
            }
            // Every site vacated at once.
            "vacated" => self.sends.caches.clear(),
            // The zone dropped every body's machine code, copies included.
            "replaced" => {
                self.sends.live_copies.clear();
                self.sends.decisions.clear();
            }
            _ => {
                if let Some(body) = event.body {
                    let compile = CompileKey { body, key };
                    if concluded {
                        self.sends.conclude(compile, &*word == "optimized");
                    }
                    let optimized_freed =
                        &*word == "freed" && head.tier.as_deref() == Some("optimized");
                    if matches!(&*word, "retired" | "invalidated") || optimized_freed {
                        self.sends.copy_gone(compile);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::trace_files::TraceDirectory;
    use super::*;
    use crate::trace::tests::{every_event, split_outside_braces};
    use crate::trace_file::event_record;
    use ns_runtime::TraceEventKind;
    use ns_runtime::trace_events::{BodyName, CompileTrigger, Revival, TraceEvent};
    use serde_json::json;
    use std::fs::File;
    use std::io::Write as _;
    use std::path::PathBuf;

    /// The fields whose values name a body, by the line's names.
    const BODY_FIELDS: [&str; 3] = ["in-body", "callee", "spent-by"];

    /// A trace file of the test `name`'s own, in a scratch directory the
    /// index deletes it from, and a writer appending to it as the child's
    /// handle does.
    struct Scratch {
        /// Dropped first: on Windows an open handle refuses the deletion.
        writer: File,
        index: TraceIndex,
        _directory: Arc<TraceDirectory>,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let directory = std::env::temp_dir()
                .join(format!("ns-next-trace-index-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            let directory = TraceDirectory::scratch(directory);
            let trace_file = directory.create(1).expect("the scratch trace file");
            let writer = File::options()
                .append(true)
                .open(trace_file.path())
                .expect("the writer's handle");
            Scratch {
                index: TraceIndex::over(trace_file),
                writer,
                _directory: directory,
            }
        }

        fn write(&mut self, text: &str) {
            self.write_bytes(text.as_bytes());
        }

        fn write_bytes(&mut self, bytes: &[u8]) {
            self.writer.write_all(bytes).expect("written");
            self.writer.flush().expect("flushed");
        }

        /// `record` as its line.
        fn record(&mut self, record: &Value) {
            self.write(&format!("{record}\n"));
        }

        fn header(&mut self, categories: &[&str], listings: &[&str]) {
            self.record(&json!({
                "kind": "header",
                "format": FORMAT,
                "version": VERSION,
                "architecture": "x64",
                "categories": categories,
                "listings": listings,
            }));
        }

        fn body(&mut self, number: u32, name: &str, family: Option<u32>) {
            self.record(&body_record(number, name, family));
        }
    }

    fn body_record(number: u32, name: &str, family: Option<u32>) -> Value {
        json!({
            "kind": "body",
            "body": number,
            "name": name,
            "family": family,
            "module": { "id": "Demo", "version": null },
            "source": { "file": "/work/Demo.ns" },
            "range": [1, 2, 1, 5],
            "placed": "source",
            "checksum": "0000000000000001",
            "statements": [{ "instructions": [0, 1], "range": null }],
            "sends": [{ "at": 0, "site": 0, "call": null, "selector": [] }],
        })
    }

    fn about(number: u32, kind: TraceEventKind) -> TraceEvent {
        TraceEvent {
            category: TraceCategory::Compile,
            body: Some(BodyName {
                number,
                name: "Demo>>run".to_string(),
            }),
            kind,
        }
    }

    #[test]
    fn every_event_is_indexed() {
        let mut scratch = Scratch::new("every-event");
        scratch.header(&["compile"], &[]);
        let events = every_event();
        for (event, _) in &events {
            scratch.record(&event_record(event));
        }
        scratch.index.catch_up();
        let index = &scratch.index;
        assert_eq!(index.failure(), None);
        assert_eq!(index.event_count(), events.len());
        for ((event, line), indexed) in events.iter().zip(index.events()) {
            let record = event_record(event);
            assert_eq!(indexed.category, event.category, "{line}");
            assert_eq!(index.text(indexed.event), record["event"], "{line}");
            // The text line with every body named by its number: each body
            // of `every_event` is numbered 1.
            let parts = split_outside_braces(line);
            let mut expected = String::new();
            if parts[1] != "{}" {
                expected.push_str("#1 ");
            }
            expected.push_str(&format!("{} {}", parts[0], parts[2]));
            for field in &parts[3..] {
                let (name, value) = field.split_once('=').expect("a field is name=value");
                if BODY_FIELDS.contains(&name) {
                    expected.push_str(&format!(" {name}={{#1}}"));
                } else {
                    expected.push_str(&format!(" {name}={value}"));
                }
            }
            assert_eq!(index.text(indexed.text), expected, "{line}");
            assert_eq!(index.text(indexed.word), parts[2], "{line}");
        }
        // Each event's record is read again whole from its offset.
        let last = index.events().last().expect("an event");
        let length = u32::try_from(index.bytes() - 1 - last.offset).unwrap();
        let record = index.read_record(last.offset, length).expect("the record");
        assert_eq!(record, event_record(&events.last().expect("an event").0));
    }

    #[test]
    fn a_partial_last_line_waits_for_its_end() {
        let mut scratch = Scratch::new("partial-line");
        scratch.header(&["compile"], &[]);
        let record = body_record(1, "Demo>>run", None).to_string();
        // Cut inside a multi-byte character, as a buffer's flush may.
        let record = record.replace("Demo>>run", "Demo>>rün").into_bytes();
        let cut = record.iter().position(|byte| *byte >= 0x80).unwrap() + 1;
        scratch.write_bytes(&record[..cut]);
        scratch.index.catch_up();
        assert_eq!(scratch.index.body_count(), 0);
        assert_eq!(scratch.index.failure(), None);
        scratch.write_bytes(&record[cut..]);
        scratch.index.catch_up();
        assert_eq!(scratch.index.body_count(), 0);
        assert_eq!(scratch.index.failure(), None);
        scratch.write("\n");
        scratch.index.catch_up();
        assert_eq!(scratch.index.failure(), None);
        assert_eq!(scratch.index.body_count(), 1);
        assert_eq!(scratch.index.body_name(1), Some("Demo>>rün"));
        // The bytes counted are every byte of the complete lines.
        let size = std::fs::metadata(scratch.index.path().unwrap())
            .unwrap()
            .len();
        assert_eq!(scratch.index.bytes(), size);
    }

    /// A code listing of a hundred instructions, as the writer writes it.
    #[cfg(target_arch = "x86_64")]
    fn hundred_instructions() -> ns_runtime::trace_events::Listing {
        use ns_runtime::trace_events::{
            Annotation, Architecture, CodeListing, CodeTier, DataWord, ListedScope, Listing,
            Position,
        };
        let body = BodyName {
            number: 2,
            name: "Demo>>answer".to_string(),
        };
        // A hundred one-byte instructions and a data word.
        let mut bytes = vec![0x90; 99];
        bytes.push(0xC3);
        bytes.extend([0; 8]);
        Listing::Code(CodeListing {
            body: body.clone(),
            tier: CodeTier::Baseline,
            key: None,
            architecture: Architecture::X64,
            base_address: 0x4000,
            bytes,
            code_end: 100,
            words: vec![(100, DataWord::Budget)],
            positions: vec![(
                0,
                Position::Instruction {
                    scope: 0,
                    program_counter: 0,
                },
            )],
            annotations: vec![(0, Annotation::Leaf("miss"))],
            scopes: vec![ListedScope {
                number: 0,
                body,
                parent: None,
                instructions: Vec::new(),
            }],
            mark: None,
        })
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_listing_is_kept_as_its_place() {
        let mut scratch = Scratch::new("listing-place");
        scratch.header(&[], &["code"]);
        let listing = hundred_instructions();
        let record = crate::trace_file::listing_record(&listing);
        assert_eq!(record["instructions"].as_array().unwrap().len(), 100);
        let line = record.to_string();
        scratch.write(&format!("{line}\n"));
        scratch.index.catch_up();
        let index = &scratch.index;
        assert_eq!(index.failure(), None);
        assert_eq!(index.listing_count(), 1);
        let entry = &index.listings()[0];
        assert_eq!(entry.kind, ListingKind::Code);
        assert_eq!(entry.body, 2);
        assert_eq!(entry.bytes, Some(108));
        assert_eq!(entry.tier.map(|tier| index.text(tier)), Some("baseline"));
        let header_length = index.bytes() - line.len() as u64 - 1;
        assert_eq!(entry.offset, header_length);
        assert_eq!(entry.length as usize, line.len());
        assert_eq!(index.read_record(entry.offset, entry.length), Ok(record));
        assert_eq!(index.body(2).unwrap().code, vec![0]);
        // No code record's instruction is kept: the listed installs count it.
        assert_eq!(index.installs(2).baseline, 1);
    }

    #[test]
    fn another_version_is_refused() {
        let mut scratch = Scratch::new("version");
        scratch.record(&json!({
            "kind": "header",
            "format": FORMAT,
            "version": 2,
            "architecture": null,
            "categories": [],
            "listings": [],
        }));
        scratch.body(1, "Demo>>run", None);
        scratch.index.catch_up();
        assert_eq!(
            scratch.index.failure(),
            Some("the trace file has version 2; this server reads version 1")
        );
        assert_eq!(scratch.index.body_count(), 0);
    }

    #[test]
    fn a_file_of_another_format_is_refused() {
        let mut scratch = Scratch::new("format");
        scratch.write("{\"kind\":\"header\",\"format\":\"other\",\"version\":1}\n");
        scratch.index.catch_up();
        assert_eq!(scratch.index.failure(), Some(NO_TRACE));
        let mut scratch = Scratch::new("no-json");
        scratch.write("not a record\n");
        scratch.index.catch_up();
        assert_eq!(scratch.index.failure(), Some(NO_TRACE));
    }

    /// A line that is no record ends the reading, naming it, and keeps
    /// what was read before.
    #[test]
    fn a_malformed_line_ends_the_reading() {
        let mut scratch = Scratch::new("malformed");
        scratch.header(&[], &[]);
        scratch.body(1, "Demo>>run", None);
        scratch.write("{\"kind\":\"event\"\n");
        scratch.body(2, "Demo>>other", None);
        scratch.index.catch_up();
        let failure = scratch.index.failure().expect("a failure");
        assert!(
            failure.starts_with("line 3 of the trace file is no record: "),
            "{failure}"
        );
        assert_eq!(scratch.index.body_count(), 1);
        scratch.index.catch_up();
        assert_eq!(scratch.index.body_count(), 1);
    }

    #[test]
    fn tallies_count_installs_and_abandonments() {
        let mut scratch = Scratch::new("tallies");
        scratch.header(&["compile"], &[]);
        for _ in 0..3 {
            for kind in [
                TraceEventKind::Compiled {
                    trigger: CompileTrigger::Call,
                    bytes: 1216,
                },
                TraceEventKind::Abandoned { exits: 64 },
                TraceEventKind::Revived {
                    revival: Revival::Decay,
                },
            ] {
                scratch.record(&event_record(&about(7, kind)));
            }
        }
        scratch.index.catch_up();
        let index = &scratch.index;
        assert_eq!(index.failure(), None);
        assert_eq!(
            index.installs(7),
            Installs {
                baseline: 3,
                twin: 0,
                optimized: 0
            }
        );
        let tally = index.body(7).expect("body 7");
        assert_eq!((tally.abandoned, tally.exits), (3, 192));
        let revived: Vec<(&str, u32)> = tally
            .revived
            .iter()
            .map(|(revival, count)| (index.text(*revival), *count))
            .collect();
        assert_eq!(revived, vec![("decay", 3)]);
        assert_eq!(tally.events.len(), 9);
        assert_eq!(index.category_count(TraceCategory::Compile), 9);
        // The cycle's three events are three texts, each the same thrice.
        let texts: Vec<u32> = index.events().iter().map(|event| event.text).collect();
        assert_eq!(texts[0..3], texts[3..6]);
        assert_ne!(texts[0], texts[1]);
        assert_eq!(
            index.text(texts[0]),
            "#7 [compile] compiled trigger=call bytes=1216"
        );
    }

    #[test]
    fn a_method_name_matches_its_blocks() {
        let mut scratch = Scratch::new("names");
        scratch.header(&["compile"], &[]);
        scratch.body(1, "Demo>>run", None);
        scratch.body(2, "[] in Demo>>run", Some(1));
        scratch.body(3, "Demo>>other", None);
        scratch.index.catch_up();
        let index = &scratch.index;
        assert_eq!(index.bodies_named("Demo>>run"), Ok(vec![1, 2]));
        assert_eq!(index.bodies_named("#3"), Ok(vec![3]));
        assert_eq!(
            index.bodies_named("Demo>>none"),
            Err("no body of the trace is named Demo>>none".to_string())
        );
        assert_eq!(index.bodies_named("Demo>>..."), Ok(vec![1, 2, 3]));
        assert_eq!(
            index.bodies_named("#9"),
            Err("no body of the trace is named #9".to_string())
        );
        // A name of no module is refused as every tool refuses it.
        let refusal = index.bodies_named("Nowhere>>run").unwrap_err();
        assert!(refusal.starts_with("`Nowhere>>run`: "), "{refusal}");
        // A shipped module is known, though the trace has no body of it.
        assert_eq!(
            index.bodies_named("newspeak.core.Kernel>>none"),
            Err("no body of the trace is named newspeak.core.Kernel>>none".to_string())
        );
    }

    /// A plan's decisions stand once its copy is heard installed and go
    /// with the copy; a cache keeps its last state by its family's site.
    #[test]
    fn a_send_keeps_its_latest_inlining_and_cache() {
        let mut scratch = Scratch::new("sends");
        scratch.header(
            &["optimize", "inlining", "deoptimize", "code-zone", "caches"],
            &[],
        );
        scratch.body(1, "Demo>>run", None);
        scratch.body(2, "[] in Demo>>run", Some(1));
        let event = |category: &str, body: u32, fields: Value| {
            let mut record = json!({ "kind": "event", "category": category, "body": body });
            for (name, value) in fields.as_object().unwrap() {
                record[name] = value.clone();
            }
            record
        };
        let declined = event(
            "inlining",
            1,
            json!({ "event": "declined", "key": "Demo", "entered": null, "in-body": 1,
                    "callee": 3, "reason": "over-weight", "at": 0 }),
        );
        let optimized = event(
            "optimize",
            1,
            json!({ "event": "optimized", "key": "Demo", "entered": null, "bytes": 2304,
                    "inlined-scopes": 1, "inlined-sites": 1, "type-case-arms": 1,
                    "eliminated-blocks": 0 }),
        );
        let transition = |before: &str, after: &str, receiver: &str| {
            event(
                "caches",
                1,
                json!({ "event": "transition", "before": before, "after": after,
                        "selector": "value", "site": 0, "receiver": receiver }),
            )
        };
        scratch.record(&declined);
        scratch.record(&optimized);
        scratch.record(&transition("vacant", "monomorphic", "Demo"));
        scratch.record(&transition("monomorphic", "polymorphic", "Other"));
        scratch.index.catch_up();
        let outcome = scratch.index.send_outcome(1, 0);
        assert_eq!(outcome.inlining, Some(0));
        let cache = outcome.cache.expect("a cache");
        assert_eq!(scratch.index.text(cache.after), "polymorphic");
        assert_eq!((cache.receivers, cache.event), (2, 3));
        // The block's site is its family's: the root's transitions name it.
        assert_eq!(scratch.index.send_outcome(2, 0).cache, Some(cache));
        assert_eq!(scratch.index.body(3).unwrap().declined_as_callee.len(), 1);
        // The copy retired, its decision goes with it.
        scratch.record(&event(
            "deoptimize",
            1,
            json!({ "event": "retired", "key": "Demo", "entered": null, "reason": "exits",
                    "version": 2 }),
        ));
        scratch.index.catch_up();
        assert_eq!(scratch.index.send_outcome(1, 0).inlining, None);
        let tally = scratch.index.body(1).unwrap();
        assert_eq!(tally.copies.len(), 1);
        assert_eq!(tally.copies[0].last_bytes, 2304);
        assert_eq!(tally.deoptimizations.len(), 1);
        // A compile whose outcome never came leaves no decision.
        scratch.record(&declined);
        scratch.record(&transition("polymorphic", "megamorphic", "Third"));
        scratch.index.catch_up();
        assert_eq!(scratch.index.send_outcome(1, 0).inlining, None);
    }

    /// The follower reads the file as it grows, and ends once the index
    /// is dropped, which deletes the file.
    #[test]
    fn the_follower_reads_as_the_file_grows() {
        let mut scratch = Scratch::new("follower");
        scratch.header(&["compile"], &[]);
        let path: PathBuf = scratch.index.path().unwrap().to_path_buf();
        let Scratch {
            index,
            mut writer,
            _directory,
        } = scratch;
        let index = Arc::new(Mutex::new(index));
        let follower = spawn_follower(&index).expect("a follower");
        let line = format!("{}\n", body_record(1, "Demo>>run", None));
        writer.write_all(line.as_bytes()).unwrap();
        writer.flush().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while lock(&index).body_count() == 0 {
            assert!(std::time::Instant::now() < deadline, "never read");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(writer);
        drop(index);
        follower.join().expect("the follower ended");
        assert!(!path.exists(), "{} is still there", path.display());
    }

    #[test]
    fn a_closed_index_deletes_its_file() {
        let mut scratch = Scratch::new("closed");
        scratch.header(&[], &[]);
        scratch.index.catch_up();
        let path = scratch.index.path().unwrap().to_path_buf();
        let Scratch {
            mut index, writer, ..
        } = scratch;
        drop(writer);
        index.close();
        assert!(!path.exists());
        assert_eq!(
            index.read_record(0, 1),
            Err("the trace file is closed".to_string())
        );
        assert!(index.header().is_some());
    }
}
