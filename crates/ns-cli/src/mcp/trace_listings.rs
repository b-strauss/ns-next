//! A trace file's listing record read back into the view `listing.rs`
//! writes every listing from (`docs/design/debugger.md`, "The trace
//! tools"), so that `debug_trace_listing`'s window reads as `--print-*`
//! writes the same listing, line for line: one layout, one writer.

use ns_runtime::bytecode;
use ns_runtime::trace_events::Position;
use serde_json::Value;

use crate::listing::{
    CodeView, InliningView, View, ViewArm, ViewArmKind, ViewBody, ViewDecline, ViewInstruction,
    ViewOutcome, ViewScope,
};

/// The lines of `record`, a `bytecode`, `code` or `inlining` record, head
/// and end included, as `--print-*` writes the listing it records: every
/// body named by `names`, and each scope's position lines given their
/// instruction from whichever of `bytecode`, the `bytecode` records of the
/// scopes' families, lists the scope's body -- `@<pc>` alone where none
/// does. A field the record lacks is read as the text listing would write
/// its absence; a record of no kind of listing has no lines.
pub(crate) fn rendered(
    record: &Value,
    bytecode: &[Value],
    names: &dyn Fn(u32) -> String,
) -> Vec<String> {
    view_of(record, bytecode, names).map_or_else(Vec::new, |view| view.lines())
}

/// `record` as a listing's view.
fn view_of(record: &Value, bytecode: &[Value], names: &dyn Fn(u32) -> String) -> Option<View> {
    let name = names(number(&record["body"])?);
    match record["kind"].as_str()? {
        "bytecode" => Some(View::Bytecode {
            name,
            bodies: array(&record["bodies"])
                .iter()
                .filter_map(body_of)
                .collect(),
        }),
        "code" => Some(View::Code(CodeView {
            name,
            tier: text(&record["tier"]),
            architecture: text(&record["architecture"]),
            key: key_of(record),
            base: record["base"].as_u64().unwrap_or(0),
            bytes: record["bytes"].as_u64().unwrap_or(0),
            scopes: scopes_of(record, bytecode, names),
            instructions: array(&record["instructions"])
                .iter()
                .map(instruction_of)
                .collect(),
            data: array(&record["data"])
                .iter()
                .map(|word| (word["offset"].as_u64().unwrap_or(0), text(&word["word"])))
                .collect(),
            mark: number(&record["mark"]),
        })),
        "inlining" => Some(View::Inlining(InliningView {
            name,
            key: key_of(record),
            scopes: scopes_of(record, bytecode, names),
            arms: array(&record["arms"])
                .iter()
                .map(|arm| ViewArm {
                    scope: number(&arm["scope"]).unwrap_or(0),
                    program_counter: number(&arm["at"]).unwrap_or(0),
                    receiver: text(&arm["receiver"]),
                    inlined: match number(&arm["inlined"]["scope"]) {
                        Some(scope) => ViewArmKind::Scope(scope),
                        None => ViewArmKind::Always(text(&arm["inlined"]["always"])),
                    },
                })
                .collect(),
            declined: array(&record["declined"])
                .iter()
                .map(|declined| ViewDecline {
                    scope: number(&declined["scope"]).unwrap_or(0),
                    program_counter: number(&declined["at"]).unwrap_or(0),
                    callee: number(&declined["callee"]).map(names),
                    reason: text(&declined["reason"]),
                })
                .collect(),
            outcome: match record["outcome"]["installed"].as_u64() {
                Some(bytes) => ViewOutcome::Installed(usize::try_from(bytes).unwrap_or(usize::MAX)),
                None => ViewOutcome::Refused(text(&record["outcome"]["refused"])),
            },
        })),
        _ => None,
    }
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn number(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|number| u32::try_from(number).ok())
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

/// A copy's key and the application it entered, where it has a key.
fn key_of(record: &Value) -> Option<(String, Option<String>)> {
    let key = record["key"].as_str()?;
    Some((
        key.to_string(),
        record["entered"].as_str().map(ToString::to_string),
    ))
}

/// A body of a `bytecode` record.
fn body_of(body: &Value) -> Option<ViewBody> {
    let arguments = body["arguments"]
        .as_u64()
        .and_then(|count| u16::try_from(count).ok());
    let registers = body["registers"]
        .as_u64()
        .and_then(|count| u16::try_from(count).ok());
    Some(ViewBody {
        number: number(&body["body"])?,
        kind: text(&body["kind"]),
        made_at: number(&body["in"]).zip(number(&body["at"])),
        counts: arguments.zip(registers),
        instructions: array(&body["instructions"])
            .iter()
            .map(|instruction| {
                (
                    number(&instruction["index"]).unwrap_or(0),
                    number(&instruction["statement"]),
                    text(&instruction["text"]),
                )
            })
            .collect(),
    })
}

/// The scopes of a `code` or `inlining` record, each with its body's
/// instructions: the scope's own `lines`, where the record carries them
/// for a body no earlier record lists, or those of the one of `bytecode`
/// that lists the body.
fn scopes_of(record: &Value, bytecode: &[Value], names: &dyn Fn(u32) -> String) -> Vec<ViewScope> {
    array(&record["scopes"])
        .iter()
        .map(|scope| {
            let body = number(&scope["body"]).unwrap_or(0);
            let parent = array(&scope["parent"]);
            ViewScope {
                number: number(&scope["scope"]).unwrap_or(0),
                body: names(body),
                parent: parent
                    .first()
                    .and_then(number)
                    .zip(parent.get(1).and_then(number)),
                instructions: match scope["lines"].as_array() {
                    Some(lines) => lines.iter().map(text).collect(),
                    None => instructions_of(body, bytecode),
                },
            }
        })
        .collect()
}

/// The body `body`'s instruction lines, as the bytecode listing writes
/// them, from the first of `bytecode` that lists it.
fn instructions_of(body: u32, bytecode: &[Value]) -> Vec<String> {
    bytecode
        .iter()
        .flat_map(|record| array(&record["bodies"]))
        .find(|listed| number(&listed["body"]) == Some(body))
        .and_then(body_of)
        .map(|listed| {
            listed
                .instructions
                .iter()
                .map(|(index, statement, text)| {
                    bytecode::listed_line(*index, *statement, text, false)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An instruction of a `code` record, with the positions reached at it:
/// those `passed` names, then the one its stretch stands in.
fn instruction_of(instruction: &Value) -> ViewInstruction {
    let mut reached: Vec<Position> = array(&instruction["passed"])
        .iter()
        .filter_map(position_of)
        .collect();
    reached.extend(position_of(instruction));
    ViewInstruction {
        offset: number(&instruction["offset"]).unwrap_or(0),
        text: text(&instruction["text"]),
        reached,
        // The record joins two annotations with `; `, which no
        // annotation's own text holds.
        annotations: instruction["annotation"]
            .as_str()
            .map(|annotation| annotation.split("; ").map(ToString::to_string).collect())
            .unwrap_or_default(),
    }
}

/// The position `value` names: out of line, a scope and program counter,
/// or none.
fn position_of(value: &Value) -> Option<Position> {
    if value["out-of-line"].as_bool() == Some(true) {
        return Some(Position::OutOfLine);
    }
    Some(Position::Instruction {
        scope: number(&value["scope"])?,
        program_counter: number(&value["at"])?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use indoc::indoc;
    use ns_runtime::bytecode::listed_line;
    use ns_runtime::trace_events::{
        Annotation, Architecture, ArmKind, BodyName, BytecodeListing, CodeListing, CodeTier,
        CompileRefusal, DataWord, InlineDecline, InliningListing, InliningOutcome, ListedArm,
        ListedBody, ListedDecline, ListedInstruction, ListedScope, Listing, MadeAt, Position,
    };
    use ns_runtime::{
        HostResult, HostSignal, Interpreter, Printing, Root, SourceOrigin, Symbol, TraceCategories,
        TraceEvent, TraceFilter, TraceSettings, TraceSink,
    };
    use serde_json::Value;

    use super::rendered;
    use crate::listing;
    use crate::trace_file::listing_record;

    /// The bodies the fixtures number, as the trace names them.
    pub(crate) const NAMES: [(u32, &str); 4] = [
        (1, "Demo>>run"),
        (2, "[] in Demo>>run"),
        (3, "[] in Demo>>run"),
        (4, "Demo>>square:"),
    ];

    fn named(number: u32) -> BodyName {
        BodyName {
            number,
            name: name_of(number),
        }
    }

    pub(crate) fn name_of(number: u32) -> String {
        NAMES
            .iter()
            .find(|(known, _)| *known == number)
            .map_or_else(|| format!("#{number}"), |(_, name)| (*name).to_string())
    }

    /// A body of a bytecode listing, its instructions `(statement, text)`
    /// by index.
    fn listed(
        number: u32,
        kind: &'static str,
        made_at: Option<(u32, u32)>,
        counts: (u16, u16),
        instructions: &[(Option<u32>, &str)],
    ) -> ListedBody {
        ListedBody {
            body: named(number),
            kind,
            made_at: made_at.map(|(maker, index)| MadeAt { maker, index }),
            argument_count: counts.0,
            register_count: counts.1,
            instructions: instructions
                .iter()
                .zip(0u32..)
                .map(|((statement, text), index)| ListedInstruction {
                    index,
                    statement: *statement,
                    text: (*text).to_string(),
                })
                .collect(),
        }
    }

    /// `Demo>>run`'s bytecode: a block it makes, which makes a block.
    pub(crate) fn run_bytecode() -> BytecodeListing {
        BytecodeListing {
            body: named(1),
            text: indoc! {"
                method Demo>>run (args 0, registers 3)
                   0   stmt 0  LoadSelf r0
                   1   stmt 0  MakeClosure r1, [] -> copying r0
                        block block (args 0, registers 2)
                           0   stmt 0  MakeClosure r1, [] -> copying
                                block block (args 0, registers 1)
                                   0   stmt 0  Return r0
                           1   stmt 0  Return r1
                   2        -  Nothing
                   3   stmt 1  Return r1
            "}
            .to_string(),
            bodies: vec![
                listed(
                    1,
                    "method",
                    None,
                    (0, 3),
                    &[
                        (Some(0), "LoadSelf r0"),
                        (Some(0), "MakeClosure r1, [] -> copying r0"),
                        (None, "Nothing"),
                        (Some(1), "Return r1"),
                    ],
                ),
                listed(
                    2,
                    "block",
                    Some((1, 1)),
                    (0, 2),
                    &[
                        (Some(0), "MakeClosure r1, [] -> copying"),
                        (Some(0), "Return r1"),
                    ],
                ),
                listed(3, "block", Some((2, 0)), (0, 1), &[(Some(0), "Return r0")]),
            ],
        }
    }

    /// `Demo>>square:`'s bytecode, a body without blocks.
    pub(crate) fn square_bytecode() -> BytecodeListing {
        BytecodeListing {
            body: named(4),
            text: indoc! {"
                method Demo>>square: (args 1, registers 2)
                   0        -  Nothing
                   1   stmt 0  Multiply r1 <- r0, r0
                   2   stmt 0  Return r1
            "}
            .to_string(),
            bodies: vec![listed(
                4,
                "method",
                None,
                (1, 2),
                &[
                    (None, "Nothing"),
                    (Some(0), "Multiply r1 <- r0, r0"),
                    (Some(0), "Return r1"),
                ],
            )],
        }
    }

    /// The scope `number` of `body`, inlined at `parent`, with the lines
    /// of `bytecode`'s body of that number.
    fn scope(
        number: u32,
        body: u32,
        parent: Option<(u32, u32)>,
        bytecode: &BytecodeListing,
    ) -> ListedScope {
        let instructions = bytecode
            .bodies
            .iter()
            .find(|listed| listed.body.number == body)
            .map(|listed| {
                listed
                    .instructions
                    .iter()
                    .map(|instruction| {
                        listed_line(
                            instruction.index,
                            instruction.statement,
                            &instruction.text,
                            false,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        ListedScope {
            number,
            body: named(body),
            parent,
            instructions,
        }
    }

    /// An architecture this binary has no decoder of, whose bytes every
    /// target lists in hex.
    fn foreign_architecture() -> Architecture {
        if cfg!(target_arch = "x86_64") {
            Architecture::Aarch64
        } else {
            Architecture::X64
        }
    }

    /// `square:`'s baseline code of `length` bytes, a stopped frame's:
    /// two positions at one instruction, two annotations on one, a stretch
    /// out of line, the mark and a data word.
    pub(crate) fn square_code(length: u32) -> CodeListing {
        let instruction = |program_counter| Position::Instruction {
            scope: 0,
            program_counter,
        };
        CodeListing {
            body: named(4),
            tier: CodeTier::Baseline,
            key: None,
            architecture: foreign_architecture(),
            base_address: 0x7ff0_2000,
            bytes: (0..length + 8)
                .map(|byte| u8::try_from(byte % 256).unwrap_or(0))
                .collect(),
            code_end: length,
            words: vec![(length, DataWord::Budget)],
            positions: vec![
                (0, instruction(0)),
                (0, instruction(1)),
                (4, instruction(2)),
                (8, Position::OutOfLine),
            ],
            annotations: vec![
                (0, Annotation::Leaf("miss")),
                (2, Annotation::Cell { site: 0 }),
                (
                    12,
                    Annotation::Exit {
                        kind: "send",
                        program_counter: 1,
                    },
                ),
            ],
            scopes: vec![scope(0, 4, None, &square_bytecode())],
            mark: Some(4),
        }
    }

    /// `run`'s optimized copy, `square:` inlined into it.
    pub(crate) fn run_optimized() -> CodeListing {
        CodeListing {
            body: named(1),
            tier: CodeTier::Optimized,
            key: Some(("Demo".to_string(), Some("Demo/Base".to_string()))),
            architecture: foreign_architecture(),
            base_address: 0x7ff0_3000,
            bytes: (0u8..32).collect(),
            code_end: 16,
            words: vec![
                (16, DataWord::DispatchingRows),
                (24, DataWord::TwinTransfer { program_counter: 3 }),
            ],
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
                (12, Position::OutOfLine),
            ],
            annotations: vec![(4, Annotation::Target("twin"))],
            scopes: vec![
                scope(0, 1, None, &run_bytecode()),
                scope(1, 4, Some((0, 1)), &square_bytecode()),
            ],
            mark: None,
        }
    }

    pub(crate) fn run_inlining(outcome: InliningOutcome) -> InliningListing {
        InliningListing {
            body: named(1),
            key: Some(("Demo".to_string(), None)),
            scopes: vec![
                scope(0, 1, None, &run_bytecode()),
                scope(1, 4, Some((0, 1)), &square_bytecode()),
            ],
            arms: vec![
                ListedArm {
                    scope: 0,
                    program_counter: 1,
                    receiver: "Demo".to_string(),
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
                    program_counter: 3,
                    callee: Some(named(4)),
                    reason: InlineDecline::OverWeight,
                },
                ListedDecline {
                    scope: 1,
                    program_counter: 0,
                    callee: None,
                    reason: InlineDecline::NotExpressed,
                },
            ],
            outcome,
        }
    }

    /// The `bytecode` records of the fixtures' two families.
    fn bytecode_records() -> Vec<Value> {
        vec![
            listing_record(&Listing::Bytecode(run_bytecode())),
            listing_record(&Listing::Bytecode(square_bytecode())),
        ]
    }

    /// The lines `--print-*` writes for `listing`.
    pub(crate) fn printed(listing: &Listing) -> Vec<String> {
        listing::text(listing)
            .lines()
            .map(ToString::to_string)
            .collect()
    }

    /// Every kind of listing read back from its record is written as
    /// `--print-*` writes the listing, line for line, its head and end
    /// included: the bytecode's blocks nested two deep, a baseline copy's
    /// positions where one instruction reaches two, its annotations, its
    /// stretch out of line, its mark and its data, an optimized copy's
    /// key and scope tree, and an inlining plan both installed and
    /// refused.
    #[test]
    fn a_record_is_rendered_as_its_listing_is_printed() {
        let bytecode = bytecode_records();
        let listings = [
            Listing::Bytecode(run_bytecode()),
            Listing::Bytecode(square_bytecode()),
            Listing::Code(square_code(16)),
            Listing::Code(run_optimized()),
            Listing::Inlining(run_inlining(InliningOutcome::Installed { bytes: 400 })),
            Listing::Inlining(run_inlining(InliningOutcome::Refused(
                CompileRefusal::ZoneFull,
            ))),
        ];
        for listing in &listings {
            let record = listing_record(listing);
            assert_eq!(
                rendered(&record, &bytecode, &name_of),
                printed(listing),
                "{record}"
            );
        }
        // Both positions at the first instruction are named, the record's
        // `passed` keeping the first of them.
        let square = printed(&Listing::Code(square_code(16)));
        assert_eq!(
            square[1..4],
            [
                "  @0  -  Nothing",
                "  @1  stmt 0  Multiply r1 <- r0, r0",
                "    +0x0000  00 01 02 03  ; leaf miss  ; cell site 0",
            ]
        );
        let run = printed(&Listing::Bytecode(run_bytecode()));
        assert_eq!(
            run[6], "                block block (args 0, registers 1)",
            "{run:?}"
        );
    }

    #[cfg(target_arch = "x86_64")]
    /// The plugin's fixture of listings (`ListingsFixtureTest`), from the
    /// repository's root: each listing's record, the bodies it names, the
    /// bytecode records its position lines read, and the text `--print-*`
    /// writes of it, which the plugin's listings are held to.
    const LISTINGS_FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/jetbrains/src/test/resources/devtool/listings.jsonl"
    );

    #[cfg(target_arch = "x86_64")]
    /// The fixture's lines: the bytecode listings and the copies of
    /// machine code, a baseline one with positions `passed` among them.
    fn listings_fixture() -> String {
        let bodies: Vec<Value> = NAMES
            .iter()
            .map(|(number, name)| serde_json::json!({ "body": number, "name": name }))
            .collect();
        let bytecode = bytecode_records();
        let mut fixture = String::new();
        for listing in [
            Listing::Bytecode(run_bytecode()),
            Listing::Bytecode(square_bytecode()),
            Listing::Code(square_code(16)),
            Listing::Code(run_optimized()),
        ] {
            let entry = serde_json::json!({
                "bodies": bodies,
                "bytecode": bytecode,
                "record": listing_record(&listing),
                "text": listing::text(&listing),
            });
            fixture.push_str(&entry.to_string());
            fixture.push('\n');
        }
        fixture
    }

    // Its machine code is in hex as an x64 binary writes a foreign
    // architecture's: the fixture is the same on every x64 host alone.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn the_listings_fixture_is_current() {
        let written = std::fs::read_to_string(LISTINGS_FIXTURE)
            .unwrap_or_else(|error| panic!("{LISTINGS_FIXTURE}: {error}"));
        // A checkout may have given the file CRLF line ends.
        assert!(
            written.replace("\r\n", "\n") == listings_fixture(),
            "{LISTINGS_FIXTURE} is stale: run `cargo test -p ns-cli --bins regenerate_the_listings_fixture -- --ignored`"
        );
    }

    // Its machine code is in hex as an x64 binary writes a foreign
    // architecture's: the fixture is the same on every x64 host alone.
    #[cfg(target_arch = "x86_64")]
    #[test]
    #[ignore = "regenerates the plugin's listings fixture; run deliberately"]
    fn regenerate_the_listings_fixture() {
        std::fs::write(LISTINGS_FIXTURE, listings_fixture()).expect("write the listings fixture");
    }

    /// Where the run listed no bytecode, a position line names its
    /// program counter alone.
    #[test]
    fn a_position_without_bytecode_names_its_program_counter_alone() {
        let lines = rendered(
            &listing_record(&Listing::Code(square_code(16))),
            &[],
            &name_of,
        );
        assert_eq!(lines[1..3], ["  @0", "  @1"]);
        assert_eq!(lines[5], "  ->+0x0004  04 05 06 07");
    }

    /// A file whose records listed no bytecode of a scope's body -- one no
    /// mixin holds, or one the print filter or the printing left out --
    /// carries its lines in the scope, which the listing is written by as
    /// `--print-code` writes it; once a record lists the body, the scope
    /// carries none.
    #[test]
    fn a_scope_no_bytecode_record_lists_carries_its_lines() {
        let code = Listing::Code(square_code(16));
        let mut listed = crate::trace_file::ListedBodies::default();
        let alone = listed.record(&code);
        let Listing::Code(square) = &code else {
            unreachable!("a code listing")
        };
        assert_eq!(
            alone["scopes"][0]["lines"],
            serde_json::json!(square.scopes[0].instructions)
        );
        assert_eq!(rendered(&alone, &[], &name_of), printed(&code));
        listed.record(&Listing::Bytecode(square_bytecode()));
        let after = listed.record(&code);
        assert_eq!(after["scopes"][0].get("lines"), None, "{after}");
    }

    /// A sink that keeps every listing it receives.
    struct Listings(Rc<RefCell<Vec<Listing>>>);

    impl TraceSink for Listings {
        fn event(&mut self, _event: &TraceEvent) {}

        fn listing(&mut self, listing: &Listing) {
            self.0.borrow_mut().push(listing.clone());
        }
    }

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// A loop hot enough to compile `run`'s blocks and, run again and
    /// again, optimize them and `twice:`, whose branches and loops lay out
    /// stretches out of line; the loop's block makes a block, which makes
    /// a block.
    const HOT: &str = indoc! {"
        class Hot = Object new () (
          public run = (
            | sum ::= 0. |
            1 to: 3000 do: [:index |
              sum:: sum + (self twice: index).
              [:each | [each + 1] value] value: index].
            ^sum.
          )

          twice: number = (
            | other ::= number. |
            number > 5 ifFalse: [other:: 1].
            [other > 100] whileFalse: [other:: other + 50].
            number isNil ifTrue: [^0].
            ^number * 2.
          )
        )
    "};

    /// Every listing of a real run -- the core library's and the
    /// program's bytecode, and on a target with a JIT every copy of
    /// machine code and every inlining plan -- read back from its record,
    /// as the trace file writes the run's records in order, is written as
    /// `--print-*` writes it: a body no bytecode record lists, the
    /// host's own factory among them, by the lines its scope carries.
    #[test]
    fn every_listing_of_a_run_is_rendered_as_it_is_printed() {
        let module = ns_syntax::parse(HOT).expect("parse");
        let resolved = ns_runtime::resolve(module, HOT.into(), SourceOrigin::File("Hot.ns".into()))
            .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        interpreter.set_jit_enabled(true);
        // Off whatever the environment says: a body the breakpoint or step
        // stress mode patched loses its machine code at every statement,
        // and the run would list none to compare.
        #[cfg(debug_assertions)]
        interpreter.set_stress_breakpoints(false);
        #[cfg(debug_assertions)]
        interpreter.set_stress_step(false);
        let listings = Rc::new(RefCell::new(Vec::new()));
        interpreter.set_trace(
            TraceSettings {
                categories: TraceCategories::default(),
                filter: TraceFilter::everything(),
                printing: Printing {
                    bytecode: true,
                    code: true,
                    inlining: true,
                },
                print_filter: TraceFilter::everything(),
                describe_bodies: false,
            },
            Box::new(Listings(Rc::clone(&listings))),
        );
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..20 {
            expect_ok(
                interpreter.send(&instance, Symbol::intern("run"), &[]),
                "run",
            );
        }
        drop(interpreter);
        let listings = listings.borrow();
        let mut names: HashMap<u32, String> = HashMap::new();
        let mut name = |body: &BodyName| {
            names.insert(body.number, body.name.clone());
        };
        let mut bytecode = Vec::new();
        for listing in listings.iter() {
            match listing {
                Listing::Bytecode(listed) => {
                    name(&listed.body);
                    listed.bodies.iter().for_each(|body| name(&body.body));
                    bytecode.push(listing_record(listing));
                }
                Listing::Code(code) => {
                    name(&code.body);
                    code.scopes.iter().for_each(|scope| name(&scope.body));
                }
                Listing::Inlining(inlining) => {
                    name(&inlining.body);
                    inlining.scopes.iter().for_each(|scope| name(&scope.body));
                    inlining
                        .declined
                        .iter()
                        .filter_map(|declined| declined.callee.as_ref())
                        .for_each(&mut name);
                }
            }
        }
        let names_of = |number: u32| names.get(&number).cloned().unwrap_or_default();
        let mut machine_code = 0;
        let mut listed = crate::trace_file::ListedBodies::default();
        for listing in listings.iter() {
            if !matches!(listing, Listing::Bytecode(_)) {
                machine_code += 1;
            }
            let record = listed.record(listing);
            assert_eq!(
                rendered(&record, &bytecode, &names_of),
                printed(listing),
                "{record}"
            );
        }
        assert!(
            listings.iter().any(|listing| matches!(
                listing,
                Listing::Bytecode(listed) if listed.body.name == "Hot>>run"
                    && listed.bodies.len() == 4
            )),
            "no bytecode of Hot>>run with its three blocks"
        );
        // Machine code only where the binary has a JIT.
        assert_eq!(
            machine_code > 0,
            crate::trace_file::architecture().is_some(),
            "{machine_code} listings of machine code"
        );
    }
}
