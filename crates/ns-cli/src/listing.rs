//! The text of a listing (`docs/design/developer-tool.md`, decisions 11,
//! 12 and 14): a body's bytecode, its machine code decoded for the target
//! the binary runs on, and an optimizing compile's inlining plan, each
//! between a head line `--- <kind> {<body>} …` and an end line `--- end`,
//! so that a script cuts it out of standard error.

use std::fmt::Write as _;

use ns_runtime::trace_events::{
    Annotation, Architecture, ArmKind, CodeListing, CodeTier, DataWord, InliningListing,
    InliningOutcome, ListedScope, Listing, Position,
};

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "x86_64")]
mod x64;

/// The line every listing ends with.
const END: &str = "--- end\n";

/// How many bytes a line of a listing shows in hex, for a target this
/// binary has no decoder of.
const HEX_LINE_BYTES: usize = 4;

/// The whole text of `listing`, its head and end lines included, each
/// line ended by a newline.
pub(crate) fn text(listing: &Listing) -> String {
    let mut text = String::new();
    match listing {
        Listing::Bytecode(bytecode) => {
            let _ = writeln!(text, "--- bytecode {{{}}}", bytecode.body.name);
            text.push_str(&bytecode.text);
            if !text.ends_with('\n') {
                text.push('\n');
            }
        }
        Listing::Code(code) => write_code(&mut text, code),
        Listing::Inlining(inlining) => write_inlining(&mut text, inlining),
    }
    text.push_str(END);
    text
}

/// An instruction decoded: its offset in the body, its length in bytes
/// and its text, every target in the body named by its offset.
struct Decoded {
    offset: u32,
    length: u32,
    text: String,
}

/// An offset in the body as a listing writes it, `+0x` and at least four
/// hex digits: the same in every run, wherever the code zone put the
/// body (decision 14).
fn body_offset(offset: impl std::fmt::LowerHex) -> String {
    format!("+0x{offset:04x}")
}

/// A data word's address as a listing writes it, `data+0x` and its offset
/// from the first data word, at least four hex digits (decision 14).
fn data_offset(offset: impl std::fmt::LowerHex) -> String {
    format!("data+0x{offset:04x}")
}

/// The extent of a listed body, by which a decoder names an address its
/// code computes: an instruction's by its offset in the body, a data
/// word's by its offset from the first data word.
#[derive(Clone, Copy)]
struct Extent {
    /// The body's length in bytes.
    length: u64,
    /// The offset of the first data word; the length for a body without
    /// data words.
    data_start: u64,
}

impl Extent {
    fn of_listing(code: &CodeListing) -> Extent {
        let length = u64::try_from(code.bytes.len()).unwrap_or(u64::MAX);
        let data_start = code
            .words
            .first()
            .map_or(length, |(offset, _)| u64::from(*offset));
        Extent { length, data_start }
    }

    /// The name of `address`, an offset from the body's start; `None`
    /// for an address outside the body, which the decoder writes as it
    /// computes it.
    fn name(self, address: u64) -> Option<String> {
        if address >= self.length {
            None
        } else if address >= self.data_start {
            Some(data_offset(address - self.data_start))
        } else {
            Some(body_offset(address))
        }
    }
}

/// ` key={…}` and ` entered={…}`, each only where the listing has it, as
/// the trace's events write a customization key.
fn write_key(text: &mut String, key: Option<&(String, Option<String>)>) {
    if let Some((receiver, entered)) = key {
        let _ = write!(text, " key={{{receiver}}}");
        if let Some(entered) = entered {
            let _ = write!(text, " entered={{{entered}}}");
        }
    }
}

/// The tree of `scopes`, each under the scope it is inlined into, two
/// spaces deeper, with the send it is inlined at; the children of a scope
/// in the order of their numbers.
fn write_scope_tree(text: &mut String, scopes: &[ListedScope]) {
    let mut pending: Vec<(&ListedScope, usize)> = scopes
        .iter()
        .filter(|scope| scope.parent.is_none())
        .rev()
        .map(|scope| (scope, 1))
        .collect();
    while let Some((scope, depth)) = pending.pop() {
        let indent = "  ".repeat(depth);
        let _ = write!(text, "{indent}s{} {{{}}}", scope.number, scope.body.name);
        if let Some((parent, program_counter)) = scope.parent {
            let _ = write!(text, " at s{parent}@{program_counter}");
        }
        text.push('\n');
        pending.extend(
            scopes
                .iter()
                .filter(|child| {
                    child
                        .parent
                        .is_some_and(|(parent, _)| parent == scope.number)
                })
                .rev()
                .map(|child| (child, depth + 1)),
        );
    }
}

fn write_inlining(text: &mut String, inlining: &InliningListing) {
    let _ = write!(text, "--- inlining {{{}}}", inlining.body.name);
    write_key(text, inlining.key.as_ref());
    text.push('\n');
    write_scope_tree(text, &inlining.scopes);
    for arm in &inlining.arms {
        let _ = write!(
            text,
            "  s{}@{} {{{}}} -> ",
            arm.scope, arm.program_counter, arm.receiver
        );
        let _ = match arm.inlined {
            ArmKind::Scope(scope) => writeln!(text, "s{scope}"),
            ArmKind::Always(kind) => writeln!(text, "always {kind}"),
        };
    }
    for declined in &inlining.declined {
        let _ = writeln!(
            text,
            "  s{}@{} declined callee={{{}}} reason={}",
            declined.scope,
            declined.program_counter,
            declined
                .callee
                .as_ref()
                .map_or("", |callee| callee.name.as_str()),
            declined.reason.name()
        );
    }
    let _ = match inlining.outcome {
        InliningOutcome::Installed { bytes } => writeln!(text, "  installed bytes={bytes}"),
        InliningOutcome::Refused(refusal) => {
            writeln!(text, "  refused reason={}", refusal.name())
        }
    };
}

fn write_code(text: &mut String, code: &CodeListing) {
    let _ = write!(
        text,
        "--- code {{{}}} {} {}",
        code.body.name,
        code.tier.name(),
        code.architecture.name()
    );
    write_key(text, code.key.as_ref());
    let _ = writeln!(
        text,
        " base=0x{:x} bytes={}",
        code.base_address,
        code.bytes.len()
    );
    // Only an optimized copy lays out more than its body's own scope; a
    // baseline body's positions name its program counters alone.
    let optimized = code.tier == CodeTier::Optimized;
    if optimized {
        text.push_str("scopes:\n");
        write_scope_tree(text, &code.scopes);
    }
    let mut positions = code.positions.iter().peekable();
    let mut annotations = code.annotations.iter().peekable();
    let mut last_position = None;
    for instruction in decoded(code) {
        while let Some((_, position)) =
            positions.next_if(|(offset, _)| *offset <= instruction.offset)
        {
            // A stretch is named where its scope or program counter
            // changes; an instruction that emits no code leaves its
            // position beside the next one's, both named.
            if last_position != Some(position) {
                write_position(text, code, *position, optimized);
                last_position = Some(position);
            }
        }
        // The instruction a stopped frame returns to takes `->` in its
        // indentation, so that the columns stay those of every other line.
        let indent = if code.mark == Some(instruction.offset) {
            "  ->"
        } else {
            "    "
        };
        let _ = write!(
            text,
            "{indent}{}  {}",
            body_offset(instruction.offset),
            instruction.text
        );
        let end = instruction.offset.saturating_add(instruction.length);
        while let Some((_, annotation)) = annotations.next_if(|(offset, _)| *offset < end) {
            let _ = write!(text, "  ; {}", annotation_text(annotation));
        }
        text.push('\n');
    }
    // Neither names an offset past the last instruction: the code's
    // positions and annotations lie within it.
    debug_assert!(
        positions.next().is_none(),
        "a position past the last instruction"
    );
    debug_assert!(
        annotations.next().is_none(),
        "an annotation past the last instruction"
    );
    if let Some((data_start, _)) = code.words.first() {
        text.push_str("data:\n");
        for (offset, word) in &code.words {
            let _ = writeln!(
                text,
                "    {}  {}",
                data_offset(offset - data_start),
                word_text(*word)
            );
        }
    }
}

/// The line that names the stretch of code from `position` on: its scope
/// and that scope's body for an optimized copy, its program counter, and
/// the instruction there as the bytecode listing writes it.
fn write_position(text: &mut String, code: &CodeListing, position: Position, optimized: bool) {
    let (scope, program_counter) = match position {
        Position::Instruction {
            scope,
            program_counter,
        } => (scope, program_counter),
        Position::OutOfLine => {
            text.push_str("  out of line\n");
            return;
        }
    };
    let listed = code.scopes.iter().find(|listed| listed.number == scope);
    text.push_str("  ");
    if optimized {
        let body = listed.map_or("", |listed| listed.body.name.as_str());
        let _ = write!(text, "s{scope} {{{body}}} ");
    }
    let _ = write!(text, "@{program_counter}");
    let instruction = listed.and_then(|listed| {
        listed
            .instructions
            .get(usize::try_from(program_counter).ok()?)
    });
    if let Some(line) = instruction {
        // The line's own index repeats the program counter just written.
        let without_index = line
            .trim_start()
            .split_once(' ')
            .map_or("", |(_, rest)| rest.trim_start());
        let _ = write!(text, "  {without_index}");
    }
    text.push('\n');
}

/// What a call or jump out of the body reaches, named so that no address
/// of the run appears.
fn annotation_text(annotation: &Annotation) -> String {
    match annotation {
        Annotation::Leaf(name) => format!("leaf {name}"),
        Annotation::Stub(name) => format!("stub {name}"),
        Annotation::Exit {
            kind,
            program_counter,
        } => format!("exit {kind} @{program_counter}"),
        Annotation::Cell { site } => format!("cell site {site}"),
        Annotation::Target(name) => format!("target {name}"),
    }
}

/// What a data word holds, by its kind: its value is an address of the
/// run.
fn word_text(word: DataWord) -> String {
    match word {
        DataWord::Budget => "budget word".to_string(),
        DataWord::DispatchingRows => "dispatching rows".to_string(),
        DataWord::DispatchingTail => "dispatching tail".to_string(),
        DataWord::TwinTransfer { program_counter } => {
            format!("twin transfer @{program_counter}")
        }
        DataWord::Other => "site".to_string(),
    }
}

/// The instructions of `code`'s bytes before its data words, decoded by
/// the decoder of the target this binary runs on; in hex for any other
/// target, which a run never lists.
fn decoded(code: &CodeListing) -> Vec<Decoded> {
    let end = usize::try_from(code.code_end)
        .unwrap_or(usize::MAX)
        .min(code.bytes.len());
    let instructions = &code.bytes[..end];
    match code.architecture {
        #[cfg(target_arch = "x86_64")]
        Architecture::X64 => x64::decoded(instructions, Extent::of_listing(code)),
        #[cfg(target_arch = "aarch64")]
        Architecture::Aarch64 => aarch64::decoded(instructions, Extent::of_listing(code)),
        _ => hex(instructions),
    }
}

/// `bytes` in hex, `HEX_LINE_BYTES` a line.
fn hex(bytes: &[u8]) -> Vec<Decoded> {
    bytes
        .chunks(HEX_LINE_BYTES)
        .zip((0u32..).step_by(HEX_LINE_BYTES))
        .map(|(chunk, offset)| Decoded {
            offset,
            length: u32::try_from(chunk.len()).unwrap_or(u32::MAX),
            text: hex_bytes(chunk),
        })
        .collect()
}

/// `bytes` as hex pairs separated by spaces.
fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ns_runtime::trace_events::{
        Annotation, Architecture, ArmKind, BodyName, BytecodeListing, CodeListing, CodeTier,
        CompileRefusal, DataWord, InlineDecline, InliningListing, InliningOutcome, ListedArm,
        ListedDecline, ListedScope, Position,
    };

    /// `name`, numbered as a trace's first body is.
    fn named(name: &str) -> BodyName {
        BodyName {
            number: 1,
            name: name.to_string(),
        }
    }

    /// A scope numbered `number`, its body `body`, inlined at `parent`,
    /// with `instructions`' lines.
    fn scope(
        number: u32,
        body: &str,
        parent: Option<(u32, u32)>,
        instructions: &[&str],
    ) -> ListedScope {
        ListedScope {
            number,
            body: named(body),
            parent,
            instructions: instructions.iter().map(ToString::to_string).collect(),
        }
    }

    /// An architecture this binary has no decoder of, whose bytes a
    /// listing shows in hex.
    fn foreign_architecture() -> Architecture {
        if cfg!(target_arch = "x86_64") {
            Architecture::Aarch64
        } else {
            Architecture::X64
        }
    }

    #[test]
    fn a_bytecode_listing_is_the_disassembly_between_its_head_and_end() {
        let listing = Listing::Bytecode(BytecodeListing {
            body: named("Demo>>sum:"),
            text: "method Demo>>sum: (args 1, registers 2)\n   0   stmt 0  Return r0\n".to_string(),
        });
        assert_eq!(
            text(&listing),
            "--- bytecode {Demo>>sum:}\n\
             method Demo>>sum: (args 1, registers 2)\n   0   stmt 0  Return r0\n\
             --- end\n"
        );
    }

    #[test]
    fn an_inlining_listing_is_its_tree_its_arms_its_declines_and_its_outcome() {
        let mut inlining = InliningListing {
            body: named("Demo>>total"),
            key: Some(("Demo/Account".to_string(), Some("Demo/Base".to_string()))),
            scopes: vec![
                scope(0, "Demo>>total", None, &[]),
                scope(1, "Demo/Account>>balance", Some((0, 5)), &[]),
                scope(2, "Demo/Account>>rate", Some((0, 9)), &[]),
                scope(3, "Demo/Account>>raw", Some((1, 2)), &[]),
            ],
            arms: vec![
                ListedArm {
                    scope: 0,
                    program_counter: 5,
                    receiver: "Demo/Account".to_string(),
                    inlined: ArmKind::Scope(1),
                },
                ListedArm {
                    scope: 1,
                    program_counter: 2,
                    receiver: "#7".to_string(),
                    inlined: ArmKind::Always("getter"),
                },
            ],
            declined: vec![
                ListedDecline {
                    scope: 0,
                    program_counter: 11,
                    callee: Some(named("Demo>>helper")),
                    reason: InlineDecline::OverWeight,
                },
                ListedDecline {
                    scope: 2,
                    program_counter: 4,
                    callee: None,
                    reason: InlineDecline::NotExpressed,
                },
            ],
            outcome: InliningOutcome::Installed { bytes: 400 },
        };
        assert_eq!(
            text(&Listing::Inlining(inlining.clone())),
            "--- inlining {Demo>>total} key={Demo/Account} entered={Demo/Base}\n\
             \x20 s0 {Demo>>total}\n\
             \x20   s1 {Demo/Account>>balance} at s0@5\n\
             \x20     s3 {Demo/Account>>raw} at s1@2\n\
             \x20   s2 {Demo/Account>>rate} at s0@9\n\
             \x20 s0@5 {Demo/Account} -> s1\n\
             \x20 s1@2 {#7} -> always getter\n\
             \x20 s0@11 declined callee={Demo>>helper} reason=over-weight\n\
             \x20 s2@4 declined callee={} reason=not-expressed\n\
             \x20 installed bytes=400\n\
             --- end\n"
        );
        inlining.key = None;
        inlining.outcome = InliningOutcome::Refused(CompileRefusal::ZoneFull);
        let refused = text(&Listing::Inlining(inlining));
        assert!(
            refused.starts_with("--- inlining {Demo>>total}\n"),
            "{refused}"
        );
        assert!(
            refused.ends_with("\n  refused reason=zone-full\n--- end\n"),
            "{refused}"
        );
    }

    /// An optimized copy's listing heads with its key and its tree of
    /// scopes, and names each stretch's scope, body and program counter;
    /// a target the binary cannot decode is shown in hex.
    #[test]
    fn an_optimized_listing_names_its_scopes_and_shows_foreign_bytes_in_hex() {
        let listing = Listing::Code(CodeListing {
            body: named("Demo>>total"),
            tier: CodeTier::Optimized,
            key: Some(("Demo".to_string(), None)),
            architecture: foreign_architecture(),
            base_address: 0x7ff0_1000,
            bytes: (0u8..24).collect(),
            code_end: 16,
            words: vec![(16, DataWord::TwinTransfer { program_counter: 3 })],
            positions: vec![
                (
                    0,
                    Position::Instruction {
                        scope: 0,
                        program_counter: 0,
                    },
                ),
                (
                    4,
                    Position::Instruction {
                        scope: 1,
                        program_counter: 1,
                    },
                ),
                (8, Position::OutOfLine),
            ],
            annotations: vec![(4, Annotation::Cell { site: 3 })],
            scopes: vec![
                scope(
                    0,
                    "Demo>>total",
                    None,
                    &["   0   stmt 0  SendSelf #balance"],
                ),
                scope(
                    1,
                    "Demo/Account>>balance",
                    Some((0, 0)),
                    &["   0       -  Nothing", "   1   stmt 0  Return r1"],
                ),
            ],
            mark: None,
        });
        assert_eq!(
            text(&listing),
            format!(
                "--- code {{Demo>>total}} optimized {} key={{Demo}} base=0x7ff01000 bytes=24\n\
                 scopes:\n\
                 \x20 s0 {{Demo>>total}}\n\
                 \x20   s1 {{Demo/Account>>balance}} at s0@0\n\
                 \x20 s0 {{Demo>>total}} @0  stmt 0  SendSelf #balance\n\
                 \x20   +0x0000  00 01 02 03\n\
                 \x20 s1 {{Demo/Account>>balance}} @1  stmt 0  Return r1\n\
                 \x20   +0x0004  04 05 06 07  ; cell site 3\n\
                 \x20 out of line\n\
                 \x20   +0x0008  08 09 0a 0b\n\
                 \x20   +0x000c  0c 0d 0e 0f\n\
                 data:\n\
                 \x20   data+0x0000  twin transfer @3\n\
                 --- end\n",
                foreign_architecture().name()
            )
        );
    }

    /// A stopped frame's listing marks the instruction its frame returns
    /// to, in the indentation, every column kept.
    #[test]
    fn a_frame_s_listing_marks_the_instruction_it_returns_to() {
        let listing = Listing::Code(CodeListing {
            body: named("Demo>>main:args:"),
            tier: CodeTier::Baseline,
            key: None,
            architecture: foreign_architecture(),
            base_address: 0x3000,
            bytes: (0u8..12).collect(),
            code_end: 12,
            words: Vec::new(),
            positions: vec![(
                0,
                Position::Instruction {
                    scope: 0,
                    program_counter: 0,
                },
            )],
            annotations: vec![(0, Annotation::Cell { site: 0 })],
            scopes: vec![scope(
                0,
                "Demo>>main:args:",
                None,
                &["   0   stmt 0  SendSelf #run"],
            )],
            mark: Some(4),
        });
        assert_eq!(
            text(&listing),
            format!(
                "--- code {{Demo>>main:args:}} baseline {} base=0x3000 bytes=12\n\
                 \x20 @0  stmt 0  SendSelf #run\n\
                 \x20   +0x0000  00 01 02 03  ; cell site 0\n\
                 \x20 ->+0x0004  04 05 06 07\n\
                 \x20   +0x0008  08 09 0a 0b\n\
                 --- end\n",
                foreign_architecture().name()
            )
        );
    }

    /// A baseline listing on x64: every kind of annotation beside its
    /// instruction, a branch and a load into the body named by the
    /// offset they reach, and the data words by their kind.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn an_x64_listing_names_its_targets() {
        let bytes = vec![
            0xEB, 0x11, // jmp short to the ret at 0x13
            0xFF, 0xD0, // call rax
            0xFF, 0xD1, // call rcx
            0xFF, 0xE2, // jmp rdx
            0xFF, 0xD3, // call rbx
            0xFF, 0xE0, // jmp rax
            0x48, 0x8B, 0x05, 0x01, 0x00, 0x00, 0x00, // mov rax, [rip+1], the word at 0x14
            0xC3, // ret
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // the data word
        ];
        let listing = Listing::Code(CodeListing {
            body: named("Demo>>square:"),
            tier: CodeTier::Baseline,
            key: None,
            architecture: Architecture::X64,
            base_address: 0x2000,
            bytes,
            code_end: 0x14,
            words: vec![(0x14, DataWord::Budget)],
            positions: vec![
                (
                    2,
                    Position::Instruction {
                        scope: 0,
                        program_counter: 0,
                    },
                ),
                (12, Position::OutOfLine),
                (
                    0x13,
                    Position::Instruction {
                        scope: 0,
                        program_counter: 1,
                    },
                ),
            ],
            annotations: vec![
                (2, Annotation::Leaf("miss")),
                (4, Annotation::Stub("polymorphic-stub")),
                (
                    6,
                    Annotation::Exit {
                        kind: "send",
                        program_counter: 12,
                    },
                ),
                (8, Annotation::Cell { site: 3 }),
                (10, Annotation::Target("twin")),
            ],
            scopes: vec![scope(
                0,
                "Demo>>square:",
                None,
                &["   0   stmt 0  Multiply r0, r0", "   1   stmt 0  Return r1"],
            )],
            mark: None,
        });
        assert_eq!(
            text(&listing),
            "--- code {Demo>>square:} baseline x64 base=0x2000 bytes=28\n\
             \x20   +0x0000  jmp short +0x0013\n\
             \x20 @0  stmt 0  Multiply r0, r0\n\
             \x20   +0x0002  call rax  ; leaf miss\n\
             \x20   +0x0004  call rcx  ; stub polymorphic-stub\n\
             \x20   +0x0006  jmp rdx  ; exit send @12\n\
             \x20   +0x0008  call rbx  ; cell site 3\n\
             \x20   +0x000a  jmp rax  ; target twin\n\
             \x20 out of line\n\
             \x20   +0x000c  mov rax,[data+0x0000]\n\
             \x20 @1  stmt 0  Return r1\n\
             \x20   +0x0013  ret\n\
             data:\n\
             \x20   data+0x0000  budget word\n\
             --- end\n"
        );
    }
}
