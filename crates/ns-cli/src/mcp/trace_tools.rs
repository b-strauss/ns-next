//! The trace tools' answers (`docs/design/debugger.md`, "The MCP server",
//! "The trace tools"), one function each, every one rendered as text from
//! a session's `TraceIndex` and bounded by `pages.rs`: `overview`, the
//! run-wide lists an agent starts from, `journal`, one method's bodies,
//! copies, events and sends, `listing_window`, a window of one listing,
//! and `events`, the run's events filtered. Each tool runs on a thread of its
//! own and locks the index alone, never waiting on the session's worker,
//! so that the trace of a program that runs, or of a wait in progress, can
//! be asked.

use std::collections::HashMap;

use ns_runtime::{TraceCategories, TraceCategory};
use serde::Deserialize;
use serde_json::{Value, json};

use super::pages::{
    ANSWER_BUDGET, ENTRY_DEFAULT, ENTRY_MAXIMUM, LINE_DEFAULT, LINE_MAXIMUM, OVERVIEW_DEFAULT,
    call_text, characters, count_of, counted, showing_line, size, thousands,
};
use super::registry::SessionHandle;
use super::tools::{Answer, parse};
use super::trace_index::{
    BodyRecord, BodySource, CopyKey, ListingEntry, ListingKind, Placed, PlanOutcome, TraceIndex,
};
use super::trace_listings;

/// The overview's lists, in the order it writes them.
const LISTS: [&str; 5] = [
    "installs",
    "abandoned",
    "deoptimized",
    "optimized",
    "declined",
];

/// Whether the tool `name` answers from a session's trace, on a thread of
/// its own rather than the session's worker.
pub(crate) fn is_trace_tool(name: &str) -> bool {
    matches!(
        name,
        "debug_trace" | "debug_trace_method" | "debug_trace_listing" | "debug_trace_events"
    )
}

/// Calls the trace tool `name` of the session `handle`, once the index
/// read everything the file held at the call.
pub(crate) fn call(handle: &SessionHandle, name: &str, arguments: Value) -> Answer {
    let Some(trace) = handle.trace.get() else {
        return Err(format!(
            "session {} traces nothing: debug_start with trace or listings writes a trace",
            handle.number
        ));
    };
    match name {
        "debug_trace" => {
            let arguments: OverviewArguments = parse(arguments)?;
            let index = trace.caught_up();
            overview(
                &index,
                handle,
                arguments.list.as_deref(),
                arguments.start,
                arguments.count,
            )
        }
        "debug_trace_method" => {
            let arguments: JournalArguments = parse(arguments)?;
            let part = arguments
                .part
                .as_deref()
                .map(|name| {
                    Part::from_name(name)
                        .ok_or_else(|| format!("{name:?} is no part: events or sends"))
                })
                .transpose()?;
            let index = trace.caught_up();
            journal(
                &index,
                handle,
                &arguments.method,
                part,
                arguments.start,
                arguments.count,
            )
        }
        "debug_trace_listing" => {
            let arguments: ListingArguments = parse(arguments)?;
            let kind = ListingKind::from_name(&arguments.kind).ok_or_else(|| {
                format!(
                    "{:?} is no kind of listing: bytecode, code or inlining",
                    arguments.kind
                )
            })?;
            let index = trace.caught_up();
            listing_window(
                &index,
                handle,
                &arguments.method,
                kind,
                arguments.index,
                arguments.start,
                arguments.count,
            )
        }
        "debug_trace_events" => {
            let arguments: EventArguments = parse(arguments)?;
            let index = trace.caught_up();
            let query = EventQuery::new(
                &index,
                arguments.category.as_deref(),
                arguments.event.as_deref(),
                arguments.reason.as_deref(),
                arguments.method.as_deref(),
            )?;
            events(&index, handle, &query, arguments.start, arguments.count)
        }
        _ => Err(format!("no tool is named {name}")),
    }
}

#[derive(Deserialize)]
struct EventArguments {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    count: Option<usize>,
}

#[derive(Deserialize)]
struct ListingArguments {
    method: String,
    kind: String,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    count: Option<usize>,
}

#[derive(Deserialize)]
struct JournalArguments {
    method: String,
    #[serde(default)]
    part: Option<String>,
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    count: Option<usize>,
}

#[derive(Deserialize)]
struct OverviewArguments {
    #[serde(default)]
    list: Option<String>,
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    count: Option<usize>,
}

/// `<size>, <n> events, <n> bodies, <n> listings`, and the failure that
/// ended the reading early, if one did: what a summary's `trace:` line and
/// the overview's first line say of the file.
fn trace_counts(index: &TraceIndex) -> String {
    let mut text = format!(
        "{}, {}, {}, {}",
        size(index.bytes()),
        counted(index.event_count() as u64, "event", "events"),
        counted(index.body_count() as u64, "body", "bodies"),
        counted(index.listing_count() as u64, "listing", "listings"),
    );
    if let Some(failure) = index.failure() {
        text.push_str("; ");
        text.push_str(failure);
    }
    text
}

/// The `trace:` line of every summary of a traced session, naming the
/// call that answers the overview.
pub(crate) fn trace_line(index: &TraceIndex, session_number: u32) -> String {
    let mut line = format!(
        "trace: {}, {}, {}, {} — {} for an overview",
        size(index.bytes()),
        counted(index.event_count() as u64, "event", "events"),
        counted(index.body_count() as u64, "body", "bodies"),
        counted(index.listing_count() as u64, "listing", "listings"),
        call_text("debug_trace", &json!({ "session": session_number })),
    );
    if let Some(failure) = index.failure() {
        line.push_str("; ");
        line.push_str(failure);
    }
    line
}

/// One of the overview's lists, as the index makes it.
struct List {
    name: &'static str,
    /// The categories whose absence leaves it empty, when the header says
    /// they were not traced.
    not_traced: Option<&'static str>,
    /// Its entries, ranked, each one line.
    entries: Vec<String>,
    /// A line that closes it, whatever its page.
    closing: Option<String>,
}

/// `debug_trace`: the run-wide overview of the session `session`'s trace.
/// Without `list`, the head and every list's first `count` entries -- 10
/// by default, 200 at most -- each cut list's head naming the call that
/// goes on; with `list`, that list alone from `start`, ending in its
/// `Showing` line. `start` and `count` are as the call gave them, so that
/// a `next:` repeats every argument given.
pub(crate) fn overview(
    index: &TraceIndex,
    session: &SessionHandle,
    list: Option<&str>,
    start: Option<usize>,
    count: Option<usize>,
) -> Answer {
    let shown = count_of(count, OVERVIEW_DEFAULT, ENTRY_MAXIMUM)?;
    if let Some(name) = list
        && !LISTS.contains(&name)
    {
        return Err(format!(
            "{name:?} is no list: installs, abandoned, deoptimized, optimized or declined"
        ));
    }
    if list.is_none() && start.is_some() {
        return Err("start pages one list: pass list as well".to_string());
    }
    let first_line = format!("{}: {}", session.description(), trace_counts(index));
    let lists = lists(index, session.number);
    let Some(name) = list else {
        return Ok(every_list(
            index,
            session.number,
            first_line,
            &lists,
            count,
            shown,
        ));
    };
    let chosen = lists
        .iter()
        .find(|candidate| candidate.name == name)
        .expect("every list is made");
    one_list(
        session.number,
        first_line,
        chosen,
        start.unwrap_or(0),
        count,
        shown,
    )
}

/// The overview without `list`: the head, then each list's first entries.
fn every_list(
    index: &TraceIndex,
    session_number: u32,
    first_line: String,
    lists: &[List],
    count: Option<usize>,
    shown: usize,
) -> String {
    let mut lines = vec![first_line, traced_line(index)];
    let traced: Vec<String> = TraceCategory::ALL
        .iter()
        .filter(|category| index.traces(**category))
        .map(|category| {
            format!(
                "{} {}",
                category.name(),
                thousands(index.category_count(*category))
            )
        })
        .collect();
    if !traced.is_empty() {
        lines.push(format!("events: {}", traced.join(" · ")));
    }
    let mut used: usize = lines.iter().map(|line| characters(line) + 1).sum();
    // What each list writes besides its entries, at the most -- its head
    // or the line standing for it, and its closing line -- is kept back
    // while any earlier list's entries are taken, so that no line written
    // later passes the budget.
    let fixed: Vec<usize> = lists
        .iter()
        .map(|list| fixed_characters(list, session_number, count))
        .collect();
    for (position, list) in lists.iter().enumerate() {
        let reserve: usize = fixed[position..].iter().sum();
        let Some(entries) = written_entries(list, &mut lines) else {
            used = lines.iter().map(|line| characters(line) + 1).sum();
            continue;
        };
        let mut taken = Vec::new();
        for entry in entries.iter().take(shown) {
            let length = characters(entry) + 1;
            if used + length + reserve > ANSWER_BUDGET {
                break;
            }
            used += length;
            taken.push(entry.as_str());
        }
        let head = list_head(list.name, taken.len(), entries.len(), session_number, count);
        used += characters(&head) + 1;
        lines.push(head);
        lines.extend(taken.iter().map(|entry| entry.to_string()));
        if let Some(closing) = &list.closing {
            used += characters(closing) + 1;
            lines.push(closing.clone());
        }
    }
    lines.join("\n")
}

/// A list's head in the overview: `name (k of N)`, and where it is cut,
/// ` — next: ` and the call that pages on from `taken`.
fn list_head(
    name: &str,
    taken: usize,
    total: usize,
    session_number: u32,
    count: Option<usize>,
) -> String {
    let mut head = format!(
        "{name} ({} of {})",
        thousands(taken as u64),
        thousands(total as u64)
    );
    if taken < total {
        let mut arguments = json!({
            "session": session_number,
            "list": name,
            "start": taken,
        });
        if let Some(count) = count {
            arguments["count"] = json!(count);
        }
        head.push_str(" — next: ");
        head.push_str(&call_text("debug_trace", &arguments));
    }
    head
}

/// The characters `list` writes in the overview besides its entries, at
/// the most: the line that stands for it, or its head as long as a head
/// of its count can be -- one with a `next:`, no number in it of more
/// digits than its total --, and its closing line.
fn fixed_characters(list: &List, session_number: u32, count: Option<usize>) -> usize {
    let mut lines = Vec::new();
    if written_entries(list, &mut lines).is_some() {
        let total = list.entries.len();
        let head = list_head(list.name, 0, total, session_number, count);
        let longest = characters(&head) + 2 * total.to_string().len();
        return longest + 1 + list.closing.as_ref().map_or(0, |line| characters(line) + 1);
    }
    lines.iter().map(|line| characters(line) + 1).sum()
}

/// Writes the line of `list` that stands for its entries when it has
/// none to page -- not traced, or none, with its closing line -- and
/// answers `None`; answers its entries otherwise.
fn written_entries<'list>(list: &'list List, lines: &mut Vec<String>) -> Option<&'list [String]> {
    if let Some(categories) = list.not_traced {
        lines.push(format!("{}: not traced ({categories})", list.name));
        return None;
    }
    if list.entries.is_empty() {
        lines.push(format!("{}: none", list.name));
        lines.extend(list.closing.clone());
        return None;
    }
    Some(&list.entries)
}

/// The overview with `list`: that list alone from `start`, ending in its
/// `Showing` line.
fn one_list(
    session_number: u32,
    first_line: String,
    list: &List,
    start: usize,
    count: Option<usize>,
    shown: usize,
) -> Answer {
    let mut lines = vec![first_line];
    let Some(entries) = written_entries(list, &mut lines) else {
        return Ok(lines.join("\n"));
    };
    let total = entries.len();
    if start >= total {
        return Err(format!(
            "start {start} is past the end: {} has {}",
            list.name,
            counted(total as u64, "entry", "entries")
        ));
    }
    lines.push(list.name.to_string());
    let next_from = |last: usize| {
        let mut arguments = json!({ "session": session_number, "list": list.name, "start": last });
        if let Some(count) = count {
            arguments["count"] = json!(count);
        }
        ("debug_trace", arguments)
    };
    // The closing line and the longest `Showing` line this page can end
    // in are kept back while its entries are taken.
    let reserve = list.closing.as_ref().map_or(0, |line| characters(line) + 1)
        + characters(&showing_line(total, total, total, Some(next_from(total))))
        + 1;
    let mut used: usize = lines.iter().map(|line| characters(line) + 1).sum();
    let mut taken = 0;
    for entry in entries.iter().skip(start).take(shown) {
        let length = characters(entry) + 1;
        if used + length + reserve > ANSWER_BUDGET {
            break;
        }
        used += length;
        taken += 1;
        lines.push(entry.clone());
    }
    lines.extend(list.closing.clone());
    let last = start + taken;
    let next = (last < total).then(|| next_from(last));
    lines.push(showing_line(start + 1, last, total, next));
    Ok(lines.join("\n"))
}

/// `traced: <categories>; listed: <kinds>`, as the header says them.
fn traced_line(index: &TraceIndex) -> String {
    if index.header().is_none() {
        return "traced: the trace file has no header yet".to_string();
    }
    let traced: Vec<&str> = TraceCategory::ALL
        .iter()
        .filter(|category| index.traces(**category))
        .map(|category| category.name())
        .collect();
    let listed: Vec<&str> = [
        ListingKind::Bytecode,
        ListingKind::Code,
        ListingKind::Inlining,
    ]
    .into_iter()
    .filter(|kind| index.lists(*kind))
    .map(ListingKind::name)
    .collect();
    let or_nothing = |names: Vec<&str>| {
        if names.is_empty() {
            "nothing".to_string()
        } else {
            names.join(", ")
        }
    };
    format!(
        "traced: {}; listed: {}",
        or_nothing(traced),
        or_nothing(listed)
    )
}

/// The body `number` as an entry names it: its name and number, or,
/// where the file never described it, its number and that it is not
/// described -- a body the trace numbers without a `body` record, which
/// its number still addresses.
fn named(index: &TraceIndex, number: u32) -> String {
    match index.body_name(number) {
        Some(name) => format!("{name} #{number}"),
        None => format!("#{number} (not described)"),
    }
}

/// The call that answers the body `number`'s journal.
fn method_call(session_number: u32, number: u32) -> String {
    call_text(
        "debug_trace_method",
        &json!({ "session": session_number, "method": format!("#{number}") }),
    )
}

/// `counts`, the most first, those of one count in the order they came.
fn ranked<T: Copy>(counts: &[(T, u32)]) -> Vec<(T, u32)> {
    let mut ranked = counts.to_vec();
    ranked.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    ranked
}

/// `<text> <count>, …` of `counts`, ranked, each key written by `text`.
fn breakdown<T: Copy>(counts: &[(T, u32)], text: impl Fn(T) -> String) -> String {
    ranked(counts)
        .into_iter()
        .map(|(key, count)| format!("{} {}", text(key), thousands(u64::from(count))))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A copy's key as the trace's lines write it: `key={Bounce}`, with its
/// `entered={…}` where it has one.
fn key_text(index: &TraceIndex, key: CopyKey) -> String {
    let parts: Vec<String> = [("key", key.key), ("entered", key.entered)]
        .into_iter()
        .filter_map(|(field, value)| {
            value.map(|value| format!("{field}={{{}}}", index.text(value)))
        })
        .collect();
    if parts.is_empty() {
        "no key".to_string()
    } else {
        parts.join(" ")
    }
}

/// Entries ranked by their first number, ties by body number.
fn sorted(mut entries: Vec<(u64, u32, String)>) -> Vec<String> {
    entries.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    entries.into_iter().map(|(_, _, text)| text).collect()
}

/// The overview's five lists, in their order.
fn lists(index: &TraceIndex, session_number: u32) -> Vec<List> {
    let compile = index.traces(TraceCategory::Compile);
    let optimize = index.traces(TraceCategory::Optimize);
    let mut installs = Vec::new();
    let mut abandoned = Vec::new();
    let mut deoptimized = Vec::new();
    let mut optimized = Vec::new();
    let mut declined = Vec::new();
    for (number, tally) in index.bodies() {
        let name = named(index, number);
        let installed = index.installs(number);
        if installed.total() > 0 {
            let tiers: Vec<String> = [
                ("baseline", installed.baseline),
                ("twin", installed.twin),
                ("optimized", installed.optimized),
            ]
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(|(tier, count)| format!("{tier} {}", thousands(u64::from(count))))
            .collect();
            installs.push((
                u64::from(installed.total()),
                number,
                format!(
                    "  {name}: {} ({}) → {}",
                    thousands(u64::from(installed.total())),
                    tiers.join(", "),
                    method_call(session_number, number)
                ),
            ));
        }
        if tally.abandoned > 0 {
            let mut text = format!(
                "  {name}: {} ({})",
                thousands(u64::from(tally.abandoned)),
                counted(tally.exits, "exit", "exits")
            );
            if !tally.revived.is_empty() {
                let revived: u32 = tally.revived.iter().map(|(_, count)| count).sum();
                text.push_str(&format!(
                    ", revived {} ({})",
                    thousands(u64::from(revived)),
                    breakdown(&tally.revived, |revival| index.text(revival).to_string())
                ));
            }
            text.push_str(&format!(" → {}", method_call(session_number, number)));
            abandoned.push((u64::from(tally.abandoned), number, text));
        }
        if !tally.deoptimizations.is_empty() {
            let total: u32 = tally.deoptimizations.iter().map(|(_, count)| count).sum();
            let reasons = breakdown(&tally.deoptimizations, |(event, reason)| match reason {
                Some(reason) => format!("{} {}", index.text(event), index.text(reason)),
                None => index.text(event).to_string(),
            });
            deoptimized.push((
                u64::from(total),
                number,
                format!(
                    "  {name}: {} ({reasons}) → {}",
                    thousands(u64::from(total)),
                    method_call(session_number, number)
                ),
            ));
        }
        if !tally.copies.is_empty() {
            let total: u32 = tally.copies.iter().map(|copies| copies.count).sum();
            let keys: Vec<(CopyKey, u32)> = tally
                .copies
                .iter()
                .map(|copies| (copies.key, copies.count))
                .collect();
            let next = if index.lists(ListingKind::Inlining) && !tally.inlining.is_empty() {
                call_text(
                    "debug_trace_listing",
                    &json!({
                        "session": session_number,
                        "method": format!("#{number}"),
                        "kind": "inlining",
                    }),
                )
            } else {
                method_call(session_number, number)
            };
            optimized.push((
                u64::from(total),
                number,
                format!(
                    "  {name}: {} ({}), last {} → {next}",
                    counted(u64::from(total), "copy", "copies"),
                    breakdown(&keys, |key| key_text(index, key)),
                    counted(tally.last_copy_bytes, "byte", "bytes"),
                ),
            ));
        }
        if !tally.declined_as_callee.is_empty() {
            let total: u32 = tally
                .declined_as_callee
                .iter()
                .map(|(_, count)| count)
                .sum();
            declined.push((
                u64::from(total),
                number,
                format!(
                    "  {name}: {} ({}){} → {}",
                    thousands(u64::from(total)),
                    breakdown(&tally.declined_as_callee, |reason| index
                        .text(reason)
                        .to_string()),
                    declined_in(index, &tally.declined_in),
                    call_text(
                        "debug_trace_events",
                        &json!({
                            "session": session_number,
                            "event": "declined",
                            "method": format!("#{number}"),
                        }),
                    )
                ),
            ));
        }
    }
    let no_body = index.no_body_declines();
    let closing = (!no_body.is_empty()).then(|| {
        let total: u32 = no_body.iter().map(|(_, count)| count).sum();
        format!(
            "  callees that are no body: {} ({})",
            thousands(u64::from(total)),
            breakdown(no_body, |reason| index.text(reason).to_string())
        )
    });
    let not_traced = |traced: bool, categories: &'static str| (!traced).then_some(categories);
    vec![
        List {
            name: "installs",
            not_traced: not_traced(
                compile || optimize || index.lists(ListingKind::Code),
                "compile, optimize",
            ),
            entries: sorted(installs),
            closing: None,
        },
        List {
            name: "abandoned",
            not_traced: not_traced(compile, "compile"),
            entries: sorted(abandoned),
            closing: None,
        },
        List {
            name: "deoptimized",
            not_traced: not_traced(index.traces(TraceCategory::Deoptimize), "deoptimize"),
            entries: sorted(deoptimized),
            closing: None,
        },
        List {
            name: "optimized",
            not_traced: not_traced(optimize, "optimize"),
            entries: sorted(optimized),
            closing: None,
        },
        List {
            name: "declined",
            not_traced: not_traced(index.traces(TraceCategory::Inlining), "inlining"),
            entries: sorted(declined),
            closing,
        },
    ]
}

/// The most compiling bodies a `declined` entry names.
const DECLINED_IN_SHOWN: usize = 3;

/// ` in <body>` for a callee declined in one body's compiles, or
/// ` in <body> (<count>), …` for several, the most first, at most
/// `DECLINED_IN_SHOWN` of them.
fn declined_in(index: &TraceIndex, bodies: &[(u32, u32)]) -> String {
    let mut ranked = bodies.to_vec();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    match ranked.as_slice() {
        [] => String::new(),
        [(body, _)] => format!(" in {}", named(index, *body)),
        _ => {
            let mut shown: Vec<String> = ranked
                .iter()
                .take(DECLINED_IN_SHOWN)
                .map(|(body, count)| {
                    format!("{} ({})", named(index, *body), thousands(u64::from(*count)))
                })
                .collect();
            if ranked.len() > DECLINED_IN_SHOWN {
                shown.push(format!("… {} more", ranked.len() - DECLINED_IN_SHOWN));
            }
            format!(" in {}", shown.join(", "))
        }
    }
}

/// A part of a journal, paged alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Events,
    Sends,
}

impl Part {
    pub(crate) fn from_name(name: &str) -> Option<Part> {
        match name {
            "events" => Some(Part::Events),
            "sends" => Some(Part::Sends),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Part::Events => "events",
            Part::Sends => "sends",
        }
    }
}

/// A text and how many times it came in a row: one step of a folded
/// cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Repeat {
    pub(crate) text: u32,
    pub(crate) count: usize,
}

/// One entry of a folded sequence: from the text at `first`, the steps of
/// `cycle`, the whole cycle repeated `repetitions` times in a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Folded {
    pub(crate) first: usize,
    pub(crate) cycle: Vec<Repeat>,
    pub(crate) repetitions: usize,
}

/// The longest cycle a fold writes once, in steps: a body that keeps
/// being compiled, twinned, abandoned, freed and revived writes six events
/// a round under every category, five steps once the freeing of both its
/// copies is one.
const LONGEST_CYCLE: usize = 8;

/// `texts` folded greedily in two passes. First every run of one text is
/// one step, its count beside it; then at each step the shortest cycle of
/// one to `LONGEST_CYCLE` steps that repeats at least twice in a row there
/// is one entry, as many repetitions as follow, and a step that starts no
/// such cycle is an entry of its own. A run of thousands of rounds of one
/// body's abandonment and revival reads as one entry, its count beside it.
/// The journal folds its events' shapes -- category and event word --
/// rather than their whole texts, so that a round whose fields differ, a
/// revival by decay among revivals by entries or a compile of other
/// bytes, stays in its run; the fields that differ are summarized beside
/// the entry (`folded_text`). That departs, for the journal alone, from
/// comparing whole entry texts.
pub(crate) fn fold(texts: &[u32]) -> Vec<Folded> {
    let mut steps: Vec<(usize, Repeat)> = Vec::new();
    for (position, &text) in texts.iter().enumerate() {
        match steps.last_mut() {
            Some((_, step)) if step.text == text => step.count += 1,
            _ => steps.push((position, Repeat { text, count: 1 })),
        }
    }
    let repeats: Vec<Repeat> = steps.iter().map(|(_, step)| *step).collect();
    let mut folded = Vec::new();
    let mut position = 0;
    while position < repeats.len() {
        let repeated = (1..=LONGEST_CYCLE).find_map(|cycle| {
            let pattern = repeats.get(position..position + cycle)?;
            let mut repetitions = 1;
            while repeats.get(position + repetitions * cycle..position + (repetitions + 1) * cycle)
                == Some(pattern)
            {
                repetitions += 1;
            }
            (repetitions >= 2).then_some((cycle, repetitions))
        });
        let (cycle, repetitions) = repeated.unwrap_or((1, 1));
        folded.push(Folded {
            first: steps[position].0,
            cycle: repeats[position..position + cycle].to_vec(),
            repetitions,
        });
        position += cycle * repetitions;
    }
    folded
}

/// The most characters a journal's head -- its bodies, copies and
/// inlining lines -- takes of the budget, so that a name matching many
/// bodies still leaves room for the events and the sends.
const HEAD_BUDGET: usize = ANSWER_BUDGET / 2;

/// `debug_trace_method`: the journal of every body `method` names -- `#<n>`
/// or a name in the tools' grammar, a method's root and its blocks, every
/// version's and every edited body's (`TraceIndex::bodies_named`).
/// Without `part`, its head, then the first page of its events, folded,
/// and the first page of its sends; with `part`, that part alone from
/// `start`. Each cut part ends in the call that goes on, which repeats
/// every argument given, `start` advanced.
pub(crate) fn journal(
    index: &TraceIndex,
    session: &SessionHandle,
    method: &str,
    part: Option<Part>,
    start: Option<usize>,
    count: Option<usize>,
) -> Answer {
    let shown = count_of(count, ENTRY_DEFAULT, ENTRY_MAXIMUM)?;
    if part.is_none() && start.is_some() {
        return Err("start pages one part: pass part as well".to_string());
    }
    let bodies = index.bodies_named(method)?;
    let (events, groups, folded_events) = journal_events(index, &bodies);
    let sends: Vec<Entry> = journal_sends(index, &bodies)
        .into_iter()
        .map(|text| Entry { group: None, text })
        .collect();
    let next_from = |part: Part, last: usize| {
        let mut arguments = json!({
            "session": session.number,
            "method": method,
            "part": part.name(),
            "start": last,
        });
        if let Some(count) = count {
            arguments["count"] = json!(count);
        }
        ("debug_trace_method", arguments)
    };
    let events_head = |taken: Option<usize>| {
        let entries = match taken {
            Some(taken) => format!(
                "{} of {}",
                thousands(taken as u64),
                counted(events.len() as u64, "entry", "entries")
            ),
            None => counted(events.len() as u64, "entry", "entries"),
        };
        format!(
            "events ({entries}, folding {}; every event, caches included: {})",
            counted(folded_events as u64, "lifecycle event", "lifecycle events"),
            call_text(
                "debug_trace_events",
                &json!({ "session": session.number, "method": method })
            )
        )
    };
    let sends_head = |taken: Option<usize>| match taken {
        Some(taken) => format!(
            "sends ({} of {})",
            thousands(taken as u64),
            thousands(sends.len() as u64)
        ),
        None => format!(
            "sends ({})",
            counted(sends.len() as u64, "entry", "entries")
        ),
    };
    let Some(part) = part else {
        let mut lines = journal_head(index, session, method, &bodies);
        let mut used: usize = lines.iter().map(|line| characters(line) + 1).sum();
        // What the sends write besides their entries is kept back while the
        // events' entries are taken.
        let sends_fixed = part_fixed(&sends_head(Some(sends.len())), sends.len(), &|last| {
            next_from(Part::Sends, last)
        });
        let events_page = Page {
            head: &|taken| events_head(Some(taken)),
            entries: &events,
            groups: &groups,
            next: &|last| next_from(Part::Events, last),
            name: Part::Events.name(),
        };
        // The events take at most half of what is left, so that the sends
        // have a page too, and what the sends do not need.
        let sends_wanted: usize = sends
            .iter()
            .take(shown)
            .map(|entry| characters(&entry.text) + 1)
            .sum();
        let left = ANSWER_BUDGET.saturating_sub(used + sends_fixed);
        let reserve = sends_fixed + sends_wanted.min(left / 2);
        events_page.write(&mut lines, &mut used, 0, shown, reserve, false);
        let sends_page = Page {
            head: &|taken| sends_head(Some(taken)),
            entries: &sends,
            groups: &[],
            next: &|last| next_from(Part::Sends, last),
            name: Part::Sends.name(),
        };
        sends_page.write(&mut lines, &mut used, 0, shown, 0, false);
        return Ok(lines.join("\n"));
    };
    let start = start.unwrap_or(0);
    let (entries, head, page_groups): (&[Entry], String, &[String]) = match part {
        Part::Events => (&events, events_head(None), &groups),
        Part::Sends => (&sends, sends_head(None), &[]),
    };
    // A part paged alone says whose it is, and whether the reading ended
    // early, as the whole journal's head does.
    let first = first_line(index, session, method, &bodies);
    if entries.is_empty() && start == 0 {
        return Ok(format!("{first}\n{}: none", part.name()));
    }
    if start >= entries.len() {
        return Err(format!(
            "start {start} is past the end: {} has {}",
            part.name(),
            counted(entries.len() as u64, "entry", "entries")
        ));
    }
    let mut used = characters(&first) + 1;
    let mut lines = vec![first];
    let page = Page {
        head: &|_| head.clone(),
        entries,
        groups: page_groups,
        next: &|last| next_from(part, last),
        name: part.name(),
    };
    page.write(&mut lines, &mut used, start, shown, 0, true);
    Ok(lines.join("\n"))
}

/// One entry of a journal's part, and the group -- a body's events --
/// whose head line stands above it.
struct Entry {
    group: Option<usize>,
    text: String,
}

/// A part of a journal as it is paged: its head by the entries taken,
/// its entries, the head lines of their groups, and the call that goes on
/// from an entry.
struct Page<'page> {
    head: &'page dyn Fn(usize) -> String,
    entries: &'page [Entry],
    groups: &'page [String],
    next: &'page dyn Fn(usize) -> (&'static str, Value),
    name: &'static str,
}

impl Page<'_> {
    /// Writes the page from `start`: its head, at most `shown` entries
    /// while the answer stays within the budget with `reserve` characters
    /// kept back for what is written after it -- each group's head line
    /// above its first entry on the page, counted with it -- and its
    /// `Showing` line, where it is cut, or always where `showing` asks it.
    fn write(
        &self,
        lines: &mut Vec<String>,
        used: &mut usize,
        start: usize,
        shown: usize,
        reserve: usize,
        showing: bool,
    ) {
        let total = self.entries.len();
        if total == 0 {
            let none = format!("{}: none", self.name);
            *used += characters(&none) + 1;
            lines.push(none);
            return;
        }
        let fixed = part_fixed(&(self.head)(total), total, self.next);
        let mut taken = Vec::new();
        let mut entries_taken = 0;
        let mut group = None;
        for entry in self.entries.iter().skip(start).take(shown) {
            let group_head = (entry.group != group)
                .then(|| entry.group.map(|number| self.groups[number].clone()))
                .flatten();
            let length = characters(&entry.text)
                + 1
                + group_head.as_ref().map_or(0, |head| characters(head) + 1);
            if *used + length + fixed + reserve > ANSWER_BUDGET {
                break;
            }
            *used += length;
            group = entry.group;
            taken.extend(group_head);
            taken.push(entry.text.clone());
            entries_taken += 1;
        }
        let head = (self.head)(entries_taken);
        *used += characters(&head) + 1;
        lines.push(head);
        lines.extend(taken);
        let last = start + entries_taken;
        if showing || last < total {
            let next = (last < total).then(|| (self.next)(last));
            let line = showing_line(start + 1, last, total, next);
            *used += characters(&line) + 1;
            lines.push(line);
        }
    }
}

/// The characters a part writes besides its entries, at the most: its
/// head as long as `head`, whose numbers are the total's, and the
/// longest `Showing` line it can end in.
fn part_fixed(head: &str, total: usize, next: &dyn Fn(usize) -> (&'static str, Value)) -> usize {
    characters(head) + 1 + characters(&showing_line(total, total, total, Some(next(total)))) + 1
}

/// ` (edited, line <n> of its current text)`: where a body an edit
/// compiled stands, in the module's current text, which no file holds --
/// the file's line `<n>` may be another member's or none.
fn edited_line(range: [u32; 4]) -> String {
    format!(" (edited, line {} of its current text)", range[0] + 1)
}

/// The journal's first line: the method, its module and place, the
/// session, and the failure that ended the reading early, if one did; for
/// a name of several methods, the name and how many.
fn first_line(index: &TraceIndex, session: &SessionHandle, method: &str, bodies: &[u32]) -> String {
    let description = |number: u32| index.body(number)?.description.as_ref();
    let mut roots: Vec<&str> = bodies
        .iter()
        .filter_map(|number| description(*number))
        .filter(|described| described.family.is_none())
        .map(|described| described.name.as_str())
        .collect();
    roots.sort_unstable();
    roots.dedup();
    let lead = bodies
        .iter()
        .find_map(|number| description(*number).filter(|described| described.family.is_none()))
        .or_else(|| bodies.iter().find_map(|number| description(*number)));
    let mut first = match (roots.len() > 1, lead) {
        (true, _) => format!(
            "{method} — {} methods — session {}",
            roots.len(),
            session.number
        ),
        (false, Some(described)) => {
            let mut place = match &described.source {
                BodySource::File(path) => path.clone(),
                BodySource::Library(path) => format!("core library {path}"),
                BodySource::Nowhere => "no file".to_string(),
            };
            match (described.range, described.placed) {
                (Some(range), Placed::Source) => place.push_str(&format!(":{}", range[0] + 1)),
                (Some(range), Placed::Current) => place.push_str(&edited_line(range)),
                _ => {}
            }
            format!(
                "{} — {}, {place} — session {}",
                described.name,
                module_text(&described.module),
                session.number
            )
        }
        (false, None) => format!(
            "#{} (not described) — session {}",
            bodies[0], session.number
        ),
    };
    if let Some(failure) = index.failure() {
        first.push_str("; ");
        first.push_str(failure);
    }
    first
}

/// The journal's head: the method, its module and place; its bodies;
/// each body's copies; and each body's inlining as a callee.
fn journal_head(
    index: &TraceIndex,
    session: &SessionHandle,
    method: &str,
    bodies: &[u32],
) -> Vec<String> {
    let first = first_line(index, session, method, bodies);
    let description = |number: u32| index.body(number)?.description.as_ref();
    let roots: Vec<&str> = {
        let mut names: Vec<&str> = bodies
            .iter()
            .filter_map(|number| description(*number))
            .filter(|described| described.family.is_none())
            .map(|described| described.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    };
    let lead = bodies
        .iter()
        .copied()
        .find(|number| description(*number).is_some_and(|described| described.family.is_none()))
        .or_else(|| {
            bodies
                .iter()
                .copied()
                .find(|number| description(*number).is_some())
        });
    let lead_description = lead.and_then(description);
    let mut lines = vec![first];
    let mut used = characters(&lines[0]) + 1;
    let items: Vec<String> = bodies
        .iter()
        .map(|number| match description(*number) {
            None => format!("#{number} (not described)"),
            Some(described) => {
                let lead_name = lead_description.map(|lead| lead.name.as_str());
                // Among several methods, every root is named.
                let kind = if roots.len() == 1
                    && described.family.is_none()
                    && Some(described.name.as_str()) == lead_name
                    && described.name.contains(">>")
                {
                    "method".to_string()
                } else {
                    described.name.clone()
                };
                let mut item = format!("#{number} {kind}");
                if let Some(lead) = lead_description
                    && lead.module != described.module
                {
                    item.push_str(&format!(" of {}", module_text(&described.module)));
                }
                match (described.range, described.placed) {
                    (Some(range), Placed::Source) => {
                        item.push_str(&format!(" (line {})", range[0] + 1));
                    }
                    (Some(range), Placed::Current) => item.push_str(&edited_line(range)),
                    _ => {}
                }
                item
            }
        })
        .collect();
    let bodies_line = joined_within("bodies: ", &items, HEAD_BUDGET.saturating_sub(used));
    used += characters(&bodies_line) + 1;
    lines.push(bodies_line);
    let mut more: Vec<String> = Vec::new();
    for number in bodies {
        more.extend(copies_line(index, *number));
    }
    for number in bodies {
        more.extend(inlined_line(index, *number));
    }
    let mut left = more.len();
    for line in more {
        let length = characters(&line) + 1;
        if used + length > HEAD_BUDGET {
            break;
        }
        used += length;
        left -= 1;
        lines.push(line);
    }
    if left > 0 {
        lines.push(format!(
            "… {} more of the head: a narrower name, or #<n>, tells them",
            counted(left as u64, "line", "lines")
        ));
    }
    lines
}

/// `prefix` and `items`, joined by `, ` while they stay within `limit`
/// characters, and how many were left out.
fn joined_within(prefix: &str, items: &[String], limit: usize) -> String {
    let mut line = prefix.to_string();
    for (position, item) in items.iter().enumerate() {
        let separator = if position == 0 { "" } else { ", " };
        let later = items.len() - position - 1;
        // Room for the item and, where more follow, the count of those a
        // later item that does not fit would leave out.
        let tail = if later > 0 {
            characters(&format!(", … {} more", thousands(later as u64)))
        } else {
            0
        };
        if position > 0
            && characters(&line) + characters(separator) + characters(item) + tail > limit
        {
            line.push_str(&format!(
                ", … {} more",
                thousands((items.len() - position) as u64)
            ));
            return line;
        }
        line.push_str(separator);
        line.push_str(item);
    }
    line
}

/// A module as the head writes it: its id, and `@` its version where it
/// has one.
fn module_text(module: &ns_runtime::ModuleName) -> String {
    match &module.version {
        Some(version) => format!("{}@{version}", module.id),
        None => module.id.clone(),
    }
}

/// `1–3, 7`: the 1-based `positions`, ascending, a run of consecutive ones
/// written as its first and last.
fn ranges(positions: &[usize]) -> String {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &position in positions {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == position => *last = position,
            _ => runs.push((position, position)),
        }
    }
    runs.into_iter()
        .map(|(first, last)| {
            if first == last {
                thousands(first as u64)
            } else {
                format!("{}–{}", thousands(first as u64), thousands(last as u64))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `copies of #<n>: …`, the body's machine code by tier -- for optimized
/// code by key -- each with its installs and the 1-based indexes of its
/// `code` and `inlining` records among the body's own, which
/// `debug_trace_listing` takes; `None` for a body without machine code.
fn copies_line(index: &TraceIndex, number: u32) -> Option<String> {
    let tally = index.body(number)?;
    let installs = index.installs(number);
    let listings = index.listings();
    let mut baseline = Vec::new();
    let mut twin = Vec::new();
    let mut optimized: Vec<(CopyKey, Vec<usize>)> = Vec::new();
    let key_of = |key: Option<u32>, entered: Option<u32>| CopyKey { key, entered };
    for (position, listing) in tally.code.iter().enumerate() {
        let entry = &listings[*listing as usize];
        let position = position + 1;
        match entry.tier.map(|tier| index.text(tier)) {
            Some("optimized") => {
                let key = key_of(entry.key, entry.entered);
                match optimized.iter_mut().find(|(known, _)| *known == key) {
                    Some((_, positions)) => positions.push(position),
                    None => optimized.push((key, vec![position])),
                }
            }
            Some("twin") => twin.push(position),
            _ => baseline.push(position),
        }
    }
    let mut plans: Vec<(CopyKey, Vec<usize>)> = Vec::new();
    for (position, listing) in tally.inlining.iter().enumerate() {
        let entry = &listings[*listing as usize];
        let key = key_of(entry.key, entry.entered);
        match plans.iter_mut().find(|(known, _)| *known == key) {
            Some((_, positions)) => positions.push(position + 1),
            None => plans.push((key, vec![position + 1])),
        }
    }
    let mut keys: Vec<CopyKey> = tally.copies.iter().map(|copies| copies.key).collect();
    for key in optimized.iter().chain(&plans).map(|(key, _)| *key) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    let mut pieces = Vec::new();
    for (tier, count, positions) in [
        ("baseline", installs.baseline, &baseline),
        ("twin", installs.twin, &twin),
    ] {
        if count == 0 && positions.is_empty() {
            continue;
        }
        let mut piece = format!("{tier} {}", thousands(u64::from(count)));
        if !positions.is_empty() {
            piece.push_str(&format!(" (code {})", ranges(positions)));
        }
        pieces.push(piece);
    }
    let positions_of = |listed: &[(CopyKey, Vec<usize>)], key: CopyKey| {
        listed
            .iter()
            .find(|(known, _)| *known == key)
            .map(|(_, positions)| positions.clone())
            .unwrap_or_default()
    };
    for key in keys {
        let code = positions_of(&optimized, key);
        let inlining = positions_of(&plans, key);
        let count = if index.traces(TraceCategory::Optimize) {
            tally
                .copies
                .iter()
                .find(|copies| copies.key == key)
                .map_or(0, |copies| copies.count)
        } else {
            u32::try_from(code.len()).unwrap_or(u32::MAX)
        };
        let mut piece = format!(
            "optimized {} {}",
            key_text(index, key),
            thousands(u64::from(count))
        );
        let mut listed = Vec::new();
        if !code.is_empty() {
            listed.push(format!("code {}", ranges(&code)));
        }
        if !inlining.is_empty() {
            listed.push(format!("inlining {}", ranges(&inlining)));
        }
        if !listed.is_empty() {
            piece.push_str(&format!(" ({})", listed.join("; ")));
        }
        pieces.push(piece);
    }
    (!pieces.is_empty()).then(|| format!("copies of #{number}: {}", pieces.join(", ")))
}

/// `inlined #<n> into: …`, the bodies whose compiles inlined the body and
/// how often, by key, the most first, and its declines as a callee by
/// reason; `None` for a body no compile inlined or declined.
fn inlined_line(index: &TraceIndex, number: u32) -> Option<String> {
    let tally = index.body(number)?;
    if tally.inlined_into.is_empty() && tally.declined_as_callee.is_empty() {
        return None;
    }
    let into = if tally.inlined_into.is_empty() {
        "none".to_string()
    } else {
        ranked(&tally.inlined_into)
            .into_iter()
            .map(|((body, key), count)| {
                let key = key.map_or_else(
                    || "no key".to_string(),
                    |key| format!("key={{{}}}", index.text(key)),
                );
                format!(
                    "{} ×{} ({key})",
                    named(index, body),
                    thousands(u64::from(count))
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut line = format!("inlined #{number} into: {into}");
    if !tally.declined_as_callee.is_empty() {
        let total: u32 = tally
            .declined_as_callee
            .iter()
            .map(|(_, count)| count)
            .sum();
        line.push_str(&format!(
            "; declined ×{} ({})",
            thousands(u64::from(total)),
            breakdown(&tally.declined_as_callee, |reason| index
                .text(reason)
                .to_string())
        ));
    }
    Some(line)
}

/// The categories whose events the journal's events part holds: a body's
/// lifecycle. Its cache transitions are its sends' (the sends part), and
/// every event is `debug_trace_events`'.
const LIFECYCLE: [TraceCategory; 5] = [
    TraceCategory::Compile,
    TraceCategory::Osr,
    TraceCategory::Optimize,
    TraceCategory::Deoptimize,
    TraceCategory::CodeZone,
];

/// The most values of a field a summary names before `…`.
const VARIANTS_SHOWN: usize = 3;

/// The lifecycle events of `bodies`, each body's apart in file order and
/// folded by their shapes -- a cycle is one body's, and another's events
/// between its rounds would hide it; `debug_trace_events` gives the
/// run-wide order --, each entry one line under its body's head line; the
/// head lines; and how many events they are.
fn journal_events(index: &TraceIndex, bodies: &[u32]) -> (Vec<Entry>, Vec<String>, usize) {
    let events = index.events();
    let mut entries = Vec::new();
    let mut groups = Vec::new();
    let mut total = 0;
    for &number in bodies {
        let Some(tally) = index.body(number) else {
            continue;
        };
        let own: Vec<u32> = tally
            .events
            .iter()
            .copied()
            .filter(|event| LIFECYCLE.contains(&events[*event as usize].category()))
            .collect();
        if own.is_empty() {
            continue;
        }
        // An event's shape: its category and its line's word.
        let mut shapes: HashMap<(TraceCategory, u32), u32> = HashMap::new();
        let keys: Vec<u32> = own
            .iter()
            .map(|event| {
                let event = &events[*event as usize];
                let next = u32::try_from(shapes.len()).unwrap_or(u32::MAX);
                *shapes
                    .entry((event.category(), event.word()))
                    .or_insert(next)
            })
            .collect();
        let folded = fold(&keys);
        let name = index
            .body_name(number)
            .map_or_else(|| "(not described)".to_string(), str::to_string);
        groups.push(format!(
            "  #{number} {name} — {}, folding {}",
            counted(folded.len() as u64, "entry", "entries"),
            counted(own.len() as u64, "event", "events")
        ));
        let group = groups.len() - 1;
        total += own.len();
        entries.extend(folded.iter().map(|entry| Entry {
            group: Some(group),
            text: format!("    {}", folded_text(index, entry, &own)),
        }));
    }
    (entries, groups, total)
}

/// An event's text without its body's `#<n> `, which its group's head
/// line names.
fn without_body(text: &str) -> &str {
    match text.split_once(' ') {
        Some((body, rest)) if body.starts_with('#') => rest,
        _ => text,
    }
}

/// The fields of an event's text line, ` key={Bounce} at=3`, as names and
/// values, braces kept.
fn fields_of(fields: &str) -> Vec<(&str, &str)> {
    let mut depth = 0usize;
    let mut token_start = 0;
    let mut tokens = Vec::new();
    for (position, byte) in fields.bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b' ' if depth == 0 => {
                tokens.push(&fields[token_start..position]);
                token_start = position + 1;
            }
            _ => {}
        }
    }
    tokens.push(&fields[token_start..]);
    tokens
        .into_iter()
        .filter_map(|token| token.split_once('='))
        .collect()
}

/// What the events of one step say of each field whose value differs
/// among them, beside its word: a number's range, `bytes 6,208–6,400`;
/// otherwise each value and how often, the most first, `by=entries ×520,
/// by=decay ×34`, at most `VARIANTS_SHOWN` of them, then `…`, and how
/// many events do not carry the field, `absent ×8`.
fn varying_fields(index: &TraceIndex, word: &str, events: &[u32]) -> Vec<String> {
    let all = index.events();
    let mut names: Vec<&str> = Vec::new();
    let mut values: Vec<Vec<&str>> = Vec::new();
    for event in events {
        for (name, value) in fields_of(index.fields(&all[*event as usize])) {
            match names.iter().position(|known| *known == name) {
                Some(position) => values[position].push(value),
                None => {
                    names.push(name);
                    values.push(vec![value]);
                }
            }
        }
    }
    let mut summaries = Vec::new();
    for (name, values) in names.into_iter().zip(values) {
        if values.iter().all(|value| *value == values[0]) && values.len() == events.len() {
            continue;
        }
        let numbers: Option<Vec<u64>> = values.iter().map(|value| value.parse().ok()).collect();
        match numbers {
            Some(numbers) if numbers.len() == events.len() => {
                let lowest = numbers.iter().min().copied().unwrap_or(0);
                let highest = numbers.iter().max().copied().unwrap_or(0);
                summaries.push(format!(
                    "{word} {name} {}–{}",
                    thousands(lowest),
                    thousands(highest)
                ));
            }
            _ => {
                let mut counts: Vec<(&str, u32)> = Vec::new();
                for value in &values {
                    count_into(&mut counts, value);
                }
                let ranked = ranked(&counts);
                let mut shown: Vec<String> = ranked
                    .iter()
                    .take(VARIANTS_SHOWN)
                    .map(|(value, count)| {
                        format!("{name}={value} ×{}", thousands(u64::from(*count)))
                    })
                    .collect();
                if ranked.len() > VARIANTS_SHOWN {
                    shown.push("…".to_string());
                }
                // A field only some of the events carry counts its absence
                // too, so that the counts add up to the step's events.
                // Saturating: an event that carries a field twice gives it
                // two values, which must never make the count underflow.
                let absent = events.len().saturating_sub(values.len());
                if absent > 0 {
                    shown.push(format!("absent ×{}", thousands(absent as u64)));
                }
                summaries.push(format!("{word} {}", shown.join(", ")));
            }
        }
    }
    summaries
}

/// Counts `value` into `counts`, its first-seen order kept.
fn count_into<'value>(counts: &mut Vec<(&'value str, u32)>, value: &'value str) {
    match counts.iter_mut().find(|(known, _)| *known == value) {
        Some((_, count)) => *count += 1,
        None => counts.push((value, 1)),
    }
}

/// A folded entry as the journal writes it, from the events `own` its
/// positions count in: each step as its first event's text -- the first
/// round's fields --, with `×<count>` after it where it came several times
/// in a row; a cycle in parentheses, its steps joined by ` → `, its first
/// step's `[<category>] ` written once before it and every other step's
/// category only where it differs, `×<repetitions>` after it; then, after
/// ` — `, each field whose value differs among a step's events, by step.
fn folded_text(index: &TraceIndex, folded: &Folded, own: &[u32]) -> String {
    let all = index.events();
    // The events of each step, over every round.
    let mut steps: Vec<Vec<u32>> = vec![Vec::new(); folded.cycle.len()];
    let mut position = folded.first;
    for _ in 0..folded.repetitions {
        for (step, repeat) in folded.cycle.iter().enumerate() {
            steps[step].extend_from_slice(&own[position..position + repeat.count]);
            position += repeat.count;
        }
    }
    let text_of = |events: &[u32]| without_body(index.text(all[events[0] as usize].text()));
    let first = text_of(&steps[0]);
    let category = (folded.cycle.len() > 1)
        .then(|| first.find("] ").map(|end| &first[..end + 2]))
        .flatten();
    let written: Vec<String> = folded
        .cycle
        .iter()
        .zip(&steps)
        .map(|(repeat, events)| {
            let text = text_of(events);
            let text = category
                .and_then(|category| text.strip_prefix(category))
                .unwrap_or(text);
            if repeat.count > 1 {
                format!("{text} ×{}", thousands(repeat.count as u64))
            } else {
                text.to_string()
            }
        })
        .collect();
    let mut text = match written.as_slice() {
        [step] => step.clone(),
        _ => format!(
            "{}({}) ×{}",
            category.unwrap_or(""),
            written.join(" → "),
            thousands(folded.repetitions as u64)
        ),
    };
    let summaries: Vec<String> = steps
        .iter()
        .flat_map(|events| {
            let word = index.text(all[events[0] as usize].word());
            varying_fields(index, word, events)
        })
        .collect();
    if !summaries.is_empty() {
        text.push_str(" — ");
        text.push_str(&summaries.join("; "));
    }
    text
}

/// The value of the field `name` in an event's fields as its text line
/// writes them, ` key={Bounce} at=3`: `{Bounce}`, braces kept.
fn field_value<'fields>(fields: &'fields str, name: &str) -> Option<&'fields str> {
    let mut depth = 0usize;
    let mut token_start = 0;
    let bytes = fields.as_bytes();
    let mut tokens = Vec::new();
    for (position, byte) in bytes.iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b' ' if depth == 0 => {
                tokens.push(&fields[token_start..position]);
                token_start = position + 1;
            }
            _ => {}
        }
    }
    tokens.push(&fields[token_start..]);
    tokens
        .into_iter()
        .find_map(|token| token.strip_prefix(name)?.strip_prefix('='))
}

/// An event's key as the trace's lines write it, from its fields:
/// `key={Bounce}`, with its `entered={…}` where it has one.
fn event_key(fields: &str) -> String {
    let parts: Vec<String> = ["key", "entered"]
        .into_iter()
        .filter_map(|name| field_value(fields, name).map(|value| format!("{name}={value}")))
        .collect();
    if parts.is_empty() {
        "no key".to_string()
    } else {
        parts.join(" ")
    }
}

/// The sends of `bodies`, each body's in bytecode order, each one line:
/// the body, the call's 1-based line and column, the selector the trace
/// file names, and what the trace says became of it -- the latest
/// inlining and the cache's state.
fn journal_sends(index: &TraceIndex, bodies: &[u32]) -> Vec<String> {
    let events = index.events();
    let mut entries = Vec::new();
    for &number in bodies {
        let Some(described) = index
            .body(number)
            .and_then(|tally| tally.description.as_ref())
        else {
            continue;
        };
        for send in &described.sends {
            // A place in the current text, which no file holds, says so
            // after the selector, where it does not part the two.
            let (place, qualifier) = match (send.call, described.placed) {
                (Some(call), Placed::Source) => (format!("{}:{}", call[0] + 1, call[1] + 1), ""),
                (Some(call), Placed::Current) => (
                    format!("{}:{}", call[0] + 1, call[1] + 1),
                    " (in its current text)",
                ),
                _ => ("(unplaced)".to_string(), ""),
            };
            let name = format!("{}{qualifier}", send.name.as_deref().unwrap_or("send"));
            let outcome = index.send_outcome(number, send.instruction);
            let mut said = Vec::new();
            if let Some(decision) = outcome.inlining {
                let event = &events[decision as usize];
                let fields = index.fields(event);
                let key = event_key(fields);
                match index.event_word(event) {
                    "inlined" => {
                        let callee = event
                            .callee()
                            .map_or_else(|| "?".to_string(), |callee| named(index, callee));
                        said.push(format!("inlined {{{callee}}} ({key})"));
                    }
                    word => {
                        let reason = event
                            .reason()
                            .map_or_else(String::new, |reason| format!(" {}", index.text(reason)));
                        said.push(format!("{word}{reason} ({key})"));
                    }
                }
            }
            if let Some(cache) = outcome.cache {
                let mut text = format!("cache {}", index.text(cache.after));
                match cache.receivers {
                    0 => {}
                    1 => {
                        if let Some(receiver) =
                            field_value(index.fields(&events[cache.event as usize]), "receiver")
                        {
                            text.push_str(&format!(" {receiver}"));
                        }
                    }
                    receivers => text.push_str(&format!(" ({receivers} receivers)")),
                }
                said.push(text);
            }
            let mut entry = format!("  #{number} {place} {name}");
            if !said.is_empty() {
                entry.push(' ');
                entry.push_str(&said.join("; "));
            }
            entries.push(entry);
        }
    }
    entries
}

/// `debug_trace_listing`: a window of one listing of the body `method`
/// names -- its bytecode, one copy of its machine code or one inlining
/// plan, `which` the 1-based index among the body's records of `kind` in
/// file order, the last by default --, read back from the file and written
/// as `--print-*` writes it (`trace_listings::rendered`). A first line says
/// which listing it is; the window counts the listing's lines without its
/// `---` head and `--- end`, and ends in its `Showing` line. A name of
/// several bodies with such records is asked back, each with its number.
pub(crate) fn listing_window(
    index: &TraceIndex,
    session: &SessionHandle,
    method: &str,
    kind: ListingKind,
    which: Option<usize>,
    start: Option<usize>,
    count: Option<usize>,
) -> Answer {
    let shown = count_of(count, LINE_DEFAULT, LINE_MAXIMUM)?;
    let bodies = index.bodies_named(method)?;
    let candidates = listing_candidates(index, &bodies, kind);
    let (number, records) = match candidates.as_slice() {
        [] => return Err(no_listing(index, method, kind)),
        [only] => only.clone(),
        several => {
            return Err(ask_back(
                index, session, method, kind, several, which, start, count,
            ));
        }
    };
    let total = records.len();
    let listings_of = |total: usize| {
        let singular = format!("{} listing", kind.name());
        let plural = format!("{} listings", kind.name());
        counted(total as u64, &singular, &plural)
    };
    let position = match which {
        None => total,
        Some(0) => return Err("index 0: a body's listings count from 1".to_string()),
        Some(asked) if asked > total => {
            return Err(format!(
                "index {asked} is past the end: {} has {}",
                named(index, number),
                listings_of(total)
            ));
        }
        Some(asked) => asked,
    };
    let entry = &index.listings()[records[position - 1]];
    let record = index.read_record(entry.offset, entry.length)?;
    let bytecode = scopes_bytecode(index, &record)?;
    let names = |body: u32| {
        index
            .body_name(body)
            .map_or_else(|| format!("#{body}"), ToString::to_string)
    };
    let rendered = trace_listings::rendered(&record, &bytecode, &names);
    // The `---` head and `--- end` stand outside the window: the first
    // line says what the head does.
    let lines: &[String] = match rendered.as_slice() {
        [_, inner @ .., _] => inner,
        _ => &[],
    };
    let mut head = format!(
        "{} {} of {} of {}: {}, {}",
        kind.name(),
        thousands(position as u64),
        thousands(total as u64),
        named(index, number),
        listing_summary(index, entry, &record),
        counted(lines.len() as u64, "line", "lines")
    );
    if let Some(failure) = index.failure() {
        head.push_str("; ");
        head.push_str(failure);
    }
    let start = start.unwrap_or(0);
    if start > 0 && start >= lines.len() {
        return Err(format!(
            "start {start} is past the end: the listing has {}",
            counted(lines.len() as u64, "line", "lines")
        ));
    }
    // The next call names the listing by its index, given or not: a
    // running program's next copy would otherwise be the last one.
    let next_from = |last: usize| {
        let mut arguments = json!({
            "session": session.number,
            "method": method,
            "kind": kind.name(),
            "index": position,
            "start": last,
        });
        if let Some(count) = count {
            arguments["count"] = json!(count);
        }
        ("debug_trace_listing", arguments)
    };
    let total_lines = lines.len();
    if total_lines == 0 {
        return Ok(head);
    }
    let showing_at_most = characters(&showing_line(
        total_lines,
        total_lines,
        total_lines,
        Some(next_from(total_lines)),
    )) + 1;
    let mut used = characters(&head) + 1;
    let mut answer = vec![head];
    let mut last = start;
    for line in lines.iter().skip(start).take(shown) {
        let length = characters(line) + 1;
        if used + length + showing_at_most > ANSWER_BUDGET {
            break;
        }
        used += length;
        answer.push(line.clone());
        last += 1;
    }
    let next = (last < total_lines).then(|| next_from(last));
    answer.push(showing_line(start + 1, last, total_lines, next));
    Ok(answer.join("\n"))
}

/// The bodies of `bodies` with records of `kind`, the lowest number first,
/// each with the indexes of its records among the file's listings, in
/// file order: for bytecode, each family root's, whose record lists every
/// block of it.
fn listing_candidates(
    index: &TraceIndex,
    bodies: &[u32],
    kind: ListingKind,
) -> Vec<(u32, Vec<usize>)> {
    let records_of = |number: u32| -> Vec<usize> {
        match kind {
            ListingKind::Code | ListingKind::Inlining => {
                let Some(tally) = index.body(number) else {
                    return Vec::new();
                };
                let records = if kind == ListingKind::Code {
                    &tally.code
                } else {
                    &tally.inlining
                };
                records.iter().map(|record| *record as usize).collect()
            }
            ListingKind::Bytecode => index
                .listings()
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.kind == ListingKind::Bytecode && entry.body == number)
                .map(|(position, _)| position)
                .collect(),
        }
    };
    let mut numbers: Vec<u32> = match kind {
        ListingKind::Bytecode => bodies
            .iter()
            .filter_map(|number| index.bytecode_of(*number).map(|entry| entry.body))
            .collect(),
        ListingKind::Code | ListingKind::Inlining => bodies.to_vec(),
    };
    numbers.sort_unstable();
    numbers.dedup();
    numbers
        .into_iter()
        .map(|number| (number, records_of(number)))
        .filter(|(_, records)| !records.is_empty())
        .collect()
}

/// Why `method` has no listing of `kind`: the run lists none, or the
/// binary has no JIT to list machine code of.
fn no_listing(index: &TraceIndex, method: &str, kind: ListingKind) -> String {
    let mut refusal = format!("no {} listing of {method}", kind.name());
    if !index.lists(kind) {
        refusal.push_str(&format!(
            " — the session lists no {0}: debug_start with listings [\"{0}\"] lists it",
            kind.name()
        ));
    } else if kind != ListingKind::Bytecode
        && index
            .header()
            .is_some_and(|header| header.architecture.is_none())
    {
        refusal.push_str(" — this ns-next has no JIT: every body runs interpreted");
    }
    refusal
}

/// The refusal of a name of several bodies with listings of `kind`: each
/// with its number, what it is and its records' indexes, and the call
/// that answers the first, every other argument given kept.
#[expect(
    clippy::too_many_arguments,
    reason = "the call's own arguments, which the call it suggests repeats"
)]
fn ask_back(
    index: &TraceIndex,
    session: &SessionHandle,
    method: &str,
    kind: ListingKind,
    candidates: &[(u32, Vec<usize>)],
    which: Option<usize>,
    start: Option<usize>,
    count: Option<usize>,
) -> String {
    let description = |number: u32| index.body(number)?.description.as_ref();
    let mut roots: Vec<&str> = candidates
        .iter()
        .filter_map(|(number, _)| description(*number))
        .filter(|described| described.family.is_none())
        .map(|described| described.name.as_str())
        .collect();
    roots.dedup();
    let one_method = {
        let mut names = roots.clone();
        names.sort_unstable();
        names.dedup();
        names.len() == 1
    };
    let mut lead: Option<&BodyRecord> = None;
    let items: Vec<String> = candidates
        .iter()
        .map(|(number, records)| {
            let what = match description(*number) {
                None => "(not described)".to_string(),
                Some(described) if described.family.is_some() => described.name.clone(),
                Some(described) => match lead {
                    None => {
                        lead = Some(described);
                        if one_method && described.name.contains(">>") {
                            "method".to_string()
                        } else {
                            described.name.clone()
                        }
                    }
                    Some(first) if first.module != described.module => {
                        format!("{} of {}", described.name, module_text(&described.module))
                    }
                    Some(first) if first.name == described.name => {
                        format!("{}, an edited version", described.name)
                    }
                    Some(_) => described.name.clone(),
                },
            };
            let positions: Vec<usize> = (1..=records.len()).collect();
            format!("#{number} {what} ({} {})", kind.name(), ranges(&positions))
        })
        .collect();
    let mut arguments = json!({
        "session": session.number,
        "method": format!("#{}", candidates[0].0),
        "kind": kind.name(),
    });
    for (name, value) in [("index", which), ("start", start), ("count", count)] {
        if let Some(value) = value {
            arguments[name] = json!(value);
        }
    }
    format!(
        "{method} matches {} bodies: {} — pass one: {}",
        thousands(candidates.len() as u64),
        items.join(", "),
        call_text("debug_trace_listing", &arguments)
    )
}

/// The `bytecode` records of the families of `record`'s scopes, each read
/// once, whose instructions its position lines name.
fn scopes_bytecode(index: &TraceIndex, record: &Value) -> Result<Vec<Value>, String> {
    let mut entries: Vec<&ListingEntry> = Vec::new();
    for scope in record["scopes"].as_array().map_or(&[][..], Vec::as_slice) {
        let Some(body) = scope["body"]
            .as_u64()
            .and_then(|body| u32::try_from(body).ok())
        else {
            continue;
        };
        if let Some(entry) = index.bytecode_of(body)
            && !entries.iter().any(|known| known.offset == entry.offset)
        {
            entries.push(entry);
        }
    }
    entries
        .into_iter()
        .map(|entry| index.read_record(entry.offset, entry.length))
        .collect()
}

/// What the window's first line says of the listing `entry` indexes
/// beside its place: a copy's tier, architecture, key and size; a plan's
/// key and outcome; a bytecode listing's bodies, which its `record` holds.
fn listing_summary(index: &TraceIndex, entry: &ListingEntry, record: &Value) -> String {
    let key = key_text(
        index,
        CopyKey {
            key: entry.key,
            entered: entry.entered,
        },
    );
    let keyed = entry.key.is_some() || entry.entered.is_some();
    match entry.kind {
        ListingKind::Code => {
            let mut summary = format!(
                "{} {}",
                entry.tier.map_or("", |tier| index.text(tier)),
                record["architecture"].as_str().unwrap_or_default()
            );
            if keyed {
                summary.push(' ');
                summary.push_str(&key);
            }
            summary.push_str(&format!(
                ", {}",
                counted(entry.bytes.unwrap_or(0), "byte", "bytes")
            ));
            summary
        }
        ListingKind::Inlining => {
            let outcome = match entry.outcome {
                Some(PlanOutcome::Installed(bytes)) => {
                    format!("installed {}", counted(bytes, "byte", "bytes"))
                }
                Some(PlanOutcome::Refused(reason)) => format!("refused {}", index.text(reason)),
                None => "no outcome".to_string(),
            };
            if keyed {
                format!("{key}, {outcome}")
            } else {
                outcome
            }
        }
        ListingKind::Bytecode => counted(
            record["bodies"].as_array().map_or(0, Vec::len) as u64,
            "body",
            "bodies",
        ),
    }
}

/// What `debug_trace_events` selects by: every filter given must hold.
pub(crate) struct EventQuery {
    categories: Option<TraceCategories>,
    /// A line's word or a record's event word: `monomorphic->polymorphic`
    /// or `transition`.
    event: Option<String>,
    /// An event's `reason`, `cause`, `by` or `walk-end`, or one of
    /// `declined-no-body`'s reasons.
    reason: Option<String>,
    /// The bodies `method` named, the lowest first.
    bodies: Option<Vec<u32>>,
    /// The filters as the call gave them, in the tool's order, which the
    /// head and the next call repeat.
    given: Vec<(&'static str, String)>,
}

impl EventQuery {
    /// The query of the filters given: `category` one category or several
    /// joined by commas, as `--trace` takes them, `method` a name in the
    /// tools' grammar or `#<n>`, resolved as the journal resolves it
    /// (`TraceIndex::bodies_named`), so that the call a journal names
    /// answers the journal's bodies. An unknown category and a name the
    /// filter's grammar or the index refuses are refused with its reason.
    pub(crate) fn new(
        index: &TraceIndex,
        category: Option<&str>,
        event: Option<&str>,
        reason: Option<&str>,
        method: Option<&str>,
    ) -> Result<EventQuery, String> {
        let categories = category
            .map(|text| {
                crate::trace::categories_named(text).map_err(|reason| format!("category: {reason}"))
            })
            .transpose()?;
        let bodies = method.map(|name| index.bodies_named(name)).transpose()?;
        let given = [
            ("category", category),
            ("event", event),
            ("reason", reason),
            ("method", method),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name, value.to_string())))
        .collect();
        Ok(EventQuery {
            categories,
            event: event.map(str::to_string),
            reason: reason.map(str::to_string),
            bodies,
            given,
        })
    }

    /// Whether the event `number` of `index` meets every filter; `reason`
    /// is the query's reason as interned -- `Some(None)` where no text of
    /// the file is it, so that no event meets it.
    fn matches(&self, index: &TraceIndex, number: usize, reason: Option<Option<u32>>) -> bool {
        let event = &index.events()[number];
        if self
            .categories
            .is_some_and(|categories| !categories.contains(event.category()))
        {
            return false;
        }
        if let Some(word) = &self.event
            && index.text(event.word()) != word
            && index.event_word(event) != word
        {
            return false;
        }
        if let Some(reason) = reason {
            let Some(reason) = reason else {
                return false;
            };
            let declined = u32::try_from(number).map_or(&[][..], |number| index.reasons(number));
            if event.reason() != Some(reason) && !declined.iter().any(|(known, _)| *known == reason)
            {
                return false;
            }
        }
        if let Some(bodies) = &self.bodies
            && !event
                .bodies()
                .any(|body| bodies.binary_search(&body).is_ok())
        {
            return false;
        }
        true
    }
}

/// `debug_trace_events`: the events `query` selects, in file order and
/// unfolded, from `start`, each numbered by its place among all the run's
/// events and written as the text trace writes its line, every body named
/// `{<name> #<n>}` (`event_line`). A first line names the session, the
/// filters, the asked categories the run does not trace and the failure
/// that ended the reading early; the answer ends in its `Showing` line,
/// with the call that goes on, which repeats every argument given,
/// `start` advanced.
pub(crate) fn events(
    index: &TraceIndex,
    session: &SessionHandle,
    query: &EventQuery,
    start: Option<usize>,
    count: Option<usize>,
) -> Answer {
    let shown = count_of(count, ENTRY_DEFAULT, ENTRY_MAXIMUM)?;
    let reason = query.reason.as_deref().map(|reason| index.interned(reason));
    let selected: Vec<usize> = (0..index.events().len())
        .filter(|number| query.matches(index, *number, reason))
        .collect();
    let mut head = format!("events of session {}", session.number);
    if !query.given.is_empty() {
        let filters: Vec<String> = query
            .given
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        head.push_str(", ");
        head.push_str(&filters.join(" "));
    }
    // An asked category the run does not trace has no events for good,
    // which an empty page would not tell from a quiet one.
    let untraced: Vec<&str> = query.categories.map_or_else(Vec::new, |categories| {
        TraceCategory::ALL
            .into_iter()
            .filter(|category| categories.contains(*category) && !index.traces(*category))
            .map(TraceCategory::name)
            .collect()
    });
    let mut tail = String::new();
    if !untraced.is_empty() {
        tail.push_str("; not traced: ");
        tail.push_str(&untraced.join(", "));
    }
    if let Some(failure) = index.failure() {
        tail.push_str("; ");
        tail.push_str(failure);
    }
    let total = selected.len();
    let start = start.unwrap_or(0);
    if total == 0 && start == 0 {
        return Ok(format!("{head}: none{tail}"));
    }
    if start >= total {
        return Err(format!(
            "start {start} is past the end: {} {}",
            counted(total as u64, "event", "events"),
            if total == 1 { "matches" } else { "match" }
        ));
    }
    let next_from = |last: usize| {
        let mut arguments = json!({ "session": session.number, "start": last });
        for (name, value) in &query.given {
            arguments[*name] = json!(value);
        }
        if let Some(count) = count {
            arguments["count"] = json!(count);
        }
        ("debug_trace_events", arguments)
    };
    let head_of = |taken: usize| {
        format!(
            "{head} ({} of {}){tail}",
            thousands(taken as u64),
            thousands(total as u64)
        )
    };
    // The head and the `Showing` line as long as they can be.
    let mut used = characters(&head_of(total))
        + 1
        + characters(&showing_line(total, total, total, Some(next_from(total))));
    let mut lines = Vec::new();
    for &number in selected.iter().skip(start).take(shown) {
        let line = format!("  {}", event_line(index, number));
        let length = characters(&line) + 1;
        if used + length > ANSWER_BUDGET {
            break;
        }
        used += length;
        lines.push(line);
    }
    let last = start + lines.len();
    let next = (last < total).then(|| next_from(last));
    let mut answer = vec![head_of(lines.len())];
    answer.extend(lines);
    answer.push(showing_line(start + 1, last, total, next));
    Ok(answer.join("\n"))
}

/// The event `number` of `index` as the text trace writes its line, its
/// place among the run's events, from 1, before it:
/// `@1,204 [deoptimize] {Bounce>>run #4} retired key={Bounce}
/// reason=exits version=2`, `{}` for an event about no body, every body
/// field `{<name> #<n>}`.
fn event_line(index: &TraceIndex, number: usize) -> String {
    let event = &index.events()[number];
    let body = event
        .body()
        .map_or_else(String::new, |body| named(index, body));
    format!(
        "@{} [{}] {{{body}}} {}{}",
        thousands(number as u64 + 1),
        event.category().name(),
        index.text(event.word()),
        bodies_named_in(index, index.fields(event))
    )
}

/// `fields` with every body field's `{#<n>}` written `{<name> #<n>}`.
fn bodies_named_in(index: &TraceIndex, fields: &str) -> String {
    let mut text = String::with_capacity(fields.len());
    let mut rest = fields;
    while let Some(found) = rest.find("{#") {
        text.push_str(&rest[..found]);
        let after = &rest[found + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let body = after[digits..]
            .starts_with('}')
            .then(|| after[..digits].parse::<u32>().ok())
            .flatten();
        match body {
            Some(body) => {
                text.push('{');
                text.push_str(&named(index, body));
                text.push('}');
                rest = &after[digits + 1..];
            }
            // A class's name never begins with `#`: only a body field is
            // written so, and anything else stays as it is.
            None => {
                text.push_str("{#");
                rest = after;
            }
        }
    }
    text.push_str(rest);
    text
}

#[cfg(test)]
mod tests {
    use super::super::registry::{Registry, SessionState};
    use super::super::trace_index::tests::{Scratch, event_of};
    use super::super::trace_listings::tests::{
        NAMES, printed, run_bytecode, run_inlining, run_optimized, square_bytecode, square_code,
    };
    use super::*;
    use crate::trace_file::listing_record;
    use ns_runtime::trace_events::{BodyName, InliningOutcome, Listing};

    /// A session `Bounce.ns`, stopped, and the registry that holds it.
    fn stopped_session() -> (Registry, std::sync::Arc<SessionHandle>) {
        let registry = Registry::new();
        let (_, _, handle) = registry.open("Bounce.ns".to_string());
        handle.publish(SessionState::Stopped);
        (registry, handle)
    }

    fn compiled(body: u32) -> Value {
        event_of(
            "compile",
            Some(body),
            &json!({ "event": "compiled", "trigger": "call", "bytes": 96 }),
        )
    }

    /// A trace of every category but `deoptimize`: bodies 1 to 12, body
    /// `n` compiled `n` times, body 3 abandoned twice and revived once,
    /// body 2 optimized for two keys, body 20 declined in bodies 1 and 2.
    fn written() -> Scratch {
        let mut scratch = Scratch::new("overview");
        scratch.header(
            &[
                "compile",
                "osr",
                "optimize",
                "inlining",
                "code-zone",
                "caches",
            ],
            &["code"],
        );
        for number in 1..=12 {
            scratch.body(number, &format!("Bounce>>m{number}"), None);
            for _ in 0..number {
                scratch.record(&compiled(number));
            }
        }
        for _ in 0..2 {
            scratch.record(&event_of(
                "compile",
                Some(3),
                &json!({ "event": "abandoned", "exits": 64 }),
            ));
        }
        scratch.record(&event_of(
            "compile",
            Some(3),
            &json!({ "event": "revived", "by": "decay" }),
        ));
        let optimized = |key: &str, bytes: u64| {
            event_of(
                "optimize",
                Some(2),
                &json!({ "event": "optimized", "key": key, "entered": null, "bytes": bytes,
                         "inlined-scopes": 0, "inlined-sites": 0, "type-case-arms": 0,
                         "eliminated-blocks": 0 }),
            )
        };
        scratch.record(&optimized("Bounce", 100));
        scratch.record(&optimized("Bounce", 200));
        scratch.record(&optimized("Bounce/Sub", 300));
        let declined = |body: u32, reason: &str| {
            event_of(
                "inlining",
                Some(body),
                &json!({ "event": "declined", "key": "Bounce", "entered": null, "in-body": body,
                         "callee": 20, "reason": reason, "at": 0 }),
            )
        };
        scratch.record(&declined(1, "over-weight"));
        scratch.record(&declined(2, "over-weight"));
        scratch.record(&declined(2, "block-escapes"));
        scratch.record(&event_of(
            "inlining",
            Some(1),
            &json!({ "event": "declined-no-body", "key": "Bounce", "entered": null, "count": 3,
                     "reasons": [{ "reason": "not-snapshotted", "count": 2 },
                                 { "reason": "not-expressed", "count": 1 }] }),
        ));
        scratch
    }

    #[test]
    fn the_overview_ranks_its_lists() {
        let scratch = written();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(index.failure(), None);
        let answer = overview(&index, &handle, None, None, None).expect("an overview");
        let lines: Vec<&str> = answer.lines().collect();
        let size = size(index.bytes());
        assert_eq!(
            lines[0],
            format!("session 1 (Bounce.ns, stopped): {size}, 88 events, 13 bodies, 0 listings")
        );
        assert_eq!(
            lines[1],
            "traced: compile, osr, optimize, inlining, code-zone, caches; listed: code"
        );
        assert_eq!(
            lines[2],
            "events: compile 81 · osr 0 · optimize 3 · inlining 4 · code-zone 0 · caches 0"
        );
        assert_eq!(
            lines[3],
            r##"installs (10 of 12) — next: debug_trace {"session":1,"list":"installs","start":10}"##
        );
        // The most installs first, body 2's optimized copies among its
        // installs, and a tie by body number.
        assert_eq!(
            lines[4],
            r##"  Bounce>>m12 #12: 12 (baseline 12) → debug_trace_method {"session":1,"method":"#12"}"##
        );
        assert!(lines[5].starts_with("  Bounce>>m11 #11: 11 "), "{answer}");
        assert!(
            lines[11].starts_with("  Bounce>>m2 #2: 5 (baseline 2, optimized 3) "),
            "{answer}"
        );
        assert!(lines[12].starts_with("  Bounce>>m5 #5: 5 "), "{answer}");
        assert!(lines[13].starts_with("  Bounce>>m4 #4: 4 "), "{answer}");
        assert_eq!(lines[14], "abandoned (1 of 1)");
        assert_eq!(
            lines[15],
            r##"  Bounce>>m3 #3: 2 (128 exits), revived 1 (decay 1) → debug_trace_method {"session":1,"method":"#3"}"##
        );
        assert_eq!(lines[16], "deoptimized: not traced (deoptimize)");
        assert_eq!(lines[17], "optimized (1 of 1)");
        assert_eq!(
            lines[18],
            r##"  Bounce>>m2 #2: 3 copies (key={Bounce} 2, key={Bounce/Sub} 1), last 300 bytes → debug_trace_method {"session":1,"method":"#2"}"##
        );
        assert_eq!(lines[19], "declined (1 of 1)");
        assert_eq!(
            lines[20],
            r##"  #20 (not described): 3 (over-weight 2, block-escapes 1) in Bounce>>m2 #2 (2), Bounce>>m1 #1 (1) → debug_trace_events {"session":1,"event":"declined","method":"#20"}"##
        );
        assert_eq!(
            lines[21],
            "  callees that are no body: 3 (not-snapshotted 2, not-expressed 1)"
        );
        assert_eq!(lines.len(), 22, "{answer}");
    }

    #[test]
    fn a_list_alone_is_paged_with_its_exact_next() {
        let scratch = written();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let page = overview(&index, &handle, Some("installs"), Some(2), Some(3)).expect("a page");
        let lines: Vec<&str> = page.lines().collect();
        assert_eq!(lines[1], "installs");
        assert!(lines[2].starts_with("  Bounce>>m10 #10: 10 "), "{page}");
        assert!(lines[4].starts_with("  Bounce>>m8 #8: 8 "), "{page}");
        assert_eq!(
            lines[5],
            r##"Showing 3–5 of 12 — next: debug_trace {"session":1,"list":"installs","start":5,"count":3}"##
        );
        assert_eq!(lines.len(), 6);
        // The last page says so, and names no next.
        let last = overview(&index, &handle, Some("installs"), Some(10), None).expect("a page");
        assert!(last.ends_with("\nShowing 11–12 of 12"), "{last}");
        // A count passed on in the overview's head too.
        let counted = overview(&index, &handle, None, None, Some(2)).expect("an overview");
        assert!(
            counted.contains(
                r##"installs (2 of 12) — next: debug_trace {"session":1,"list":"installs","start":2,"count":2}"##
            ),
            "{counted}"
        );
        assert_eq!(
            overview(&index, &handle, Some("deoptimized"), None, None),
            Ok(format!(
                "{}\ndeoptimized: not traced (deoptimize)",
                page.lines().next().unwrap()
            ))
        );
        assert_eq!(
            overview(&index, &handle, Some("installs"), Some(12), None),
            Err("start 12 is past the end: installs has 12 entries".to_string())
        );
        assert_eq!(
            overview(&index, &handle, Some("everything"), None, None),
            Err(
                r##""everything" is no list: installs, abandoned, deoptimized, optimized or declined"##
                    .to_string()
            )
        );
        assert!(overview(&index, &handle, None, Some(3), None).is_err());
    }

    /// A trace of a binary without a JIT holds no install, and says so.
    #[test]
    fn a_trace_without_machine_code_has_no_installs() {
        let mut scratch = Scratch::new("no-installs");
        scratch.header(
            &[
                "compile",
                "osr",
                "optimize",
                "inlining",
                "deoptimize",
                "code-zone",
                "caches",
            ],
            &[],
        );
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer = overview(&index, &handle, None, None, None).expect("an overview");
        let lists: Vec<&str> = answer.lines().skip(3).collect();
        assert_eq!(
            lists,
            [
                "installs: none",
                "abandoned: none",
                "deoptimized: none",
                "optimized: none",
                "declined: none"
            ]
        );
        assert!(answer.contains("listed: nothing"), "{answer}");
    }

    /// However many entries are asked, the answer stays under the budget,
    /// each cut list's head saying where it stopped.
    #[test]
    fn the_overview_stays_under_its_budget() {
        let mut scratch = Scratch::new("overview-budget");
        scratch.header(&["compile"], &[]);
        let long = "x".repeat(400);
        for number in 1..=200 {
            scratch.body(number, &format!("Bounce>>{long}{number}"), None);
            scratch.record(&compiled(number));
        }
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer = overview(&index, &handle, None, None, Some(200)).expect("an overview");
        assert!(
            characters(&answer) <= ANSWER_BUDGET,
            "{}",
            characters(&answer)
        );
        let head = answer
            .lines()
            .find(|line| line.starts_with("installs ("))
            .expect("the installs head");
        let taken: usize = head["installs (".len()..]
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(taken > 0 && taken < 200, "{head}");
        assert!(
            head.ends_with(&format!(
                r##"next: debug_trace {{"session":1,"list":"installs","start":{taken},"count":200}}"##
            )),
            "{head}"
        );
        let page = overview(&index, &handle, Some("installs"), None, Some(200)).expect("a page");
        assert!(characters(&page) <= ANSWER_BUDGET, "{}", characters(&page));
        let showing = page.lines().last().unwrap();
        assert!(showing.starts_with("Showing 1–"), "{showing}");
        assert!(showing.contains(" — next: "), "{showing}");
    }

    /// Four lists full of long entries, and a callee that is no body
    /// declined for many reasons: the lines written after the earlier
    /// lists' entries -- the later heads, `declined: none` and the closing
    /// line -- still fit the budget.
    #[test]
    fn every_list_full_stays_under_the_budget() {
        let mut scratch = Scratch::new("every-list-budget");
        scratch.header(
            &[
                "compile",
                "osr",
                "optimize",
                "inlining",
                "deoptimize",
                "code-zone",
                "caches",
            ],
            &[],
        );
        let long = "y".repeat(400);
        for number in 1..=150 {
            scratch.body(number, &format!("Bounce>>{long}{number}"), None);
            scratch.record(&compiled(number));
            scratch.record(&event_of(
                "compile",
                Some(number),
                &json!({ "event": "abandoned", "exits": 64 }),
            ));
            scratch.record(&event_of(
                "deoptimize",
                Some(number),
                &json!({ "event": "retired", "key": "Bounce", "entered": null,
                         "reason": "exits", "version": 2 }),
            ));
            scratch.record(&event_of(
                "optimize",
                Some(number),
                &json!({ "event": "optimized", "key": "Bounce", "entered": null, "bytes": 100,
                         "inlined-scopes": 0, "inlined-sites": 0, "type-case-arms": 0,
                         "eliminated-blocks": 0 }),
            ));
        }
        let reasons: Vec<Value> = (0..12)
            .map(|number| {
                json!({
                    "reason": format!("a-reason-of-a-length-no-decline-has-yet-{number}"),
                    "count": 1_000 + number,
                })
            })
            .collect();
        scratch.record(&event_of(
            "inlining",
            Some(1),
            &json!({ "event": "declined-no-body", "key": "Bounce", "entered": null,
                     "count": 12, "reasons": reasons }),
        ));
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(index.failure(), None);
        let answer = overview(&index, &handle, None, None, Some(200)).expect("an overview");
        assert!(
            characters(&answer) <= ANSWER_BUDGET,
            "{}",
            characters(&answer)
        );
        for list in &LISTS[..4] {
            assert!(
                answer
                    .lines()
                    .any(|line| line.starts_with(&format!("{list} ("))
                        && line.contains(" of 150) — next: ")),
                "no cut head of {list}:\n{answer}"
            );
        }
        let lines: Vec<&str> = answer.lines().collect();
        assert_eq!(lines[lines.len() - 2], "declined: none", "{answer}");
        assert!(
            lines[lines.len() - 1].starts_with("  callees that are no body: 12,066 ("),
            "{answer}"
        );
    }

    /// A body the trace numbers without describing it is named by its
    /// number, saying so, and addressed by it; an optimized body whose
    /// plans were listed names its last plan's listing.
    #[test]
    fn an_undescribed_body_is_named_by_its_number() {
        let mut scratch = Scratch::new("undescribed");
        scratch.header(&["compile", "optimize"], &["inlining"]);
        scratch.record(&compiled(44));
        scratch.record(&event_of(
            "optimize",
            Some(44),
            &json!({ "event": "optimized", "key": null, "entered": null, "bytes": 1_053,
                     "inlined-scopes": 0, "inlined-sites": 0, "type-case-arms": 0,
                     "eliminated-blocks": 0 }),
        ));
        scratch.record(&json!({
            "kind": "inlining", "body": 44, "key": null, "entered": null,
            "outcome": { "installed": 1_053 }, "scopes": [], "arms": [], "declined": [],
        }));
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(index.failure(), None);
        let answer = overview(&index, &handle, None, None, None).expect("an overview");
        assert!(
            answer.contains(
                r##"
  #44 (not described): 2 (baseline 1, optimized 1) → debug_trace_method {"session":1,"method":"#44"}
"##
            ),
            "{answer}"
        );
        assert!(
            answer.contains(
                r##"
  #44 (not described): 1 copy (no key 1), last 1,053 bytes → debug_trace_listing {"session":1,"method":"#44","kind":"inlining"}"##
            ),
            "{answer}"
        );
        assert_eq!(index.bodies_named("#44"), Ok(vec![44]));
    }

    #[test]
    fn a_summary_s_trace_line_names_the_overview() {
        let scratch = written();
        let index = scratch.caught_up();
        assert_eq!(
            trace_line(&index, 4),
            format!(
                r##"trace: {}, 88 events, 13 bodies, 0 listings — debug_trace {{"session":4}} for an overview"##,
                size(index.bytes())
            )
        );
    }

    #[test]
    fn a_fold_writes_a_repeated_cycle_once() {
        // A cycle of `(text, count)` steps from the text at `first`.
        let entry = |first: usize, cycle: &[(u32, usize)], repetitions: usize| Folded {
            first,
            cycle: cycle
                .iter()
                .map(|&(text, count)| Repeat { text, count })
                .collect(),
            repetitions,
        };
        assert_eq!(
            fold(&[1, 2, 3, 2, 3, 2, 3, 4]),
            [
                entry(0, &[(1, 1)], 1),
                entry(1, &[(2, 1), (3, 1)], 3),
                entry(7, &[(4, 1)], 1)
            ]
        );
        assert_eq!(fold(&[5, 5, 5]), [entry(0, &[(5, 3)], 1)]);
        // A cycle of five steps, one of them a repeat: a body's compile,
        // twin, abandonment, two freeings and revival.
        let round = [1, 2, 3, 4, 4, 5];
        let rounds: Vec<u32> = round.iter().copied().cycle().take(6 * 554).collect();
        assert_eq!(
            fold(&rounds),
            [entry(0, &[(1, 1), (2, 1), (3, 1), (4, 2), (5, 1)], 554)]
        );
        // Eight steps fold; nine are longer than a fold writes once.
        let eight: Vec<u32> = (1..=8).chain(1..=8).collect();
        assert_eq!(fold(&eight).len(), 1);
        let nine: Vec<u32> = (1..=9).chain(1..=9).collect();
        assert_eq!(
            fold(&nine),
            (0..18)
                .map(|first| entry(first, &[(nine[first], 1)], 1))
                .collect::<Vec<_>>()
        );
        // The shortest cycle first.
        assert_eq!(
            fold(&[1, 2, 3, 4, 1, 2, 3, 4, 6, 6]),
            [
                entry(0, &[(1, 1), (2, 1), (3, 1), (4, 1)], 2),
                entry(8, &[(6, 2)], 1)
            ]
        );
        assert_eq!(fold(&[]), []);
    }

    /// A body record of `Bounce@1.0.0`'s file, standing at `line` (from 0).
    fn bounce_body(number: u32, name: &str, family: Option<u32>, line: u32, sends: Value) -> Value {
        json!({
            "kind": "body", "body": number, "name": name, "family": family,
            "module": { "id": "Bounce", "version": "1.0.0" },
            "source": { "file": "D:\\work\\Bounce.ns" },
            "range": [line, 2, line + 2, 3], "placed": "source",
            "checksum": "0000000000000001", "statements": [], "sends": sends,
        })
    }

    fn code_record(body: u32, tier: &str, key: Option<&str>) -> Value {
        json!({ "kind": "code", "body": body, "tier": tier, "key": key, "entered": key,
                "bytes": 100, "instructions": [] })
    }

    fn optimized_event(body: u32, key: &str) -> Value {
        event_of(
            "optimize",
            Some(body),
            &json!({ "event": "optimized", "key": key, "entered": key, "bytes": 900,
                     "inlined-scopes": 1, "inlined-sites": 1, "type-case-arms": 0,
                     "eliminated-blocks": 0 }),
        )
    }

    /// `Bounce/Ball>>bounce`, #12, with two blocks: compiled, abandoned and
    /// revived three times over, its twin, four on-stack entries of a
    /// block, two optimized copies of one key that inline `x`, a cache
    /// filled; inlined and declined in `Bounce>>run`'s compile.
    fn bounce_journal() -> Scratch {
        let mut scratch = Scratch::new("journal");
        scratch.header(
            &[
                "compile",
                "osr",
                "optimize",
                "inlining",
                "deoptimize",
                "code-zone",
                "caches",
            ],
            &["code", "inlining"],
        );
        scratch.record(&bounce_body(4, "Bounce>>run", None, 3, json!([])));
        scratch.record(&bounce_body(
            12,
            "Bounce/Ball>>bounce",
            None,
            13,
            json!([
                { "at": 0, "site": 0, "call": [14, 8, 14, 12], "selector": [[14, 10, 14, 12]],
                  "name": "x:" },
                { "at": 4, "site": 1, "call": [15, 4, 15, 20], "selector": [[15, 8, 15, 11]],
                  "name": "do:" },
                // A record of a writer that named no selector.
                { "at": 7, "site": null, "call": null, "selector": [] },
            ]),
        ));
        scratch.record(&bounce_body(
            13,
            "[] in Bounce/Ball>>bounce",
            Some(12),
            15,
            json!([]),
        ));
        scratch.record(&bounce_body(
            15,
            "[] in Bounce/Ball>>bounce",
            Some(12),
            18,
            json!([]),
        ));
        scratch.record(&bounce_body(30, "Bounce/Ball>>x", None, 30, json!([])));
        let compile = |fields: Value| event_of("compile", Some(12), &fields);
        scratch.record(&compile(
            json!({ "event": "compiled", "trigger": "call", "bytes": 1216 }),
        ));
        scratch.record(&code_record(12, "baseline", None));
        scratch.record(&code_record(12, "baseline", None));
        for _ in 0..3 {
            scratch.record(&compile(json!({ "event": "abandoned", "exits": 64 })));
            scratch.record(&compile(json!({ "event": "revived", "by": "decay" })));
            scratch.record(&compile(
                json!({ "event": "compiled", "trigger": "entry", "bytes": 1216 }),
            ));
        }
        scratch.record(&compile(json!({ "event": "compiled-twin", "bytes": 2000 })));
        scratch.record(&code_record(12, "twin", None));
        for _ in 0..4 {
            scratch.record(&event_of(
                "osr",
                Some(13),
                &json!({ "event": "osr-entered", "at": 3 }),
            ));
        }
        scratch.record(&event_of(
            "inlining",
            Some(12),
            &json!({ "event": "inlined", "key": "Bounce/Ball", "entered": "Bounce/Ball",
                     "in-body": 12, "callee": 30, "at": 0 }),
        ));
        for _ in 0..2 {
            scratch.record(&optimized_event(12, "Bounce/Ball"));
            scratch.record(&code_record(12, "optimized", Some("Bounce/Ball")));
            scratch.record(&json!({
                "kind": "inlining", "body": 12, "key": "Bounce/Ball", "entered": "Bounce/Ball",
                "outcome": { "installed": 900 }, "scopes": [], "arms": [], "declined": [],
            }));
        }
        scratch.record(&event_of(
            "inlining",
            Some(4),
            &json!({ "event": "inlined", "key": "Bounce", "entered": null, "in-body": 4,
                     "callee": 12, "at": 2 }),
        ));
        scratch.record(&event_of(
            "inlining",
            Some(4),
            &json!({ "event": "declined", "key": "Bounce", "entered": null, "in-body": 4,
                     "callee": 12, "reason": "over-weight", "at": 5 }),
        ));
        scratch.record(&optimized_event(4, "Bounce"));
        scratch.record(&event_of(
            "caches",
            Some(12),
            &json!({ "event": "transition", "before": "vacant", "after": "monomorphic",
                     "selector": "x:", "site": 0, "receiver": "Bounce/Ball" }),
        ));
        scratch
    }

    #[test]
    fn a_journal_names_its_bodies_copies_events_and_sends() {
        let scratch = bounce_journal();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(index.failure(), None);
        let answer =
            journal(&index, &handle, "Bounce/Ball>>bounce", None, None, None).expect("a journal");
        let expected = [
            r"Bounce/Ball>>bounce — Bounce@1.0.0, D:\work\Bounce.ns:14 — session 1",
            "bodies: #12 method (line 14), #13 [] in Bounce/Ball>>bounce (line 16), #15 [] in Bounce/Ball>>bounce (line 19)",
            "copies of #12: baseline 4 (code 1–2), twin 1 (code 3), optimized key={Bounce/Ball} entered={Bounce/Ball} 2 (code 4–5; inlining 1–2)",
            "inlined #12 into: Bounce>>run #4 ×1 (key={Bounce}); declined ×1 (over-weight 1)",
            "events (5 of 5 entries, folding 17 lifecycle events; every event, caches included: debug_trace_events {\"session\":1,\"method\":\"Bounce/Ball>>bounce\"})",
            "  #12 Bounce/Ball>>bounce — 4 entries, folding 13 events",
            "    [compile] (compiled trigger=call bytes=1216 → abandoned exits=64 → revived by=decay) ×3 — compiled trigger=entry ×2, trigger=call ×1",
            "    [compile] compiled trigger=entry bytes=1216",
            "    [compile] compiled-twin bytes=2000",
            "    [optimize] optimized key={Bounce/Ball} entered={Bounce/Ball} bytes=900 inlined-scopes=1 inlined-sites=1 type-case-arms=0 eliminated-blocks=0 ×2",
            "  #13 [] in Bounce/Ball>>bounce — 1 entry, folding 4 events",
            "    [osr] osr-entered at=3 ×4",
            "sends (3 of 3)",
            "  #12 15:9 x: inlined {Bounce/Ball>>x #30} (key={Bounce/Ball} entered={Bounce/Ball}); cache monomorphic {Bounce/Ball}",
            "  #12 16:5 do:",
            "  #12 (unplaced) send",
        ];
        assert_eq!(answer.lines().collect::<Vec<_>>(), expected, "{answer}");
        // A block alone, by its number.
        let block = journal(&index, &handle, "#13", None, None, None).expect("a journal");
        assert_eq!(
            block.lines().collect::<Vec<_>>(),
            [
                r"[] in Bounce/Ball>>bounce — Bounce@1.0.0, D:\work\Bounce.ns:16 — session 1",
                "bodies: #13 [] in Bounce/Ball>>bounce (line 16)",
                "events (1 of 1 entry, folding 4 lifecycle events; every event, caches included: debug_trace_events {\"session\":1,\"method\":\"#13\"})",
                "  #13 [] in Bounce/Ball>>bounce — 1 entry, folding 4 events",
                "    [osr] osr-entered at=3 ×4",
                "sends: none",
            ],
            "{block}"
        );
    }

    /// Every part pages with the call that goes on, which repeats every
    /// argument given, and says where it stopped.
    #[test]
    fn a_journal_s_parts_are_paged_with_their_exact_next() {
        let scratch = bounce_journal();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let method = "Bounce/Ball>>bounce";
        let page = journal(
            &index,
            &handle,
            method,
            Some(Part::Events),
            Some(2),
            Some(2),
        )
        .expect("a page");
        assert_eq!(
            page.lines().collect::<Vec<_>>(),
            [
                r"Bounce/Ball>>bounce — Bounce@1.0.0, D:\work\Bounce.ns:14 — session 1",
                "events (5 entries, folding 17 lifecycle events; every event, caches included: debug_trace_events {\"session\":1,\"method\":\"Bounce/Ball>>bounce\"})",
                "  #12 Bounce/Ball>>bounce — 4 entries, folding 13 events",
                "    [compile] compiled-twin bytes=2000",
                "    [optimize] optimized key={Bounce/Ball} entered={Bounce/Ball} bytes=900 inlined-scopes=1 inlined-sites=1 type-case-arms=0 eliminated-blocks=0 ×2",
                r##"Showing 3–4 of 5 — next: debug_trace_method {"session":1,"method":"Bounce/Ball>>bounce","part":"events","start":4,"count":2}"##,
            ]
        );
        let last =
            journal(&index, &handle, method, Some(Part::Sends), Some(2), None).expect("a page");
        assert_eq!(
            last,
            "Bounce/Ball>>bounce — Bounce@1.0.0, D:\\work\\Bounce.ns:14 — session 1\nsends (3 entries)\n  #12 (unplaced) send\nShowing 3–3 of 3"
        );
        let head = journal(&index, &handle, method, None, None, Some(2)).expect("a journal");
        let lines: Vec<&str> = head.lines().collect();
        assert_eq!(
            lines[4],
            "events (2 of 5 entries, folding 17 lifecycle events; every event, caches included: debug_trace_events {\"session\":1,\"method\":\"Bounce/Ball>>bounce\"})"
        );
        assert_eq!(
            lines[5],
            "  #12 Bounce/Ball>>bounce — 4 entries, folding 13 events"
        );
        assert_eq!(
            lines[8],
            r##"Showing 1–2 of 5 — next: debug_trace_method {"session":1,"method":"Bounce/Ball>>bounce","part":"events","start":2,"count":2}"##
        );
        assert_eq!(lines[9], "sends (2 of 3)");
        assert_eq!(
            lines[12],
            r##"Showing 1–2 of 3 — next: debug_trace_method {"session":1,"method":"Bounce/Ball>>bounce","part":"sends","start":2,"count":2}"##
        );
        assert_eq!(lines.len(), 13, "{head}");
        // A name of several methods names every root.
        let several =
            journal(&index, &handle, "Bounce/Ball>>...", None, None, None).expect("a journal");
        let lines: Vec<&str> = several.lines().collect();
        assert_eq!(lines[0], "Bounce/Ball>>... — 2 methods — session 1");
        assert_eq!(
            lines[1],
            "bodies: #12 Bounce/Ball>>bounce (line 14), #13 [] in Bounce/Ball>>bounce (line 16), #15 [] in Bounce/Ball>>bounce (line 19), #30 Bounce/Ball>>x (line 31)"
        );
        assert_eq!(
            journal(&index, &handle, method, Some(Part::Sends), Some(3), None),
            Err("start 3 is past the end: sends has 3 entries".to_string())
        );
        assert_eq!(
            journal(&index, &handle, method, None, Some(1), None),
            Err("start pages one part: pass part as well".to_string())
        );
        assert_eq!(
            journal(&index, &handle, method, None, None, Some(0)),
            Err("a count of 0: give 1 or more".to_string())
        );
        assert_eq!(
            journal(&index, &handle, "#9", None, None, None),
            Err("no body of the trace is named #9".to_string())
        );
        assert_eq!(
            journal(&index, &handle, "#15", Some(Part::Events), None, None),
            Ok(
                "[] in Bounce/Ball>>bounce — Bounce@1.0.0, D:\\work\\Bounce.ns:19 — session 1\nevents: none"
                    .to_string()
            )
        );
    }

    /// A block that keeps being compiled, twinned, abandoned, freed twice
    /// and revived -- the round a trace of every category writes -- while
    /// its root's sites fill between the rounds: each body's events fold
    /// apart, the block's fifty rounds into one entry.
    #[test]
    fn each_body_s_rounds_fold_apart() {
        let mut scratch = Scratch::new("journal-rounds");
        scratch.header(&["compile", "code-zone", "caches"], &[]);
        scratch.record(&bounce_body(
            48,
            "Bounce/Tree>>treeAt:insert:",
            None,
            751,
            json!([]),
        ));
        scratch.record(&bounce_body(
            55,
            "[] in Bounce/Tree>>treeAt:insert:",
            Some(48),
            755,
            json!([]),
        ));
        let root = |fields: Value| event_of("compile", Some(48), &fields);
        scratch.record(&root(
            json!({ "event": "compiled", "trigger": "call", "bytes": 12224 }),
        ));
        let block = |fields: Value| event_of("compile", Some(55), &fields);
        for round in 0..50_u64 {
            // Every tenth round is compiled larger and revived by decay.
            let tenth = round % 10 == 9;
            scratch.record(&block(json!({ "event": "compiled", "trigger": "entry",
                        "bytes": if tenth { 6400 } else { 6208 } })));
            scratch.record(&event_of(
                "caches",
                Some(48),
                &json!({ "event": "transition", "before": "vacant", "after": "monomorphic",
                         "selector": "compareTo:", "site": round, "receiver": "CD/Node" }),
            ));
            scratch.record(&block(json!({ "event": "compiled-twin", "bytes": 8384 })));
            scratch.record(&block(json!({ "event": "abandoned", "exits": 64 })));
            for _ in 0..2 {
                scratch.record(&event_of(
                    "code-zone",
                    Some(55),
                    &json!({ "event": "freed", "key": null, "entered": null,
                             "tier": "baseline", "cause": "abandoned" }),
                ));
            }
            scratch.record(&block(json!({
                "event": "revived", "by": if tenth { "decay" } else { "entries" },
            })));
        }
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(index.failure(), None);
        let answer = journal(
            &index,
            &handle,
            "Bounce/Tree>>treeAt:insert:",
            Some(Part::Events),
            None,
            None,
        )
        .expect("a journal");
        assert_eq!(
            answer.lines().collect::<Vec<_>>(),
            [
                r"Bounce/Tree>>treeAt:insert: — Bounce@1.0.0, D:\work\Bounce.ns:752 — session 1",
                r##"events (2 entries, folding 301 lifecycle events; every event, caches included: debug_trace_events {"session":1,"method":"Bounce/Tree>>treeAt:insert:"})"##,
                "  #48 Bounce/Tree>>treeAt:insert: — 1 entry, folding 1 event",
                "    [compile] compiled trigger=call bytes=12224",
                "  #55 [] in Bounce/Tree>>treeAt:insert: — 1 entry, folding 300 events",
                "    [compile] (compiled trigger=entry bytes=6208 → compiled-twin bytes=8384 → abandoned exits=64 → [code-zone] freed tier=baseline cause=abandoned ×2 → revived by=entries) ×50 — compiled bytes 6,208–6,400; revived by=entries ×45, by=decay ×5",
                "Showing 1–2 of 2",
            ],
            "{answer}"
        );
    }

    /// A single repeat whose fields differ folds as a cycle does, its
    /// values summarized, at most three of a field's named.
    #[test]
    fn a_repeat_s_differing_fields_are_summarized() {
        let mut scratch = Scratch::new("journal-repeat");
        scratch.header(&["compile"], &[]);
        scratch.record(&bounce_body(1, "Bounce>>run", None, 0, json!([])));
        for (trigger, bytes) in [
            ("call", 96),
            ("entry", 128),
            ("back-edge", 96),
            ("forced", 96),
            ("call", 100),
        ] {
            scratch.record(&event_of(
                "compile",
                Some(1),
                &json!({ "event": "compiled", "trigger": trigger, "bytes": bytes }),
            ));
        }
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer =
            journal(&index, &handle, "#1", Some(Part::Events), None, None).expect("a journal");
        assert_eq!(
            answer.lines().nth(3),
            Some(
                "    [compile] compiled trigger=call bytes=96 ×5 — compiled trigger=call ×2, trigger=entry ×1, trigger=back-edge ×1, …; compiled bytes 96–128"
            ),
            "{answer}"
        );
    }

    /// A field only some of a step's events carry counts its absence, so
    /// that its counts add up to the step's events.
    #[test]
    fn a_field_some_events_lack_counts_its_absence() {
        let mut scratch = Scratch::new("journal-absent");
        scratch.header(&["compile"], &[]);
        scratch.record(&bounce_body(1, "Bounce>>run", None, 0, json!([])));
        for trigger in ["entry", "entry", "call", "entry"] {
            scratch.record(&event_of(
                "compile",
                Some(1),
                &json!({ "event": "held", "trigger": trigger, "reason": "zone-full" }),
            ));
        }
        // A `held` whose record has no trigger.
        scratch.record(&event_of(
            "compile",
            Some(1),
            &json!({ "event": "held", "reason": "zone-full" }),
        ));
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer =
            journal(&index, &handle, "#1", Some(Part::Events), None, None).expect("a journal");
        assert_eq!(
            answer.lines().nth(3),
            Some(
                "    [compile] held trigger=entry reason=zone-full ×5 — held trigger=entry ×3, trigger=call ×1, absent ×1"
            ),
            "{answer}"
        );
    }

    /// A body an edit compiled is placed in the module's current text,
    /// which no file holds: the journal names its line as that text's,
    /// never as the file's, and a body placed nowhere has no line. A record
    /// without `placed` is placed in its source.
    #[test]
    fn a_body_an_edit_compiled_is_placed_in_its_current_text() {
        let mut scratch = Scratch::new("placed");
        scratch.header(&["compile"], &[]);
        let sends = json!([{ "at": 0, "site": 0, "call": [14, 8, 14, 12], "selector": [],
                             "name": "x:" }]);
        let mut edited = bounce_body(12, "Bounce/Ball>>bounce", None, 13, sends.clone());
        edited["placed"] = json!("current");
        scratch.record(&edited);
        let mut block = bounce_body(13, "[] in Bounce/Ball>>bounce", Some(12), 15, json!([]));
        block["placed"] = json!(null);
        block["range"] = json!(null);
        scratch.record(&block);
        let mut unsaid = bounce_body(30, "Bounce/Ball>>x", None, 30, sends);
        unsaid.as_object_mut().expect("a record").remove("placed");
        scratch.record(&unsaid);
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(index.failure(), None);
        let answer =
            journal(&index, &handle, "Bounce/Ball>>bounce", None, None, None).expect("a journal");
        assert_eq!(
            answer.lines().collect::<Vec<_>>(),
            [
                r"Bounce/Ball>>bounce — Bounce@1.0.0, D:\work\Bounce.ns (edited, line 14 of its current text) — session 1",
                "bodies: #12 method (edited, line 14 of its current text), #13 [] in Bounce/Ball>>bounce",
                "events: none",
                "sends (1 of 1)",
                "  #12 15:9 x: (in its current text)",
            ],
            "{answer}"
        );
        let answer = journal(&index, &handle, "#30", None, None, None).expect("a journal");
        assert_eq!(
            answer.lines().collect::<Vec<_>>(),
            [
                r"Bounce/Ball>>x — Bounce@1.0.0, D:\work\Bounce.ns:31 — session 1",
                "bodies: #30 method (line 31)",
                "events: none",
                "sends (1 of 1)",
                "  #30 15:9 x:",
            ],
            "{answer}"
        );
    }

    /// A body the trace numbers without describing it has a journal of
    /// its number, and its events.
    #[test]
    fn an_undescribed_body_has_a_journal() {
        let mut scratch = Scratch::new("undescribed-journal");
        scratch.header(&["compile"], &[]);
        scratch.record(&compiled(44));
        scratch.record(&compiled(44));
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer = journal(&index, &handle, "#44", None, None, None).expect("a journal");
        assert_eq!(
            answer.lines().collect::<Vec<_>>(),
            [
                "#44 (not described) — session 1",
                "bodies: #44 (not described)",
                "copies of #44: baseline 2",
                "events (1 of 1 entry, folding 2 lifecycle events; every event, caches included: debug_trace_events {\"session\":1,\"method\":\"#44\"})",
                "  #44 (not described) — 1 entry, folding 2 events",
                "    [compile] compiled trigger=call bytes=96 ×2",
                "sends: none",
            ]
        );
    }

    /// However many entries are asked, the journal stays under the
    /// budget, and each part cut by it names the call that goes on.
    #[test]
    fn a_journal_stays_under_its_budget() {
        let mut scratch = Scratch::new("journal-budget");
        scratch.header(&["compile"], &[]);
        let sends: Vec<Value> = (0..300)
            .map(|instruction| {
                json!({ "at": instruction, "site": null, "call": [1, 0, 1, 4], "selector": [],
                        "name": format!("{}{instruction}:", "s".repeat(200)) })
            })
            .collect();
        scratch.record(&bounce_body(1, "Bounce>>run", None, 0, json!(sends)));
        // The number of 1s between consecutive 0s of the Thue–Morse
        // sequence: a word over three shapes with no square in it, which no
        // fold shortens.
        let mut shapes = Vec::new();
        let mut ones = 0;
        let mut place = 1_u32;
        while shapes.len() < 300 {
            if place.count_ones() % 2 == 1 {
                ones += 1;
            } else {
                shapes.push(ones);
                ones = 0;
            }
            place += 1;
        }
        let long = "x".repeat(200);
        for shape in shapes {
            let fields = match shape {
                0 => json!({ "event": "compiled", "trigger": long, "bytes": 1 }),
                1 => json!({ "event": "held", "trigger": long, "reason": long }),
                _ => json!({ "event": "refused", "trigger": long, "reason": long }),
            };
            scratch.record(&event_of("compile", Some(1), &fields));
        }
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer =
            journal(&index, &handle, "Bounce>>run", None, None, Some(200)).expect("a journal");
        assert!(
            characters(&answer) <= ANSWER_BUDGET,
            "{}",
            characters(&answer)
        );
        let showing: Vec<&str> = answer
            .lines()
            .filter(|line| line.starts_with("Showing 1–"))
            .collect();
        assert_eq!(showing.len(), 2, "{answer}");
        assert!(
            showing[0].contains(r#""part":"events","start":"#)
                && showing[1].contains(r#""part":"sends","start":"#),
            "{answer}"
        );
        let page = journal(
            &index,
            &handle,
            "Bounce>>run",
            Some(Part::Sends),
            None,
            Some(200),
        )
        .expect("a page");
        assert!(characters(&page) <= ANSWER_BUDGET, "{}", characters(&page));
        assert!(page.lines().last().unwrap().contains(" — next: "), "{page}");
    }

    /// A field an event's text carries twice -- a value holding ` name=` --
    /// gives more values than there are events, and the summary counts
    /// no absence rather than one below none.
    #[test]
    fn a_field_carried_twice_counts_no_absence() {
        let mut scratch = Scratch::new("journal-twice");
        scratch.header(&["compile"], &[]);
        scratch.record(&bounce_body(1, "Bounce>>run", None, 0, json!([])));
        for trigger in ["call trigger=entry", "entry"] {
            scratch.record(&event_of(
                "compile",
                Some(1),
                &json!({ "event": "held", "trigger": trigger, "reason": "zone-full" }),
            ));
        }
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer =
            journal(&index, &handle, "#1", Some(Part::Events), None, None).expect("a journal");
        let entry = answer.lines().nth(3).unwrap_or_default();
        assert!(
            entry.ends_with("— held trigger=entry ×2, trigger=call ×1"),
            "{answer}"
        );
    }

    /// A file of `trace_listings`' fixtures: `Demo>>run` #1 and its
    /// blocks #2 and #3, `Demo>>square:` #4, their bytecode, two baseline
    /// copies of `square:`, of 16 and 24 bytes of code, an optimized copy
    /// of `run` and its plan; and the code of #44, which no `body` record
    /// describes.
    fn listed() -> Scratch {
        let mut scratch = Scratch::new("listing-window");
        scratch.header(&["compile"], &["bytecode", "code", "inlining"]);
        for (number, name) in NAMES {
            scratch.body(number, name, (number == 2 || number == 3).then_some(1));
        }
        for listing in [
            Listing::Bytecode(run_bytecode()),
            Listing::Bytecode(square_bytecode()),
            Listing::Code(square_code(16)),
            Listing::Code(square_code(24)),
            Listing::Code(run_optimized()),
            Listing::Inlining(run_inlining(InliningOutcome::Installed { bytes: 400 })),
        ] {
            scratch.record(&listing_record(&listing));
        }
        let mut undescribed = square_code(16);
        undescribed.body = BodyName {
            number: 44,
            name: "Demo>>hidden".to_string(),
        };
        undescribed.scopes.clear();
        scratch.record(&listing_record(&Listing::Code(undescribed)));
        scratch
    }

    /// The lines `--print-*` writes for `listing` without its head and
    /// end: the window's.
    fn inner(listing: &Listing) -> Vec<String> {
        let lines = printed(listing);
        lines[1..lines.len() - 1].to_vec()
    }

    /// A window names its listing, holds the lines `--print-*` writes in
    /// the order it writes them, and ends in where it stopped, the next
    /// call naming the listing by its index; the last listing by default.
    #[test]
    fn a_listing_window_names_its_listing_and_pages_it() {
        let scratch = listed();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let window = |method: &str, kind, which, start, count| {
            listing_window(&index, &handle, method, kind, which, start, count)
        };
        let square = inner(&Listing::Code(square_code(24)));
        let architecture = square_code(24).architecture.name();
        assert_eq!(square.len(), 12);
        assert_eq!(
            window("Demo>>square:", ListingKind::Code, None, None, Some(3)),
            Ok([
                format!(
                    "code 2 of 2 of Demo>>square: #4: baseline {architecture}, 32 bytes, 12 lines"
                ),
                square[0].clone(),
                square[1].clone(),
                square[2].clone(),
                "Showing 1–3 of 12 — next: debug_trace_listing {\"session\":1,\"method\":\"Demo>>square:\",\"kind\":\"code\",\"index\":2,\"start\":3,\"count\":3}".to_string(),
            ]
            .join("\n"))
        );
        // The last page, and a count above the maximum capped.
        let last = window(
            "Demo>>square:",
            ListingKind::Code,
            Some(2),
            Some(9),
            Some(600),
        )
        .expect("the last page");
        assert_eq!(
            last.lines().skip(1).collect::<Vec<_>>(),
            [
                square[9].as_str(),
                square[10].as_str(),
                square[11].as_str(),
                "Showing 10–12 of 12"
            ]
        );
        let first = inner(&Listing::Code(square_code(16)));
        let whole = window("#4", ListingKind::Code, Some(1), None, None).expect("the first copy");
        assert_eq!(
            whole.lines().collect::<Vec<_>>(),
            [format!(
                "code 1 of 2 of Demo>>square: #4: baseline {architecture}, 24 bytes, 10 lines"
            )
            .as_str()]
            .into_iter()
            .chain(first.iter().map(String::as_str))
            .chain(["Showing 1–10 of 10"])
            .collect::<Vec<_>>()
        );
        // An optimized copy's position lines take their instructions from
        // the scopes' bytecode.
        let optimized = window("Demo>>run", ListingKind::Code, None, None, None).expect("run");
        assert!(
            optimized.starts_with(&format!(
                "code 1 of 1 of Demo>>run #1: optimized {architecture} key={{Demo}} entered={{Demo/Base}}, 32 bytes, "
            )),
            "{optimized}"
        );
        assert_eq!(
            optimized.lines().skip(1).take(4).collect::<Vec<_>>(),
            inner(&Listing::Code(run_optimized()))[..4]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert!(
            optimized
                .lines()
                .any(|line| line == "  s1 {Demo>>square:} @1  stmt 0  Multiply r1 <- r0, r0"),
            "{optimized}"
        );
        // A method's bytecode is its root's record, its blocks within.
        let bytecode = inner(&Listing::Bytecode(run_bytecode()));
        assert_eq!(
            window("Demo>>run", ListingKind::Bytecode, None, None, None),
            Ok([format!(
                "bytecode 1 of 1 of Demo>>run #1: 3 bodies, {} lines",
                bytecode.len()
            )]
            .into_iter()
            .chain(bytecode.iter().cloned())
            .chain([format!("Showing 1–{0} of {0}", bytecode.len())])
            .collect::<Vec<_>>()
            .join("\n"))
        );
        let plan = inner(&Listing::Inlining(run_inlining(
            InliningOutcome::Installed { bytes: 400 },
        )));
        let inlining = window("#1", ListingKind::Inlining, None, None, None).expect("a plan");
        assert_eq!(
            inlining.lines().next(),
            Some(
                format!(
                    "inlining 1 of 1 of Demo>>run #1: key={{Demo}}, installed 400 bytes, {} lines",
                    plan.len()
                )
                .as_str()
            )
        );
        assert_eq!(
            inlining
                .lines()
                .skip(1)
                .take(plan.len())
                .collect::<Vec<_>>(),
            plan.iter().map(String::as_str).collect::<Vec<_>>()
        );
        // A body no record describes is named by its number.
        let hidden = window("#44", ListingKind::Code, None, None, Some(1)).expect("#44");
        assert!(
            hidden.starts_with(&format!(
                "code 1 of 1 of #44 (not described): baseline {architecture}, 24 bytes, "
            )),
            "{hidden}"
        );
    }

    /// What a window refuses: an index or a start past the end, a count
    /// of 0, and a kind of listing the method has none of.
    #[test]
    fn a_listing_window_refuses_what_it_cannot_show() {
        let scratch = listed();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let window = |method: &str, kind, which, start, count| {
            listing_window(&index, &handle, method, kind, which, start, count)
        };
        assert_eq!(
            window("Demo>>square:", ListingKind::Code, Some(3), None, None),
            Err("index 3 is past the end: Demo>>square: #4 has 2 code listings".to_string())
        );
        assert_eq!(
            window("Demo>>square:", ListingKind::Code, Some(0), None, None),
            Err("index 0: a body's listings count from 1".to_string())
        );
        assert_eq!(
            window("Demo>>square:", ListingKind::Code, None, Some(12), None),
            Err("start 12 is past the end: the listing has 12 lines".to_string())
        );
        assert_eq!(
            window("Demo>>square:", ListingKind::Code, None, None, Some(0)),
            Err("a count of 0: give 1 or more".to_string())
        );
        assert_eq!(
            window("Demo>>square:", ListingKind::Inlining, None, None, None),
            Err("no inlining listing of Demo>>square:".to_string())
        );
    }

    /// A session that lists no machine code says so, and how to list it.
    #[test]
    fn a_session_without_code_listings_says_so() {
        let mut scratch = Scratch::new("listing-unlisted");
        scratch.header(&["compile"], &["bytecode"]);
        scratch.body(4, "Demo>>square:", None);
        scratch.record(&listing_record(&Listing::Bytecode(square_bytecode())));
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(
            listing_window(&index, &handle, "Demo>>square:", ListingKind::Code, None, None, None),
            Err("no code listing of Demo>>square: — the session lists no code: debug_start with listings [\"code\"] lists it".to_string())
        );
    }

    /// A name of several bodies with listings of the kind is asked back,
    /// each body with its number, what it is and its listings' indexes,
    /// with the call that answers the first; for bytecode, a root and its
    /// blocks are one record.
    #[test]
    fn a_name_of_several_bodies_is_asked_back() {
        let mut scratch = listed();
        // `run`'s block #2 with code, and `run` edited, #5, with two copies.
        let mut block = square_code(16);
        block.body = BodyName {
            number: 2,
            name: "[] in Demo>>run".to_string(),
        };
        scratch.record(&listing_record(&Listing::Code(block)));
        scratch.body(5, "Demo>>run", None);
        let mut edited = run_bytecode();
        edited.body.number = 5;
        edited.bodies.truncate(1);
        edited.bodies[0].body.number = 5;
        scratch.record(&listing_record(&Listing::Bytecode(edited)));
        for _ in 0..2 {
            let mut copy = square_code(16);
            copy.body = BodyName {
                number: 5,
                name: "Demo>>run".to_string(),
            };
            scratch.record(&listing_record(&Listing::Code(copy)));
        }
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(
            listing_window(&index, &handle, "Demo>>run", ListingKind::Code, None, None, Some(5)),
            Err("Demo>>run matches 3 bodies: #1 method (code 1), #2 [] in Demo>>run (code 1), #5 Demo>>run, an edited version (code 1–2) — pass one: debug_trace_listing {\"session\":1,\"method\":\"#1\",\"kind\":\"code\",\"count\":5}".to_string())
        );
        assert_eq!(
            listing_window(&index, &handle, "Demo>>run", ListingKind::Bytecode, None, None, None),
            Err("Demo>>run matches 2 bodies: #1 method (bytecode 1), #5 Demo>>run, an edited version (bytecode 1) — pass one: debug_trace_listing {\"session\":1,\"method\":\"#1\",\"kind\":\"bytecode\"}".to_string())
        );
    }

    /// A window of the most lines over long ones stays within the answer
    /// budget, and says where it stopped with the call that goes on.
    #[test]
    fn a_listing_window_stays_under_its_budget() {
        let mut scratch = Scratch::new("listing-budget");
        scratch.header(&["compile"], &["code"]);
        scratch.body(4, "Demo>>square:", None);
        let instructions: Vec<Value> = (0..1_000)
            .map(|offset| {
                json!({ "offset": offset * 4, "text": format!("mov rax,{}", "x".repeat(100)),
                        "scope": 0, "at": 0, "out-of-line": false,
                        "annotation": "leaf miss" })
            })
            .collect();
        scratch.record(&json!({
            "kind": "code", "body": 4, "tier": "baseline", "key": null, "entered": null,
            "architecture": "x64", "base": 4096, "bytes": 4000,
            "instructions": instructions, "data": [], "scopes": [], "mark": null,
        }));
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer = listing_window(
            &index,
            &handle,
            "Demo>>square:",
            ListingKind::Code,
            None,
            None,
            Some(500),
        )
        .expect("a window");
        assert!(
            characters(&answer) <= ANSWER_BUDGET,
            "{}",
            characters(&answer)
        );
        let lines: Vec<&str> = answer.lines().collect();
        // The head, the position line and the instructions that fit.
        let shown = lines.len() - 2;
        assert!(shown < 500, "{shown} lines");
        assert_eq!(
            lines.last().copied(),
            Some(
                format!(
                    "Showing 1–{shown} of 1,001 — next: debug_trace_listing {{\"session\":1,\"method\":\"Demo>>square:\",\"kind\":\"code\",\"index\":1,\"start\":{shown},\"count\":500}}"
                )
                .as_str()
            )
        );
    }

    /// A trace of every category but `osr` and `optimize`: `Demo>>run`
    /// (#1) and its block (#2), `Demo>>square:` (#3), and an undescribed
    /// callee (#20), nine events among them, one about no body.
    fn events_trace() -> Scratch {
        let mut scratch = Scratch::new("events");
        scratch.header(
            &["compile", "inlining", "deoptimize", "code-zone", "caches"],
            &[],
        );
        scratch.body(1, "Demo>>run", None);
        scratch.body(2, "[] in Demo>>run", Some(1));
        scratch.body(3, "Demo>>square:", None);
        let compile = |body: u32, trigger: &str, bytes: u64| {
            event_of(
                "compile",
                Some(body),
                &json!({ "event": "compiled", "trigger": trigger, "bytes": bytes }),
            )
        };
        scratch.record(&compile(1, "call", 96));
        scratch.record(&compile(3, "call", 64));
        scratch.record(&event_of(
            "inlining",
            Some(1),
            &json!({ "event": "declined", "key": "Demo", "entered": null, "in-body": 1,
                     "callee": 20, "reason": "over-weight", "at": 4 }),
        ));
        scratch.record(&event_of(
            "inlining",
            Some(3),
            &json!({ "event": "inlined", "key": "Demo", "entered": null, "in-body": 3,
                     "callee": 2, "at": 0 }),
        ));
        scratch.record(&event_of(
            "inlining",
            Some(1),
            &json!({ "event": "declined-no-body", "key": "Demo", "entered": null, "count": 2,
                     "reasons": [{ "reason": "not-snapshotted", "count": 2 }] }),
        ));
        scratch.record(&event_of(
            "deoptimize",
            Some(1),
            &json!({ "event": "invalidated", "key": "Demo", "entered": null, "cause": "edit" }),
        ));
        scratch.record(&event_of(
            "code-zone",
            None,
            &json!({ "event": "compacted", "freeing": "no-room", "bodies-freed": 1 }),
        ));
        scratch.record(&event_of(
            "caches",
            Some(2),
            &json!({ "event": "transition", "before": "monomorphic", "after": "polymorphic",
                     "selector": "x", "site": 0, "receiver": "Demo" }),
        ));
        scratch.record(&compile(2, "entry", 32));
        scratch
    }

    /// The lines of `events_trace`, by their place.
    const EVENT_LINES: [&str; 9] = [
        "  @1 [compile] {Demo>>run #1} compiled trigger=call bytes=96",
        "  @2 [compile] {Demo>>square: #3} compiled trigger=call bytes=64",
        "  @3 [inlining] {Demo>>run #1} declined key={Demo} in-body={Demo>>run #1} callee={#20 (not described)} reason=over-weight at=4",
        "  @4 [inlining] {Demo>>square: #3} inlined key={Demo} in-body={Demo>>square: #3} callee={[] in Demo>>run #2} at=0",
        "  @5 [inlining] {Demo>>run #1} declined-no-body key={Demo} count=2 reasons=not-snapshotted:2",
        "  @6 [deoptimize] {Demo>>run #1} invalidated key={Demo} cause=edit",
        "  @7 [code-zone] {} compacted freeing=no-room bodies-freed=1",
        "  @8 [caches] {[] in Demo>>run #2} monomorphic->polymorphic selector=#x site=0 receiver={Demo}",
        "  @9 [compile] {[] in Demo>>run #2} compiled trigger=entry bytes=32",
    ];

    /// The answer of `debug_trace_events` with the filters given, from
    /// `start`, `count` at most.
    fn queried(
        index: &TraceIndex,
        handle: &SessionHandle,
        filters: [Option<&str>; 4],
        start: Option<usize>,
        count: Option<usize>,
    ) -> Answer {
        let [category, event, reason, method] = filters;
        let query = EventQuery::new(index, category, event, reason, method)?;
        events(index, handle, &query, start, count)
    }

    /// The whole answer of the filters given: its head, the places of its
    /// lines, and its `Showing` line, each line checked against
    /// `EVENT_LINES`.
    fn selected_places(
        index: &TraceIndex,
        handle: &SessionHandle,
        filters: [Option<&str>; 4],
    ) -> (String, Vec<usize>, String) {
        let answer = queried(index, handle, filters, None, None).expect("events");
        let lines: Vec<&str> = answer.lines().collect();
        let places = lines[1..lines.len() - 1]
            .iter()
            .map(|line| {
                let place = EVENT_LINES
                    .iter()
                    .position(|known| known == line)
                    .unwrap_or_else(|| panic!("{line:?} is no line of the trace:\n{answer}"));
                place + 1
            })
            .collect();
        (
            lines[0].to_string(),
            places,
            lines[lines.len() - 1].to_string(),
        )
    }

    /// A query's filters, the head its answer begins with, and the places
    /// of the events it answers.
    type Case<'case> = ([Option<&'case str>; 4], &'case str, &'case [usize]);

    #[test]
    fn the_events_are_selected_by_each_filter_alone_and_together() {
        let scratch = events_trace();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(index.failure(), None);
        let every = queried(&index, &handle, [None; 4], None, None).expect("events");
        let mut expected = vec!["events of session 1 (9 of 9)"];
        expected.extend(EVENT_LINES);
        expected.push("Showing 1–9 of 9");
        assert_eq!(every.lines().collect::<Vec<_>>(), expected, "{every}");
        let cases: [Case; 16] = [
            (
                [Some("compile"), None, None, None],
                "events of session 1, category=compile (3 of 3)",
                &[1, 2, 9],
            ),
            (
                [Some("compile,caches"), None, None, None],
                "events of session 1, category=compile,caches (4 of 4)",
                &[1, 2, 8, 9],
            ),
            (
                [Some("deoptimize"), None, None, None],
                "events of session 1, category=deoptimize (1 of 1)",
                &[6],
            ),
            (
                [Some("all"), None, None, None],
                "events of session 1, category=all (9 of 9); not traced: osr, optimize",
                &[1, 2, 3, 4, 5, 6, 7, 8, 9],
            ),
            // The record's word and the line's.
            (
                [None, Some("declined"), None, None],
                "events of session 1, event=declined (1 of 1)",
                &[3],
            ),
            (
                [None, Some("transition"), None, None],
                "events of session 1, event=transition (1 of 1)",
                &[8],
            ),
            (
                [None, Some("monomorphic->polymorphic"), None, None],
                "events of session 1, event=monomorphic->polymorphic (1 of 1)",
                &[8],
            ),
            // A reason, a cause, and one of `declined-no-body`'s reasons.
            (
                [None, None, Some("over-weight"), None],
                "events of session 1, reason=over-weight (1 of 1)",
                &[3],
            ),
            (
                [None, None, Some("edit"), None],
                "events of session 1, reason=edit (1 of 1)",
                &[6],
            ),
            (
                [None, None, Some("not-snapshotted"), None],
                "events of session 1, reason=not-snapshotted (1 of 1)",
                &[5],
            ),
            // A body field names the body as its own body does.
            (
                [None, None, None, Some("#20")],
                "events of session 1, method=#20 (1 of 1)",
                &[3],
            ),
            (
                [None, None, None, Some("#2")],
                "events of session 1, method=#2 (3 of 3)",
                &[4, 8, 9],
            ),
            // A method's name names its blocks, a prefix every match.
            (
                [None, None, None, Some("Demo>>run")],
                "events of session 1, method=Demo>>run (7 of 7)",
                &[1, 3, 4, 5, 6, 8, 9],
            ),
            (
                [None, None, None, Some("Demo>>sq...")],
                "events of session 1, method=Demo>>sq... (2 of 2)",
                &[2, 4],
            ),
            (
                [Some("inlining"), None, None, Some("Demo>>run")],
                "events of session 1, category=inlining method=Demo>>run (3 of 3)",
                &[3, 4, 5],
            ),
            (
                [
                    Some("inlining,deoptimize"),
                    Some("declined"),
                    Some("over-weight"),
                    Some("#1"),
                ],
                "events of session 1, category=inlining,deoptimize event=declined reason=over-weight method=#1 (1 of 1)",
                &[3],
            ),
        ];
        for (filters, head, places) in cases {
            let (answered_head, answered, showing) = selected_places(&index, &handle, filters);
            assert_eq!(answered_head, head, "{filters:?}");
            assert_eq!(answered, places, "{filters:?}");
            assert_eq!(
                showing,
                format!("Showing 1–{0} of {0}", places.len()),
                "{filters:?}"
            );
        }
        // A filter no event meets, and a category the run does not trace.
        assert_eq!(
            queried(
                &index,
                &handle,
                [None, None, Some("nothing"), None],
                None,
                None
            ),
            Ok("events of session 1, reason=nothing: none".to_string())
        );
        assert_eq!(
            queried(
                &index,
                &handle,
                [None, Some("compiled"), None, Some("#20")],
                None,
                None
            ),
            Ok("events of session 1, event=compiled method=#20: none".to_string())
        );
        assert_eq!(
            queried(
                &index,
                &handle,
                [Some("osr,compile"), None, None, Some("#3")],
                None,
                None
            )
            .expect("events")
            .lines()
            .next(),
            Some("events of session 1, category=osr,compile method=#3 (1 of 1); not traced: osr")
        );
        assert_eq!(
            queried(&index, &handle, [Some("osr"), None, None, None], None, None),
            Ok("events of session 1, category=osr: none; not traced: osr".to_string())
        );
    }

    #[test]
    fn the_events_are_paged_with_their_exact_next() {
        let scratch = events_trace();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let page = queried(&index, &handle, [None; 4], None, Some(2)).expect("events");
        assert_eq!(
            page.lines().collect::<Vec<_>>(),
            [
                "events of session 1 (2 of 9)",
                EVENT_LINES[0],
                EVENT_LINES[1],
                r#"Showing 1–2 of 9 — next: debug_trace_events {"session":1,"start":2,"count":2}"#,
            ],
            "{page}"
        );
        // Every filter given is repeated, in the signature's order.
        let page = queried(
            &index,
            &handle,
            [Some("compile,caches"), None, None, Some("Demo>>run")],
            Some(1),
            Some(1),
        )
        .expect("events");
        assert_eq!(
            page.lines().collect::<Vec<_>>(),
            [
                "events of session 1, category=compile,caches method=Demo>>run (1 of 3)",
                EVENT_LINES[7],
                r#"Showing 2–2 of 3 — next: debug_trace_events {"session":1,"category":"compile,caches","method":"Demo>>run","start":2,"count":1}"#,
            ],
            "{page}"
        );
        // The last page names no next; a count past the maximum is capped.
        let last = queried(&index, &handle, [None; 4], Some(8), Some(500)).expect("events");
        assert_eq!(
            last.lines().collect::<Vec<_>>(),
            [
                "events of session 1 (1 of 9)",
                EVENT_LINES[8],
                "Showing 9–9 of 9"
            ],
            "{last}"
        );
    }

    #[test]
    fn the_events_refuse_what_they_cannot_answer() {
        let scratch = events_trace();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        assert_eq!(
            queried(&index, &handle, [Some("nothing"), None, None, None], None, None),
            Err("category: no category `nothing`; the categories are compile, osr, optimize, inlining, deoptimize, code-zone, caches, or all".to_string())
        );
        assert_eq!(
            queried(&index, &handle, [None, None, None, Some("#99")], None, None),
            Err("no body of the trace is named #99".to_string())
        );
        // A name of no module is refused as every tool refuses it.
        let refused = queried(
            &index,
            &handle,
            [None, None, None, Some("Nope>>x")],
            None,
            None,
        )
        .expect_err("a refusal");
        assert_eq!(Err(refused), index.bodies_named("Nope>>x").map(|_| ()));
        assert_eq!(
            queried(&index, &handle, [None; 4], None, Some(0)),
            Err("a count of 0: give 1 or more".to_string())
        );
        assert_eq!(
            queried(&index, &handle, [None; 4], Some(9), None),
            Err("start 9 is past the end: 9 events match".to_string())
        );
        assert_eq!(
            queried(
                &index,
                &handle,
                [Some("deoptimize"), None, None, None],
                Some(1),
                None
            ),
            Err("start 1 is past the end: 1 event matches".to_string())
        );
    }

    /// The overview's call for a declined callee answers its declines.
    #[test]
    fn the_overview_s_call_finds_a_callee_s_declines() {
        let scratch = written();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer = queried(
            &index,
            &handle,
            [None, Some("declined"), None, Some("#20")],
            None,
            None,
        )
        .expect("events");
        assert_eq!(
            answer.lines().collect::<Vec<_>>(),
            [
                "events of session 1, event=declined method=#20 (3 of 3)",
                "  @85 [inlining] {Bounce>>m1 #1} declined key={Bounce} in-body={Bounce>>m1 #1} callee={#20 (not described)} reason=over-weight at=0",
                "  @86 [inlining] {Bounce>>m2 #2} declined key={Bounce} in-body={Bounce>>m2 #2} callee={#20 (not described)} reason=over-weight at=0",
                "  @87 [inlining] {Bounce>>m2 #2} declined key={Bounce} in-body={Bounce>>m2 #2} callee={#20 (not described)} reason=block-escapes at=0",
                "Showing 1–3 of 3",
            ],
            "{answer}"
        );
    }

    /// The call a journal's head names answers the journal's bodies'
    /// events -- the method's root and its blocks, and every event naming
    /// one in a field -- caches included, in the run-wide order.
    #[test]
    fn the_journal_s_call_answers_its_bodies_events() {
        let scratch = bounce_journal();
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let journal =
            journal(&index, &handle, "Bounce/Ball>>bounce", None, None, None).expect("a journal");
        let call = journal
            .lines()
            .find_map(|line| line.split_once("every event, caches included: "))
            .and_then(|(_, call)| call.strip_suffix(')'))
            .unwrap_or_else(|| panic!("no call:\n{journal}"));
        let arguments = call
            .strip_prefix("debug_trace_events ")
            .unwrap_or_else(|| panic!("{call} is no call of debug_trace_events"));
        let arguments: EventArguments =
            parse(serde_json::from_str(arguments).expect("JSON")).expect("the tool's arguments");
        let query = EventQuery::new(
            &index,
            arguments.category.as_deref(),
            arguments.event.as_deref(),
            arguments.reason.as_deref(),
            arguments.method.as_deref(),
        )
        .expect("a query");
        let answer =
            events(&index, &handle, &query, arguments.start, arguments.count).expect("events");
        let lines: Vec<&str> = answer.lines().collect();
        assert_eq!(
            lines[0], "events of session 1, method=Bounce/Ball>>bounce (21 of 21)",
            "{answer}"
        );
        let places: Vec<&str> = lines[1..lines.len() - 1]
            .iter()
            .map(|line| {
                line.strip_prefix("  @")
                    .and_then(|line| line.split_once(' '))
                    .map_or("", |(place, _)| place)
            })
            .collect();
        let expected: Vec<String> = (1..=20)
            .chain([22])
            .map(|place| place.to_string())
            .collect();
        assert_eq!(places, expected, "{answer}");
        assert_eq!(
            lines[1],
            "  @1 [compile] {Bounce/Ball>>bounce #12} compiled trigger=call bytes=1216"
        );
        assert_eq!(
            lines[12],
            "  @12 [osr] {[] in Bounce/Ball>>bounce #13} osr-entered at=3"
        );
        assert_eq!(
            lines[16],
            "  @16 [inlining] {Bounce/Ball>>bounce #12} inlined key={Bounce/Ball} entered={Bounce/Ball} in-body={Bounce/Ball>>bounce #12} callee={Bounce/Ball>>x #30} at=0"
        );
        // Another method's compile, by its callee field.
        assert_eq!(
            lines[20],
            "  @20 [inlining] {Bounce>>run #4} declined key={Bounce} in-body={Bounce>>run #4} callee={Bounce/Ball>>bounce #12} reason=over-weight at=5"
        );
        assert_eq!(
            lines[21],
            "  @22 [caches] {Bounce/Ball>>bounce #12} vacant->monomorphic selector=#x: site=0 receiver={Bounce/Ball}"
        );
        assert_eq!(lines[22], "Showing 1–21 of 21");
    }

    /// A page of the most events of long lines stays within the budget,
    /// cut before its count, numbered past a thousand, and goes on where
    /// it stopped.
    #[test]
    fn the_events_stay_under_their_budget() {
        let mut scratch = Scratch::new("events-budget");
        scratch.header(&["inlining"], &[]);
        let name = format!("Demo/{}>>run", "Inner".repeat(16));
        scratch.body(1, &name, None);
        for _ in 0..1_500 {
            scratch.record(&event_of(
                "inlining",
                Some(1),
                &json!({ "event": "declined", "key": "Demo", "entered": null, "in-body": 1,
                         "callee": 1, "reason": "over-weight", "at": 4 }),
            ));
        }
        let (_registry, handle) = stopped_session();
        let index = scratch.caught_up();
        let answer = queried(&index, &handle, [None; 4], Some(1_000), Some(200)).expect("events");
        assert!(
            characters(&answer) <= ANSWER_BUDGET,
            "{}",
            characters(&answer)
        );
        let lines: Vec<&str> = answer.lines().collect();
        let shown = lines.len() - 2;
        assert!(shown < 200, "{shown} events");
        let named = format!("{{{name} #1}}");
        assert_eq!(
            lines[1],
            format!(
                "  @1,001 [inlining] {named} declined key={{Demo}} in-body={named} callee={named} reason=over-weight at=4"
            )
        );
        assert_eq!(lines[0], format!("events of session 1 ({shown} of 1,500)"));
        assert_eq!(
            lines[lines.len() - 1],
            format!(
                "Showing 1,001–{} of 1,500 — next: debug_trace_events {{\"session\":1,\"start\":{},\"count\":200}}",
                thousands((1_000 + shown) as u64),
                1_000 + shown
            )
        );
        // The page stopped at the line that would have passed the budget,
        // the head and the `Showing` line counted as long as they can be.
        let longest_head = "events of session 1 (1,500 of 1,500)";
        let longest_showing = showing_line(
            1_500,
            1_500,
            1_500,
            Some((
                "debug_trace_events",
                json!({ "session": 1, "start": 1_500, "count": 200 }),
            )),
        );
        let counted_as = characters(&answer) + characters(longest_head) - characters(lines[0])
            + characters(&longest_showing)
            - characters(lines[lines.len() - 1]);
        let next_line =
            lines[1].replace("@1,001", &format!("@{}", thousands((1_001 + shown) as u64)));
        assert!(counted_as <= ANSWER_BUDGET, "{counted_as}");
        assert!(
            counted_as + characters(&next_line) + 1 > ANSWER_BUDGET,
            "{counted_as}"
        );
    }
}
