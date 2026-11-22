//! The trace tools' answers (`docs/design/debugger.md`, "The MCP server",
//! "The trace tools"), one function each, every one rendered as text from
//! a session's `TraceIndex` and bounded by `pages.rs`: `overview`, the
//! run-wide lists an agent starts from, and `journal`, one method's
//! bodies, copies, events and sends. Each tool runs on a thread of its
//! own and locks the index alone, never waiting on the session's worker,
//! so that the trace of a program that runs, or of a wait in progress, can
//! be asked.

use std::collections::HashMap;

use ns_runtime::TraceCategory;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pages::{
    ANSWER_BUDGET, ENTRY_DEFAULT, ENTRY_MAXIMUM, OVERVIEW_DEFAULT, call_text, characters, count_of,
    counted, showing_line, size, thousands,
};
use super::registry::SessionHandle;
use super::tools::{Answer, parse};
use super::trace_index::{BodySource, CopyKey, ListingKind, TraceIndex};

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
    matches!(name, "debug_trace" | "debug_trace_method")
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
        _ => Err(format!("no tool is named {name}")),
    }
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
            if let Some(range) = described.range {
                place.push_str(&format!(":{}", range[0] + 1));
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
                if let Some(range) = described.range {
                    item.push_str(&format!(" (line {})", range[0] + 1));
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
/// by=decay ×34`, at most `VARIANTS_SHOWN` of them, then `…`.
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
            let place = send.call.map_or_else(
                || "(unplaced)".to_string(),
                |call| format!("{}:{}", call[0] + 1, call[1] + 1),
            );
            let name = send.name.as_deref().unwrap_or("send");
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

#[cfg(test)]
mod tests {
    use super::super::registry::{Registry, SessionState};
    use super::super::trace_index::tests::{Scratch, event_of};
    use super::*;

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
}
