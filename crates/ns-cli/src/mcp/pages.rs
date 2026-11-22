//! What every paged answer shares, the trace tools', `debug_code`'s,
//! `debug_stack`'s and `debug_variables`' (`docs/design/debugger.md`,
//! "The MCP server"): how many entries and
//! lines a page or window holds by default and at most, the character
//! budget every answer stays under, the `Showing` line that ends a page
//! and the exact `next:` call in it, and how numbers and sizes are
//! written.

use serde_json::Value;

/// The most characters an answer holds: about 10,000 tokens even at a
/// pessimistic two characters a token -- the density of machine code's
/// hex and registers is not measured, so the budget assumes the worst
/// plausible one -- and so at or below Claude Code's warning, never near
/// its cut. A page stops at the entry that would pass it.
pub(crate) const ANSWER_BUDGET: usize = 20_000;

/// The entries of a journal's, the events' or the stack's page by default.
pub(crate) const ENTRY_DEFAULT: usize = 50;

/// The entries of each of the overview's lists by default.
pub(crate) const OVERVIEW_DEFAULT: usize = 10;

/// The lines of a listing's or a frame's code's window by default.
pub(crate) const LINE_DEFAULT: usize = 150;

/// The most entries a page holds, whatever it is asked.
pub(crate) const ENTRY_MAXIMUM: usize = 200;

/// The most lines a window holds, whatever it is asked.
pub(crate) const LINE_MAXIMUM: usize = 500;

/// How many entries or lines a call asked for: `default` when it asked
/// none, `maximum` when it asked more; a count of 0 is refused, since a
/// page of nothing has no next.
pub(crate) fn count_of(
    asked: Option<usize>,
    default: usize,
    maximum: usize,
) -> Result<usize, String> {
    match asked {
        None => Ok(default),
        Some(0) => Err("a count of 0: give 1 or more".to_string()),
        Some(count) => Ok(count.min(maximum)),
    }
}

/// `Showing <first>–<last> of <total>`, counted from 1, and where more
/// follow, ` — next: <tool> <arguments>`, the exact call that answers
/// them.
pub(crate) fn showing_line(
    first: usize,
    last: usize,
    total: usize,
    next: Option<(&str, Value)>,
) -> String {
    let mut line = format!(
        "Showing {}–{} of {}",
        thousands(first as u64),
        thousands(last as u64),
        thousands(total as u64)
    );
    if let Some((tool, arguments)) = next {
        line.push_str(" — next: ");
        line.push_str(&call_text(tool, &arguments));
    }
    line
}

/// The arguments of the call that answers what follows a page: every
/// argument `given` holds, `session` set to the session's number, which
/// the call names even where it was left out, and `start` advanced to
/// `start`, so that the next page is asked as the page was.
pub(crate) fn next_arguments(given: &Value, session: u32, start: usize) -> Value {
    let mut arguments = match given {
        Value::Object(members) => Value::Object(members.clone()),
        _ => Value::Object(serde_json::Map::new()),
    };
    arguments["session"] = Value::from(session);
    arguments["start"] = Value::from(start);
    arguments
}

/// A page of `entries`, the entries from `start` of `total`, as many as
/// keep the page and its `Showing` line within `budget` characters, a
/// line's newline counted; a page that is not the whole ends in its
/// `Showing` line, naming the call `next_from` makes of the first entry
/// it leaves out where more follow.
pub(crate) fn page<'a>(
    entries: impl IntoIterator<Item = &'a String>,
    start: usize,
    total: usize,
    budget: usize,
    next_from: impl Fn(usize) -> (&'static str, Value),
) -> Vec<String> {
    let showing_at_most =
        characters(&showing_line(total, total, total, Some(next_from(total)))) + 1;
    let mut used = 0;
    let mut lines = Vec::new();
    let mut last = start;
    for entry in entries {
        let room = budget.saturating_sub(used + showing_at_most);
        let Some(taken) = taken_within(entry, room, lines.is_empty(), None) else {
            break;
        };
        used += characters(&taken) + 1;
        lines.push(taken);
        last += 1;
    }
    if start > 0 || last < total {
        let next = (last < total).then(|| next_from(last));
        lines.push(showing_line(start + 1, last, total, next));
    }
    lines
}

/// The fewest characters a page's first entry is cut to: below it, the
/// page takes nothing and leaves the entry to the call that pages on,
/// whose page it begins with all the room an answer has.
pub(crate) const CUT_MINIMUM: usize = 200;

/// `entry` as a page takes it within `room` characters, its newline
/// counted: whole where it fits; where it does not but is the page's
/// `first`, cut to the room by `cut_line` -- `hint` saying where it is
/// whole -- so that a page always gets past it and the call that pages on
/// never asks the page again; and none otherwise, the page ending before
/// it.
pub(crate) fn taken_within(
    entry: &str,
    room: usize,
    first: bool,
    hint: Option<&str>,
) -> Option<String> {
    let length = characters(entry) + 1;
    if length <= room {
        return Some(entry.to_string());
    }
    (first && room > CUT_MINIMUM).then(|| cut_line(entry, room - 1, hint))
}

/// `line` cut to `width` characters at most: its start, then `… (<n>
/// characters left out)`, `hint` -- how to see the line whole, where a
/// tool has a way -- after a `; ` inside the parentheses; the line itself
/// where it is no longer than `width`. A width too narrow for the marker
/// keeps none of the line and answers the marker alone.
pub(crate) fn cut_line(line: &str, width: usize, hint: Option<&str>) -> String {
    let length = characters(line);
    if length <= width {
        return line.to_string();
    }
    let marker = |left_out: usize| match hint {
        Some(hint) => format!(
            "… ({} characters left out; {hint})",
            thousands(left_out as u64)
        ),
        None => format!("… ({} characters left out)", thousands(left_out as u64)),
    };
    // The marker with every character counted out is at least as long as
    // the one the cut writes, so the cut stays within `width`.
    let kept = width.saturating_sub(characters(&marker(length)));
    let mut cut: String = line.chars().take(kept).collect();
    cut.push_str(&marker(length - kept));
    cut
}

/// Cuts the longest of `lines` until they and their newlines hold at most
/// `budget` characters, each cut by `cut_line` with `hint` and to no fewer
/// than `CUT_MINIMUM`: what holds an answer to the budget whose lines are
/// each bounded but could together pass it.
pub(crate) fn fit_lines(lines: &mut [String], budget: usize, hint: Option<&str>) {
    loop {
        let used: usize = lines.iter().map(|line| characters(line) + 1).sum();
        if used <= budget + 1 {
            return;
        }
        let Some(longest) = (0..lines.len()).max_by_key(|position| characters(&lines[*position]))
        else {
            return;
        };
        let length = characters(&lines[longest]);
        let width = length.saturating_sub(used - budget - 1).max(CUT_MINIMUM);
        if width >= length {
            return;
        }
        lines[longest] = cut_line(&lines[longest], width, hint);
    }
}

/// What each tool an answer names a call of is asked about, in the order
/// its signature gives its parameters: between `session`, which comes
/// first, and `start` and `count`, which come last.
const PARAMETER_ORDERS: [(&str, &[&str]); 6] = [
    ("debug_code", &["frame"]),
    ("debug_variables", &["frame", "reference"]),
    ("debug_trace", &["list"]),
    ("debug_trace_method", &["method", "part"]),
    ("debug_trace_listing", &["method", "kind", "index"]),
    (
        "debug_trace_events",
        &["category", "event", "reason", "method"],
    ),
];

/// The parameters every call names first and last.
const FIRST: &str = "session";
const LAST: [&str; 2] = ["start", "count"];

/// The call of `tool` with `arguments` as an answer names it: the tool's
/// name and its arguments as one JSON object: `session` first, then what
/// the call is about in the order of the tool's parameters
/// (`PARAMETER_ORDERS`), an argument of no listed parameter after them by
/// name, then `start` and `count`, so that every call an answer suggests
/// reads as the tool's signature does, whatever order it was given in.
pub(crate) fn call_text(tool: &str, arguments: &Value) -> String {
    let Some(object) = arguments.as_object() else {
        return format!("{tool} {arguments}");
    };
    let order: &[&str] = PARAMETER_ORDERS
        .iter()
        .find(|(name, _)| *name == tool)
        .map_or(&[], |(_, order)| order);
    let mut unlisted: Vec<&String> = object
        .keys()
        .filter(|name| {
            *name != FIRST && !order.contains(&name.as_str()) && !LAST.contains(&name.as_str())
        })
        .collect();
    unlisted.sort();
    let ordered = object
        .get_key_value(FIRST)
        .into_iter()
        .chain(order.iter().filter_map(|name| object.get_key_value(*name)))
        .chain(
            unlisted
                .into_iter()
                .filter_map(|name| object.get_key_value(name)),
        )
        .chain(LAST.iter().filter_map(|name| object.get_key_value(*name)));
    let members: Vec<String> = ordered
        .map(|(name, value)| format!("{}:{value}", Value::String(name.clone())))
        .collect();
    format!("{tool} {{{}}}", members.join(","))
}

/// `number` with a `,` between each group of three digits: `16,636`.
pub(crate) fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (place, digit) in digits.chars().enumerate() {
        if place > 0 && (digits.len() - place).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// The decimal units a size is written in, each a thousand times the one
/// before.
const SIZE_UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];

/// `bytes` in the largest decimal unit that keeps it at 1 or more, with
/// three significant digits at most and no trailing zero: `980 KB`,
/// `412 MB`, `1.1 GB`; below a kilobyte the bytes themselves.
pub(crate) fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    loop {
        while value >= 1000.0 && unit + 1 < SIZE_UNITS.len() {
            value /= 1000.0;
            unit += 1;
        }
        let decimals = if value >= 100.0 {
            0
        } else if value >= 10.0 {
            1
        } else {
            2
        };
        let text = format!("{value:.decimals$}");
        // Rounding may carry into a fourth digit, 999.6 KB to `1000`: the
        // next unit writes it.
        if text.parse::<f64>().is_ok_and(|rounded| rounded >= 1000.0) && unit + 1 < SIZE_UNITS.len()
        {
            value = 1000.0;
            continue;
        }
        let text = if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            text
        };
        return format!("{text} {}", SIZE_UNITS[unit]);
    }
}

/// `count` and the noun that counts, singular for one: `1 event`,
/// `16,636 events`.
pub(crate) fn counted(count: u64, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{} {noun}", thousands(count))
}

/// How many characters `text` holds, as the budget counts them.
pub(crate) fn characters(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_are_written_in_groups_of_three() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(16_636), "16,636");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn a_size_has_three_significant_digits_at_most() {
        assert_eq!(size(412_000_000), "412 MB");
        assert_eq!(size(1_100_000_000), "1.1 GB");
        assert_eq!(size(980_000), "980 KB");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1_000), "1 KB");
        assert_eq!(size(12_345), "12.3 KB");
        assert_eq!(size(1_234), "1.23 KB");
        assert_eq!(size(999_600), "1 MB");
        assert_eq!(size(412_449_999), "412 MB");
    }

    #[test]
    fn a_call_names_its_session_first_and_its_page_last() {
        assert_eq!(
            call_text(
                "debug_trace",
                &json!({ "start": 10, "list": "installs", "session": 1 })
            ),
            r##"debug_trace {"session":1,"list":"installs","start":10}"##
        );
        assert_eq!(
            call_text(
                "debug_code",
                &json!({ "count": 3, "frame": 0, "session": 1, "start": 3 })
            ),
            r##"debug_code {"session":1,"frame":0,"start":3,"count":3}"##
        );
        assert_eq!(
            call_text(
                "debug_trace_listing",
                &json!({ "kind": "inlining", "method": "#4", "session": 1 })
            ),
            r##"debug_trace_listing {"session":1,"method":"#4","kind":"inlining"}"##
        );
        assert_eq!(
            call_text(
                "debug_trace_events",
                &json!({ "method": "#20", "event": "declined", "session": 1 })
            ),
            r##"debug_trace_events {"session":1,"event":"declined","method":"#20"}"##
        );
        assert_eq!(
            call_text(
                "debug_trace_method",
                &json!({ "start": 50, "part": "events", "session": 1, "method": "Demo>>run" })
            ),
            r##"debug_trace_method {"session":1,"method":"Demo>>run","part":"events","start":50}"##
        );
        assert_eq!(
            call_text(
                "debug_trace_method",
                &json!({ "session": 1, "method": "#12" })
            ),
            r##"debug_trace_method {"session":1,"method":"#12"}"##
        );
    }

    #[test]
    fn a_showing_line_says_where_the_page_stopped() {
        assert_eq!(showing_line(1, 64, 64, None), "Showing 1–64 of 64");
        assert_eq!(
            showing_line(
                1_001,
                1_050,
                2_000,
                Some((
                    "debug_trace_events",
                    json!({ "session": 1, "category": "deoptimize", "start": 1_050 })
                ))
            ),
            r##"Showing 1,001–1,050 of 2,000 — next: debug_trace_events {"session":1,"category":"deoptimize","start":1050}"##
        );
    }

    #[test]
    fn a_next_call_repeats_every_argument_given() {
        assert_eq!(
            next_arguments(&json!({ "frame": 0, "count": 3 }), 1, 3),
            json!({ "session": 1, "frame": 0, "start": 3, "count": 3 })
        );
        assert_eq!(
            next_arguments(&json!({ "session": 2, "start": 3 }), 2, 6),
            json!({ "session": 2, "start": 6 })
        );
        assert_eq!(
            next_arguments(&Value::Null, 1, 50),
            json!({ "session": 1, "start": 50 })
        );
    }

    #[test]
    fn a_page_says_where_it_stopped_unless_it_is_the_whole() {
        let entries: Vec<String> = (1..=5).map(|entry| format!("entry {entry}")).collect();
        let next_from = |start: usize| ("debug_stack", json!({ "session": 1, "start": start }));
        assert_eq!(page(&entries, 0, 5, ANSWER_BUDGET, next_from), entries);
        assert_eq!(
            page(&entries[..2], 0, 5, ANSWER_BUDGET, next_from),
            [
                "entry 1",
                "entry 2",
                r#"Showing 1–2 of 5 — next: debug_stack {"session":1,"start":2}"#
            ]
        );
        assert_eq!(
            page(&entries[3..], 3, 5, ANSWER_BUDGET, next_from),
            ["entry 4", "entry 5", "Showing 4–5 of 5"]
        );
        // A budget of the Showing line at its longest and two entries,
        // each with its newline.
        let showing_at_most =
            characters(r#"Showing 5–5 of 5 — next: debug_stack {"session":1,"start":5}"#) + 1;
        let budget = showing_at_most + 2 * ("entry 1".len() + 1);
        assert_eq!(
            page(&entries, 0, 5, budget, next_from),
            [
                "entry 1",
                "entry 2",
                r#"Showing 1–2 of 5 — next: debug_stack {"session":1,"start":2}"#
            ]
        );
        assert_eq!(
            page(&entries, 0, 5, budget - 1, next_from),
            [
                "entry 1",
                r#"Showing 1–1 of 5 — next: debug_stack {"session":1,"start":1}"#
            ]
        );
    }

    #[test]
    fn a_line_is_cut_saying_how_much_it_left_out() {
        let line = "x".repeat(1_000);
        // The marker is sized for every character counted out, so the
        // cut may end a little short of the width, never past it.
        assert_eq!(
            cut_line(&line, 100, None),
            format!("{}… (929 characters left out)", "x".repeat(71))
        );
        assert_eq!(
            cut_line(&line, 100, Some("see the file")),
            format!(
                "{}… (943 characters left out; see the file)",
                "x".repeat(57)
            )
        );
        assert_eq!(cut_line("short", 100, None), "short");
    }

    #[test]
    fn a_page_s_first_entry_is_taken_cut_and_passed() {
        let long = "y".repeat(ANSWER_BUDGET * 2);
        let entries = vec!["short".to_string(), long.clone(), "after".to_string()];
        let next_from = |start: usize| ("debug_code", json!({ "session": 1, "start": start }));
        // The page ends before the long entry, which is not its first.
        assert_eq!(
            page(&entries, 0, 3, ANSWER_BUDGET, next_from),
            [
                "short",
                r#"Showing 1–1 of 3 — next: debug_code {"session":1,"start":1}"#
            ]
        );
        // The page it begins takes it cut, and the next one goes on after
        // it: never a page of nothing whose next call is the call made.
        let cut = page(&entries[1..], 1, 3, ANSWER_BUDGET, next_from);
        assert_eq!(cut.len(), 2, "{cut:?}");
        assert!(cut[0].starts_with("yyy"), "{}", &cut[0][..10]);
        assert!(
            cut[0].ends_with(" characters left out)"),
            "{}",
            &cut[0][cut[0].len() - 40..]
        );
        assert_eq!(
            cut[1],
            r#"Showing 2–2 of 3 — next: debug_code {"session":1,"start":2}"#
        );
        let used: usize = cut.iter().map(|line| characters(line) + 1).sum();
        assert!(used <= ANSWER_BUDGET + 1, "{used}");
        // Too little room left to say anything takes nothing.
        assert_eq!(taken_within(&long, CUT_MINIMUM, true, None), None);
        assert_eq!(taken_within(&long, 10_000, false, None), None);
    }

    #[test]
    fn lines_are_fitted_to_a_budget_longest_first() {
        let mut lines = vec!["a".repeat(300), "b".repeat(1_000), "c".repeat(50)];
        fit_lines(&mut lines, 1_000, None);
        let used: usize = lines.iter().map(|line| characters(line) + 1).sum::<usize>() - 1;
        assert!(used <= 1_000, "{used}");
        assert_eq!(lines[0], "a".repeat(300));
        assert_eq!(lines[2], "c".repeat(50));
        assert!(lines[1].ends_with(" characters left out)"), "{}", lines[1]);
        let mut fitting = vec!["a".repeat(10)];
        fit_lines(&mut fitting, 1_000, None);
        assert_eq!(fitting, ["a".repeat(10)]);
    }

    #[test]
    fn a_count_is_bounded() {
        assert_eq!(count_of(None, OVERVIEW_DEFAULT, ENTRY_MAXIMUM), Ok(10));
        assert_eq!(count_of(Some(500), ENTRY_DEFAULT, ENTRY_MAXIMUM), Ok(200));
        assert_eq!(count_of(Some(7), ENTRY_DEFAULT, ENTRY_MAXIMUM), Ok(7));
        assert_eq!(count_of(None, LINE_DEFAULT, LINE_MAXIMUM), Ok(150));
        assert_eq!(count_of(Some(501), LINE_DEFAULT, LINE_MAXIMUM), Ok(500));
        assert!(count_of(Some(0), ENTRY_DEFAULT, ENTRY_MAXIMUM).is_err());
    }

    #[test]
    fn a_count_names_one_in_the_singular() {
        assert_eq!(counted(1, "event", "events"), "1 event");
        assert_eq!(counted(16_636, "event", "events"), "16,636 events");
    }
}
