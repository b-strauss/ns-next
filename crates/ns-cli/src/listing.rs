//! The text of a listing (`docs/design/developer-tool.md`, decisions 11,
//! 12 and 14): a body's bytecode, its machine code decoded for the target
//! the binary runs on, and an optimizing compile's inlining plan, each
//! between a head line `--- <kind> {<body>} …` and an end line `--- end`,
//! so that a script cuts it out of standard error.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;

use ns_runtime::bytecode;
use ns_runtime::trace_events::{
    Annotation, Architecture, ArmKind, CodeListing, CodeTier, DataWord, InliningOutcome,
    ListedScope, Listing, Position,
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
    let view = View::of_listing(listing);
    let text = view.text();
    // A bytecode listing is written from its bodies, as a trace file's
    // record is read back; the runtime's disassembly beside them is the
    // oracle the two layouts are held to.
    #[cfg(debug_assertions)]
    if let Listing::Bytecode(bytecode) = listing
        && !bytecode.bodies.is_empty()
    {
        let disassembled = View::BytecodeText {
            name: bytecode.body.name.clone(),
            text: bytecode.text.clone(),
        };
        debug_assert_eq!(
            text,
            disassembled.text(),
            "a bytecode listing's bodies write its disassembly"
        );
    }
    text
}

/// A listing as its text is written from it: what the runtime hands the
/// sink and what a trace file's record holds are both read into it, so
/// that `--print-*` and the MCP server's window over a record write one
/// layout through one function (`View::text`), every body named as the
/// trace names it.
pub(crate) enum View {
    /// Bytecode given as its disassembly alone, as a stopped frame's is.
    BytecodeText {
        name: String,
        text: String,
    },
    /// Bytecode by its bodies, the root first, then the blocks in the
    /// order the disassembly nests them.
    Bytecode {
        name: String,
        bodies: Vec<ViewBody>,
    },
    Code(CodeView),
    Inlining(InliningView),
}

/// One body of a bytecode listing.
pub(crate) struct ViewBody {
    pub(crate) number: u32,
    /// Its kind's word, `method` or `block`.
    pub(crate) kind: String,
    /// A block's maker, by number, and the index of the `MakeClosure`
    /// there it is nested under; `None` for the root.
    pub(crate) made_at: Option<(u32, u32)>,
    /// Its counts of arguments and registers; `None` where they are not
    /// known, whose line has none.
    pub(crate) counts: Option<(u16, u16)>,
    /// Each instruction's index, statement and text.
    pub(crate) instructions: Vec<(u32, Option<u32>, String)>,
}

/// A scope of a code or inlining listing.
pub(crate) struct ViewScope {
    pub(crate) number: u32,
    pub(crate) body: String,
    /// The scope and program counter it is inlined at.
    pub(crate) parent: Option<(u32, u32)>,
    /// Its body's instructions as the bytecode listing writes them, by
    /// program counter; empty where they are not known.
    pub(crate) instructions: Vec<String>,
}

/// A body's machine code.
pub(crate) struct CodeView {
    pub(crate) name: String,
    pub(crate) tier: String,
    pub(crate) architecture: String,
    pub(crate) key: Option<(String, Option<String>)>,
    pub(crate) base: u64,
    pub(crate) bytes: u64,
    pub(crate) scopes: Vec<ViewScope>,
    pub(crate) instructions: Vec<ViewInstruction>,
    /// Each data word's offset from the first and what it holds.
    pub(crate) data: Vec<(u64, String)>,
    pub(crate) mark: Option<u32>,
}

/// An instruction of a code listing.
pub(crate) struct ViewInstruction {
    pub(crate) offset: u32,
    pub(crate) text: String,
    /// The positions reached at it, in order (`AnnotatedInstruction`).
    pub(crate) reached: Vec<Position>,
    /// What each call or jump out of it reaches, as `annotation_text`
    /// writes it.
    pub(crate) annotations: Vec<String>,
}

/// An optimizing compile's inlining plan.
pub(crate) struct InliningView {
    pub(crate) name: String,
    pub(crate) key: Option<(String, Option<String>)>,
    pub(crate) scopes: Vec<ViewScope>,
    pub(crate) arms: Vec<ViewArm>,
    pub(crate) declined: Vec<ViewDecline>,
    pub(crate) outcome: ViewOutcome,
}

/// An arm of a type-case.
pub(crate) struct ViewArm {
    pub(crate) scope: u32,
    pub(crate) program_counter: u32,
    pub(crate) receiver: String,
    pub(crate) inlined: ViewArmKind,
}

/// What an arm inlines: a scope, or always the same kind of code.
pub(crate) enum ViewArmKind {
    Scope(u32),
    Always(String),
}

/// A send the plan declined to inline.
pub(crate) struct ViewDecline {
    pub(crate) scope: u32,
    pub(crate) program_counter: u32,
    pub(crate) callee: Option<String>,
    pub(crate) reason: String,
}

/// What a plan's compile came to.
pub(crate) enum ViewOutcome {
    Installed(usize),
    Refused(String),
}

impl View {
    /// `listing` as a view, its instructions decoded.
    pub(crate) fn of_listing(listing: &Listing) -> View {
        match listing {
            Listing::Bytecode(bytecode) if bytecode.bodies.is_empty() => View::BytecodeText {
                name: bytecode.body.name.clone(),
                text: bytecode.text.clone(),
            },
            Listing::Bytecode(bytecode) => View::Bytecode {
                name: bytecode.body.name.clone(),
                bodies: bytecode
                    .bodies
                    .iter()
                    .map(|listed| ViewBody {
                        number: listed.body.number,
                        kind: listed.kind.to_string(),
                        made_at: listed.made_at.map(|made_at| (made_at.maker, made_at.index)),
                        counts: Some((listed.argument_count, listed.register_count)),
                        instructions: listed
                            .instructions
                            .iter()
                            .map(|instruction| {
                                (
                                    instruction.index,
                                    instruction.statement,
                                    instruction.text.clone(),
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            },
            Listing::Code(code) => View::Code(code_view(code)),
            Listing::Inlining(inlining) => View::Inlining(InliningView {
                name: inlining.body.name.clone(),
                key: inlining.key.clone(),
                scopes: view_scopes(&inlining.scopes),
                arms: inlining
                    .arms
                    .iter()
                    .map(|arm| ViewArm {
                        scope: arm.scope,
                        program_counter: arm.program_counter,
                        receiver: arm.receiver.clone(),
                        inlined: match arm.inlined {
                            ArmKind::Scope(scope) => ViewArmKind::Scope(scope),
                            ArmKind::Always(kind) => ViewArmKind::Always(kind.to_string()),
                        },
                    })
                    .collect(),
                declined: inlining
                    .declined
                    .iter()
                    .map(|declined| ViewDecline {
                        scope: declined.scope,
                        program_counter: declined.program_counter,
                        callee: declined.callee.as_ref().map(|callee| callee.name.clone()),
                        reason: declined.reason.name().to_string(),
                    })
                    .collect(),
                outcome: match inlining.outcome {
                    InliningOutcome::Installed { bytes } => ViewOutcome::Installed(bytes),
                    InliningOutcome::Refused(refusal) => {
                        ViewOutcome::Refused(refusal.name().to_string())
                    }
                },
            }),
        }
    }

    /// The whole text of the listing, its head and end lines included,
    /// each line ended by a newline.
    pub(crate) fn text(&self) -> String {
        let mut text = String::new();
        match self {
            View::BytecodeText { name, text: listed } => {
                let _ = writeln!(text, "--- bytecode {{{name}}}");
                text.push_str(listed);
                if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            View::Bytecode { name, bodies } => {
                let _ = writeln!(text, "--- bytecode {{{name}}}");
                write_bodies(&mut text, name, bodies);
            }
            View::Code(code) => write_code(&mut text, code),
            View::Inlining(inlining) => write_inlining(&mut text, inlining),
        }
        text.push_str(END);
        text
    }

    /// The listing's lines, its head and end lines included, without
    /// their newlines.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.text().lines().map(ToString::to_string).collect()
    }
}

fn view_scopes(scopes: &[ListedScope]) -> Vec<ViewScope> {
    scopes
        .iter()
        .map(|scope| ViewScope {
            number: scope.number,
            body: scope.body.name.clone(),
            parent: scope.parent,
            instructions: scope.instructions.clone(),
        })
        .collect()
}

fn code_view(code: &CodeListing) -> CodeView {
    let data_start = code.words.first().map_or(0, |(offset, _)| *offset);
    CodeView {
        name: code.body.name.clone(),
        tier: code.tier.name().to_string(),
        architecture: code.architecture.name().to_string(),
        key: code.key.clone(),
        base: code.base_address,
        bytes: u64::try_from(code.bytes.len()).unwrap_or(u64::MAX),
        scopes: view_scopes(&code.scopes),
        instructions: annotated(code)
            .into_iter()
            .map(|annotated| ViewInstruction {
                offset: annotated.instruction.offset,
                text: annotated.instruction.text,
                reached: annotated.reached,
                annotations: annotated
                    .annotations
                    .into_iter()
                    .map(annotation_text)
                    .collect(),
            })
            .collect(),
        data: code
            .words
            .iter()
            .map(|(offset, word)| (u64::from(offset - data_start), word_text(*word)))
            .collect(),
        mark: code.mark,
    }
}

/// The bodies of a bytecode listing as the disassembly nests them: the
/// root under `name`, each block eight columns deeper right below the
/// `MakeClosure` that makes it. A worklist, not recursion: an image's body
/// chooses how deep its blocks nest.
fn write_bodies(text: &mut String, name: &str, bodies: &[ViewBody]) {
    let Some(root) = bodies.first() else {
        return;
    };
    // The blocks under each `MakeClosure`, in the order they are listed:
    // one block made at two places is listed at each.
    let mut blocks: HashMap<(u32, u32), VecDeque<usize>> = HashMap::new();
    for (position, body) in bodies.iter().enumerate().skip(1) {
        if let Some(made_at) = body.made_at {
            blocks.entry(made_at).or_default().push_back(position);
        }
    }
    write_body_line(text, root, name, 0);
    let mut pending: Vec<(usize, usize, usize)> = vec![(0, 0, 0)];
    while let Some((position, next, indent)) = pending.last_mut().map(|top| *top) {
        let body = &bodies[position];
        let Some((index, statement, instruction)) = body.instructions.get(next) else {
            pending.pop();
            continue;
        };
        if let Some(top) = pending.last_mut() {
            top.1 += 1;
        }
        let pad = " ".repeat(indent);
        let line = bytecode::listed_line(*index, *statement, instruction, false);
        let _ = writeln!(text, "{pad}{line}");
        if let Some(block) = blocks
            .get_mut(&(body.number, *index))
            .and_then(VecDeque::pop_front)
        {
            let deeper = indent + bytecode::BLOCK_INDENT;
            write_body_line(text, &bodies[block], bytecode::BLOCK_NAME, deeper);
            pending.push((block, 0, deeper));
        }
    }
}

/// The line above a body's instructions, `indent` columns in.
fn write_body_line(text: &mut String, body: &ViewBody, name: &str, indent: usize) {
    let pad = " ".repeat(indent);
    let line = match body.counts {
        Some((arguments, registers)) => bytecode::body_line(&body.kind, name, arguments, registers),
        None => format!("{} {name}", body.kind),
    };
    let _ = writeln!(text, "{pad}{line}");
}

/// An instruction decoded: its offset in the body, its length in bytes
/// and its text, every target in the body named by its offset.
pub(crate) struct Decoded {
    pub(crate) offset: u32,
    pub(crate) length: u32,
    pub(crate) text: String,
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
fn write_scope_tree(text: &mut String, scopes: &[ViewScope]) {
    let mut pending: Vec<(&ViewScope, usize)> = scopes
        .iter()
        .filter(|scope| scope.parent.is_none())
        .rev()
        .map(|scope| (scope, 1))
        .collect();
    while let Some((scope, depth)) = pending.pop() {
        let indent = "  ".repeat(depth);
        let _ = write!(text, "{indent}s{} {{{}}}", scope.number, scope.body);
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

fn write_inlining(text: &mut String, inlining: &InliningView) {
    let _ = write!(text, "--- inlining {{{}}}", inlining.name);
    write_key(text, inlining.key.as_ref());
    text.push('\n');
    write_scope_tree(text, &inlining.scopes);
    for arm in &inlining.arms {
        let _ = write!(
            text,
            "  s{}@{} {{{}}} -> ",
            arm.scope, arm.program_counter, arm.receiver
        );
        let _ = match &arm.inlined {
            ViewArmKind::Scope(scope) => writeln!(text, "s{scope}"),
            ViewArmKind::Always(kind) => writeln!(text, "always {kind}"),
        };
    }
    for declined in &inlining.declined {
        let _ = writeln!(
            text,
            "  s{}@{} declined callee={{{}}} reason={}",
            declined.scope,
            declined.program_counter,
            declined.callee.as_deref().unwrap_or(""),
            declined.reason
        );
    }
    let _ = match &inlining.outcome {
        ViewOutcome::Installed(bytes) => writeln!(text, "  installed bytes={bytes}"),
        ViewOutcome::Refused(reason) => writeln!(text, "  refused reason={reason}"),
    };
}

fn write_code(text: &mut String, code: &CodeView) {
    let _ = write!(
        text,
        "--- code {{{}}} {} {}",
        code.name, code.tier, code.architecture
    );
    write_key(text, code.key.as_ref());
    let _ = writeln!(text, " base=0x{:x} bytes={}", code.base, code.bytes);
    // Only an optimized copy lays out more than its body's own scope; a
    // baseline body's positions name its program counters alone.
    let optimized = code.tier == CodeTier::Optimized.name();
    if optimized {
        text.push_str("scopes:\n");
        write_scope_tree(text, &code.scopes);
    }
    let mut last_position = None;
    for instruction in &code.instructions {
        for position in &instruction.reached {
            // A stretch is named where its scope or program counter
            // changes; an instruction that emits no code leaves its
            // position beside the next one's, both named.
            if last_position != Some(*position) {
                write_position(text, code, *position, optimized);
                last_position = Some(*position);
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
        for annotation in &instruction.annotations {
            let _ = write!(text, "  ; {annotation}");
        }
        text.push('\n');
    }
    if !code.data.is_empty() {
        text.push_str("data:\n");
        for (offset, word) in &code.data {
            let _ = writeln!(text, "    {}  {word}", data_offset(*offset));
        }
    }
}

/// An instruction of a code listing with what the listing says of it.
pub(crate) struct AnnotatedInstruction<'code> {
    pub(crate) instruction: Decoded,
    /// The positions that start at or before the instruction and after
    /// the one before it, in order: more than one where instructions
    /// emitted no code.
    pub(crate) reached: Vec<Position>,
    /// The position in force at the instruction: the last one reached at
    /// it or before; `None` before the first.
    pub(crate) position: Option<Position>,
    /// What each call or jump out of the instruction reaches, in order.
    pub(crate) annotations: Vec<&'code Annotation>,
}

/// `code`'s instructions decoded, each with the positions and the
/// annotations that fall to it: the one walk by which the text listing
/// and the trace file both place them, so that the two never disagree.
pub(crate) fn annotated(code: &CodeListing) -> Vec<AnnotatedInstruction<'_>> {
    let mut positions = code.positions.iter().peekable();
    let mut annotations = code.annotations.iter().peekable();
    let mut position = None;
    let walked = decoded(code)
        .into_iter()
        .map(|instruction| {
            let mut reached = Vec::new();
            while let Some((_, next)) =
                positions.next_if(|(offset, _)| *offset <= instruction.offset)
            {
                reached.push(*next);
                position = Some(*next);
            }
            let end = instruction.offset.saturating_add(instruction.length);
            let mut falling = Vec::new();
            while let Some((_, annotation)) = annotations.next_if(|(offset, _)| *offset < end) {
                falling.push(annotation);
            }
            AnnotatedInstruction {
                instruction,
                reached,
                position,
                annotations: falling,
            }
        })
        .collect();
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
    walked
}

/// The line that names the stretch of code from `position` on: its scope
/// and that scope's body for an optimized copy, its program counter, and
/// the instruction there as the bytecode listing writes it.
fn write_position(text: &mut String, code: &CodeView, position: Position, optimized: bool) {
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
        let body = listed.map_or("", |listed| listed.body.as_str());
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
pub(crate) fn annotation_text(annotation: &Annotation) -> String {
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
pub(crate) fn word_text(word: DataWord) -> String {
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
            bodies: Vec::new(),
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
