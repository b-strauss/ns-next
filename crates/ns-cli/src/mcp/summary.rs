//! The program's state as the text a tool answers (`docs/design/debugger.md`,
//! "The MCP server"): a stop as one compact block -- where, the source
//! around it, the top frame's variables, the stack on one line, the output
//! since the last answer -- and the lines `debug_stack` and
//! `debug_variables` answer, in the same forms. Text, not JSON: it is what
//! an agent reads, in fewer tokens and as precisely. Every line that would
//! say nothing is left out.

/// The longest value a summary or a listing shows, in characters.
pub(crate) const VALUE_LIMIT: usize = 80;

/// The most variables a stop's summary shows.
pub(crate) const LOCALS_LIMIT: usize = 20;

/// The most frames a stop's summary shows.
pub(crate) const STACK_LIMIT: usize = 8;

/// The most characters of output one answer shows: the last ones, which
/// lead up to the stop or the end.
pub(crate) const OUTPUT_LIMIT: usize = 2_000;

/// The source lines a stop shows before and after its own.
const SOURCE_CONTEXT: u32 = 2;

/// A frame as the summary and `debug_stack` show it.
pub(crate) struct Frame {
    pub(crate) name: String,
    /// The absolute path of its module's file; none for a module shown by
    /// reference only, or a frame without a location.
    pub(crate) path: Option<String>,
    /// 1-based; 0 for a frame without a location.
    pub(crate) line: u32,
    pub(crate) column: u32,
    /// The frame runs a body of a shipped module.
    pub(crate) shipped: bool,
}

/// A variable, or a value's child, as a summary and `debug_variables`
/// show it.
pub(crate) struct Shown {
    pub(crate) name: String,
    pub(crate) value: String,
    /// The reference `debug_variables`, `debug_mark` and `debug_memory`
    /// take, for a value with children.
    pub(crate) reference: Option<i64>,
}

/// What the program is doing, as the first line of an answer says it.
pub(crate) enum Situation {
    Stopped(Box<Stop>),
    Running { seconds: f64 },
    Ended { status: Option<i64> },
}

/// A stop, as its summary shows it.
pub(crate) struct Stop {
    /// DAP's reason: `breakpoint`, `step`, `pause`, `entry`, `exception`,
    /// `function breakpoint`, `restart`.
    pub(crate) reason: String,
    /// At an exception's stop, `<class>: <text>`, or the text of a `halt`.
    pub(crate) exception: Option<String>,
    /// At a breakpoint whose condition or log message failed, its error.
    pub(crate) condition_error: Option<String>,
    /// The top frame, frame 0.
    pub(crate) top: Option<Frame>,
    /// The text of the top frame's file, when it could be read.
    pub(crate) text: Option<String>,
    pub(crate) locals: Vec<Shown>,
    /// How many variables of the top frame are not in `locals`.
    pub(crate) more_locals: usize,
    /// The first frames, at most `STACK_LIMIT`.
    pub(crate) stack: Vec<Frame>,
    pub(crate) total_frames: usize,
}

/// The answer of a tool that let the program go on, or that asked where
/// it is: the situation, the output since the last answer, and on
/// `debug_start`'s answer the binary and its fingerprint.
pub(crate) fn summary(situation: &Situation, output: &str, runtime: Option<&str>) -> String {
    let mut lines = Vec::new();
    match situation {
        Situation::Stopped(stop) => stopped_lines(stop, &mut lines),
        Situation::Running { seconds } => lines.push(format!("running for {seconds:.1} s")),
        Situation::Ended { status } => lines.push(match status {
            Some(status) => format!("ended: exit status {status}"),
            None => "ended: exit status unknown".to_string(),
        }),
    }
    lines.extend(output_line(output));
    if let Some(runtime) = runtime {
        lines.push(format!("runtime: {runtime}"));
    }
    lines.join("\n")
}

fn stopped_lines(stop: &Stop, lines: &mut Vec<String>) {
    let mut first = format!("stopped: {}", stop.reason);
    if let Some(top) = &stop.top {
        if let Some(path) = &top.path {
            first.push_str(&format!(" at {path}:{}:{}", top.line, top.column));
        }
        first.push_str(&format!(" in {} (frame 0)", top.name));
    }
    lines.push(first);
    if let (Some(top), Some(text)) = (&stop.top, &stop.text) {
        lines.extend(source_excerpt(text, top.line));
    }
    if let Some(exception) = &stop.exception {
        lines.push(format!("exception: {exception}"));
    }
    if let Some(error) = &stop.condition_error {
        lines.push(format!("condition failed: {error}"));
    }
    if !stop.locals.is_empty() {
        let mut shown: Vec<String> = stop.locals.iter().map(variable_text).collect();
        if stop.more_locals > 0 {
            shown.push(format!(
                "… {} more, debug_variables frame 0",
                stop.more_locals
            ));
        }
        lines.push(format!("locals: {}", shown.join(", ")));
    }
    if !stop.stack.is_empty() {
        let mut shown: Vec<String> = stop
            .stack
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                let place = frame
                    .path
                    .as_deref()
                    .map(|path| format!(" {}:{}", file_name(path), frame.line))
                    .unwrap_or_default();
                format!("#{index} {}{place}", frame.name)
            })
            .collect();
        let more = stop.total_frames.saturating_sub(stop.stack.len());
        if more > 0 {
            shown.push(format!("… {more} more, debug_stack"));
        }
        lines.push(format!("stack: {}", shown.join(" · ")));
    }
}

/// The lines around the 1-based `line` of `text`, numbered, `>` at `line`;
/// none when `line` is not in the text.
pub(crate) fn source_excerpt(text: &str, line: u32) -> Vec<String> {
    let all: Vec<&str> = text.lines().collect();
    let Ok(index) = usize::try_from(line) else {
        return Vec::new();
    };
    if index == 0 || index > all.len() {
        return Vec::new();
    }
    let first = line.saturating_sub(SOURCE_CONTEXT).max(1);
    let last = line
        .saturating_add(SOURCE_CONTEXT)
        .min(u32::try_from(all.len()).unwrap_or(u32::MAX));
    let width = last.to_string().len();
    (first..=last)
        .map(|number| {
            let marker = if number == line { '>' } else { ' ' };
            let text = all[usize::try_from(number - 1).unwrap_or_default()].trim_end();
            format!("{marker} {number:>width$} | {text}")
        })
        .collect()
}

/// `output: "<text>"`, the text quoted as a JSON string, its start cut
/// when it is longer than `OUTPUT_LIMIT`; none for no output.
pub(crate) fn output_line(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let count = text.chars().count();
    if count <= OUTPUT_LIMIT {
        return Some(format!("output: {}", quoted(text)));
    }
    let left_out = count - OUTPUT_LIMIT;
    let kept: String = text.chars().skip(left_out).collect();
    Some(format!(
        "output: ({left_out} earlier characters left out) {}",
        quoted(&kept)
    ))
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// `<name> = <value> [ref <n>]`, the value cut at `VALUE_LIMIT`.
pub(crate) fn variable_text(variable: &Shown) -> String {
    let mut text = format!("{} = {}", variable.name, cut(&variable.value, VALUE_LIMIT));
    if let Some(reference) = variable.reference {
        text.push_str(&format!(" [ref {reference}]"));
    }
    text
}

/// `#<index> <name> <path>:<line>:<column>`, a shipped frame marked.
pub(crate) fn frame_text(index: usize, frame: &Frame) -> String {
    let mut text = format!("#{index} {}", frame.name);
    if let Some(path) = &frame.path {
        text.push_str(&format!(" {path}:{}:{}", frame.line, frame.column));
    }
    if frame.shipped {
        text.push_str(" (core library)");
    }
    text
}

/// `text` cut at `limit` characters, `…` saying so.
pub(crate) fn cut(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// The last segment of a path, in either separator.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(name: &str, path: &str, line: u32) -> Frame {
        Frame {
            name: name.to_string(),
            path: Some(path.to_string()),
            line,
            column: 5,
            shipped: false,
        }
    }

    #[test]
    fn a_stop_is_summarized_in_its_order() {
        let text = "one\ntwo\nthree\nfour\nfive\nsix\n";
        let stop = Stop {
            reason: "breakpoint".to_string(),
            exception: None,
            condition_error: None,
            top: Some(frame("Counter>>main:args:", "/work/Counter.ns", 4)),
            text: Some(text.to_string()),
            locals: vec![
                Shown {
                    name: "count".to_string(),
                    value: "41".to_string(),
                    reference: None,
                },
                Shown {
                    name: "elements".to_string(),
                    value: "an Array".to_string(),
                    reference: Some(7),
                },
            ],
            more_locals: 0,
            stack: vec![
                frame("Counter>>main:args:", "/work/Counter.ns", 4),
                frame("Kernel>>run", "/core/Kernel.ns", 120),
            ],
            total_frames: 2,
        };
        let summary = summary(
            &Situation::Stopped(Box::new(stop)),
            "start\n",
            Some("/bin/ns-next 0123"),
        );
        assert_eq!(
            summary,
            "stopped: breakpoint at /work/Counter.ns:4:5 in Counter>>main:args: (frame 0)\n\
             \x20 2 | two\n\
             \x20 3 | three\n\
             > 4 | four\n\
             \x20 5 | five\n\
             \x20 6 | six\n\
             locals: count = 41, elements = an Array [ref 7]\n\
             stack: #0 Counter>>main:args: Counter.ns:4 · #1 Kernel>>run Kernel.ns:120\n\
             output: \"start\\n\"\n\
             runtime: /bin/ns-next 0123"
        );
    }

    #[test]
    fn long_lists_are_cut_saying_how_to_see_more() {
        let stop = Stop {
            reason: "exception".to_string(),
            exception: Some("Error: boom".to_string()),
            condition_error: None,
            top: Some(frame("Error>>signal", "/core/Kernel.ns", 1)),
            text: None,
            locals: vec![Shown {
                name: "text".to_string(),
                value: "x".repeat(100),
                reference: None,
            }],
            more_locals: 3,
            stack: (0..STACK_LIMIT)
                .map(|index| frame(&format!("F{index}"), "C:\\work\\A.ns", 1))
                .collect(),
            total_frames: STACK_LIMIT + 5,
        };
        let summary = summary(&Situation::Stopped(Box::new(stop)), "", None);
        let lines: Vec<&str> = summary.lines().collect();
        assert_eq!(lines[1], "exception: Error: boom");
        assert_eq!(
            lines[2],
            format!(
                "locals: text = {}…, … 3 more, debug_variables frame 0",
                "x".repeat(VALUE_LIMIT)
            )
        );
        assert!(
            lines[3].starts_with("stack: #0 F0 A.ns:1 · #1 F1 A.ns:1"),
            "{}",
            lines[3]
        );
        assert!(
            lines[3].ends_with(" · … 5 more, debug_stack"),
            "{}",
            lines[3]
        );
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn long_output_keeps_its_end() {
        let text = format!("{}{}", "a".repeat(10), "b".repeat(OUTPUT_LIMIT));
        let line = output_line(&text).expect("an output line");
        assert_eq!(
            line,
            format!(
                "output: (10 earlier characters left out) \"{}\"",
                "b".repeat(OUTPUT_LIMIT)
            )
        );
        assert_eq!(output_line(""), None);
    }

    #[test]
    fn running_and_ended_say_so() {
        assert_eq!(
            summary(&Situation::Running { seconds: 1.04 }, "x", None),
            "running for 1.0 s\noutput: \"x\""
        );
        assert_eq!(
            summary(&Situation::Ended { status: Some(0) }, "", None),
            "ended: exit status 0"
        );
    }

    #[test]
    fn an_excerpt_stays_in_the_text() {
        assert_eq!(
            source_excerpt("one\ntwo", 1),
            vec!["> 1 | one".to_string(), "  2 | two".to_string()]
        );
        assert!(source_excerpt("one", 3).is_empty());
        assert!(source_excerpt("one", 0).is_empty());
    }
}
