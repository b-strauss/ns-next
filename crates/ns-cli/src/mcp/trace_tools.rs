//! The trace tools' answers (`docs/design/debugger.md`, "The MCP server",
//! "The trace tools"), one function each, every one rendered as text from
//! a session's `TraceIndex` and bounded by `pages.rs`: `overview`, the
//! run-wide lists an agent starts from. Each tool runs on a thread of its
//! own and locks the index alone, never waiting on the session's worker,
//! so that the trace of a program that runs, or of a wait in progress, can
//! be asked.

use ns_runtime::TraceCategory;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pages::{
    ANSWER_BUDGET, ENTRY_MAXIMUM, OVERVIEW_DEFAULT, call_text, characters, count_of, counted,
    showing_line, size, thousands,
};
use super::registry::SessionHandle;
use super::tools::{Answer, parse};
use super::trace_index::{CopyKey, ListingKind, TraceIndex};

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
    name == "debug_trace"
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
        _ => Err(format!("no tool is named {name}")),
    }
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
}
