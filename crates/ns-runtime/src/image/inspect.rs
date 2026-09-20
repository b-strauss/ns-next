//! A read-only structural view of an artifact: where each section starts and
//! ends, what the symbol and mixin tables hold, and how much graph there is.
//!
//! This exists because an artifact is otherwise opaque. `disasm` shows one
//! compiled body; nothing else shows the file. Two things need that. A failed
//! prefix-equality check between a `build` and a `snapshot` of the same source
//! reports a differing byte offset, which says nothing on its own and
//! everything once the offset is attributed to a section. And the claim that
//! the platform is cut out of an artifact -- the reason an image is kilobytes
//! rather than megabytes -- is otherwise checkable only by reading the encoder.
//!
//! Nothing here bootstraps an interpreter, allocates a class object, or runs
//! anything -- inspecting a hostile file exercises only the decoders below,
//! never `Interpreter::bootstrap`.

use std::fmt::Write as _;

use crate::bytecode::{CodeKind, CompiledCode};
use std::rc::Rc;

use crate::image::{decode, mixins, opcodes, symbols};

/// One named span of the file. `end` is exclusive.
pub struct Section {
    pub name: &'static str,
    pub start: usize,
    pub end: usize,
    /// A short count or value for the section, empty when it has none.
    pub detail: String,
}

pub struct Report {
    pub format_version: u32,
    pub has_source: bool,
    pub stateless: bool,
    pub origin: String,
    pub sections: Vec<Section>,
    pub symbols: Vec<String>,
    /// One line per code id, in id order, naming the declaration that owns it
    /// and what it is. This is the bridge between the two views: it tells a
    /// reader which body to ask `disasm` for.
    pub code: Vec<String>,
    /// One line per mixin row: name, slot count, factory, method count.
    pub mixins: Vec<String>,
    /// The same rows expanded: slot names rather than a count, each method
    /// against the code id that holds it, the nested declarations by name.
    /// Rendered only by `render_full`.
    pub mixin_detail: Vec<String>,
    /// Every body disassembled, in code-id order. Rendered only by
    /// `render_full`.
    pub bodies: Vec<String>,
    /// The object graph, read record by record rather than rebuilt. Empty for
    /// a stateless binary, which carries none.
    pub graph_objects: Vec<String>,
    pub graph_frames: Vec<String>,
    pub graph_roots: Vec<String>,
    /// The identity hashes the graph section records for the objects it
    /// names by tag. Rendered only by `render_full`.
    pub graph_tag_named_hashes: Vec<String>,
    pub entry: Option<String>,
    pub total_length: usize,
}

/// Attributes each code id to the declaration that owns it.
///
/// A mixin row names its methods, its initializer and its two superclass-clause
/// bodies by id, so those attribute directly. A block body is named by no row
/// at all -- it is reachable only from the `MakeClosure` instruction that
/// creates it -- so it is labelled by its kind instead. Anything left over
/// would be a code id no declaration claims and no body's kind explains, which
/// is worth showing rather than hiding: `(unnamed)` in the output means the
/// code table and the mixin table disagree.
///
/// Returned as `(owner, what)` rather than one string because the two views
/// want different halves: the code table prints both, while a body's own
/// heading prints only the owner -- `disassemble` already states the kind, and
/// "initializer code 1 Arithmetic initializer" would say it twice.
fn attribute_code(decoded: &mixins::Decoded, codes: &[Rc<CompiledCode>]) -> Vec<(String, String)> {
    let mut labels = vec![(String::new(), String::new()); codes.len()];
    let mut claim = |id: u32, owner: &str, what: &str| {
        if let Some(slot) = labels.get_mut(id as usize) {
            *slot = (owner.to_string(), what.to_string());
        }
    };
    for (mixin, code) in decoded.mixins.iter().zip(decoded.code.iter()) {
        let owner = mixin.name.to_string();
        for (selector, id) in &code.methods {
            claim(*id, &owner, &selector.to_string());
        }
        if let Some(id) = code.initializer {
            claim(id, &owner, "initializer");
        }
        if let Some(id) = code.superclass_clause {
            claim(id, &owner, "superclass clause");
        }
        if let Some(id) = code.superclass_message {
            claim(id, &owner, "superclass message");
        }
    }
    for (label, body) in labels.iter_mut().zip(codes.iter()) {
        if label.0.is_empty() && label.1.is_empty() {
            label.1 = match body.kind {
                CodeKind::Block => "block body".to_string(),
                _ => "(unnamed)".to_string(),
            };
        }
    }
    labels
}

/// A mixin row, expanded: everything the table records about one declaration,
/// with each body named by the code id that holds it so the two views join up.
fn expand_mixin(mixin: &Rc<crate::mixin::Mixin>, code: &mixins::MixinCode) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} ({:?})", mixin.name, mixin.access);
    match &mixin.factory {
        Some(factory) => {
            let _ = writeln!(
                out,
                "      factory   {}/{}",
                factory.selector, factory.argument_count
            );
        }
        None => {
            let _ = writeln!(out, "      factory   -");
        }
    }
    if mixin.slots.is_empty() {
        let _ = writeln!(out, "      slots     -");
    } else {
        let names: Vec<String> = mixin
            .slots
            .iter()
            .map(|slot| slot.name.to_string())
            .collect();
        let _ = writeln!(out, "      slots     {}", names.join(", "));
    }
    for (selector, id) in &code.methods {
        let _ = writeln!(out, "      method    {selector} -> code {id}");
    }
    for (what, id) in [
        ("initializer", code.initializer),
        ("superclass clause", code.superclass_clause),
        ("superclass message", code.superclass_message),
    ] {
        if let Some(id) = id {
            let _ = writeln!(out, "      {what} -> code {id}");
        }
    }
    for nested in &mixin.nested {
        let _ = writeln!(out, "      nested    {} ({:?})", nested.name, nested.access);
    }
    if mixin.class_side.is_some() {
        // Naming it would print "class side" twice: every class-side mixin
        // carries that as its name. It is a row of this table in its own
        // right, so its detail is one row further down.
        let _ = writeln!(out, "      class side  present");
    }
    out
}

/// The object graph, read rather than rebuilt.
///
/// `decode_graph_section` materializes: it allocates real objects, resolves
/// class tags against live builtins and therefore needs a `Mutator`. A
/// viewer must not do that -- `javap` does not instantiate a class to print
/// it -- so this walks the same bytes and reports each record instead.
///
/// It is a second reader over one layout, which is a drift risk taken with
/// eyes open. Two things hold it: the tags it interprets are the shared
/// constants in `opcodes.rs`, not a private copy, so it cannot disagree with
/// the loader about what a byte *means*; and `graph_walk_consumes_the_whole_
/// section` pins that it ends exactly at the file's end, so any disagreement
/// about record *shape* desynchronizes the walk and fails loudly rather than
/// printing plausible nonsense.
struct GraphView {
    objects: Vec<String>,
    frames: Vec<String>,
    roots: Vec<String>,
    tag_named: Vec<String>,
}

fn read_class_tag(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    Ok(match reader.read_u8()? {
        opcodes::CLASS_BUILTIN => format!("{:?}", decode::read_builtin_class(reader)?),
        opcodes::CLASS_USER_ROOT => "<root class>".to_string(),
        opcodes::CLASS_RECORD => format!("object {}", reader.read_u32()?),
        tag => {
            return Err(decode::DecodeError::InvalidTag {
                what: "class tag",
                tag,
            })
        }
    })
}

fn read_optional_class(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    if reader.read_boolean()? {
        read_class_tag(reader)
    } else {
        Ok("-".to_string())
    }
}

fn read_frame_reference(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    Ok(if reader.read_boolean()? {
        format!("frame {}", reader.read_u32()?)
    } else {
        "-".to_string()
    })
}

fn read_value(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    Ok(match reader.read_u8()? {
        opcodes::VALUE_NIL => "nil".to_string(),
        opcodes::VALUE_FALSE => "false".to_string(),
        opcodes::VALUE_TRUE => "true".to_string(),
        opcodes::VALUE_SMALL_INTEGER => reader.read_i64()?.to_string(),
        opcodes::VALUE_FLOAT => reader.read_f64()?.to_string(),
        opcodes::VALUE_OBJECT_ID => format!("object {}", reader.read_u32()?),
        opcodes::VALUE_CLASS_TAG => format!("class {}", read_class_tag(reader)?),
        opcodes::VALUE_PLATFORM => "<platform>".to_string(),
        tag => return Err(decode::DecodeError::InvalidTag { what: "value", tag }),
    })
}

fn read_values(reader: &mut decode::Reader) -> Result<Vec<String>, decode::DecodeError> {
    let count = decode::bounded_count(reader.read_u32()? as usize, 1, reader)?;
    (0..count).map(|_| read_value(reader)).collect()
}

fn read_closure_record(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    let code = reader.read_u32()?;
    let captured = reader.read_u32()?;
    let count = reader.read_u16()? as usize;
    let mut copied = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        copied.push(read_value(reader)?);
    }
    Ok(format!(
        "code {code}, captured frame {captured}, copied [{}]",
        copied.join(", ")
    ))
}

fn walk_graph(
    reader: &mut decode::Reader,
    symbols: &symbols::Table,
) -> Result<GraphView, decode::DecodeError> {
    let object_count = decode::bounded_count(reader.read_u32()? as usize, 1, reader)?;
    let frame_count = decode::bounded_count(reader.read_u32()? as usize, 1, reader)?;

    let mut objects = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        let class = read_class_tag(reader)?;
        let hash = reader.read_u32()?;
        let body = match reader.read_u8()? {
            opcodes::OBJECT_INSTANCE => format!("instance {{{}}}", read_values(reader)?.join(", ")),
            opcodes::OBJECT_STRING => format!("string {:?}", reader.read_str()?),
            opcodes::OBJECT_SYMBOL => format!("symbol #{}", symbols.get(reader.read_u32()?)?),
            opcodes::OBJECT_ARRAY => format!("array [{}]", read_values(reader)?.join(", ")),
            opcodes::OBJECT_CLOSURE => format!("closure {}", read_closure_record(reader)?),
            opcodes::OBJECT_CLASS => {
                let mixin = reader.read_u32()?;
                let superclass = read_optional_class(reader)?;
                let enclosing = if reader.read_boolean()? {
                    read_value(reader)?
                } else {
                    "-".to_string()
                };
                let declaring = read_optional_class(reader)?;
                format!(
                    "class mixin {mixin}, superclass {superclass}, enclosing {enclosing}, \
                     declaring {declaring}"
                )
            }
            tag => {
                return Err(decode::DecodeError::InvalidTag {
                    what: "object body",
                    tag,
                })
            }
        };
        objects.push(format!("class {class}, hash {hash}, {body}"));
    }

    let mut frames = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        let receiver = read_value(reader)?;
        let arguments = read_values(reader)?;
        let temporaries = read_values(reader)?;
        let program_counter = reader.read_u32()?;
        let sender = read_frame_reference(reader)?;
        let home = read_frame_reference(reader)?;
        let current_class = read_optional_class(reader)?;
        let kind = match reader.read_u8()? {
            opcodes::FRAME_METHOD => format!("method {}", symbols.get(reader.read_u32()?)?),
            opcodes::FRAME_TOP_LEVEL => "top level".to_string(),
            opcodes::FRAME_INITIALIZER => format!("initializer of {}", read_class_tag(reader)?),
            opcodes::FRAME_SUPERCLASS_CLAUSE => {
                format!("superclass clause of {}", read_class_tag(reader)?)
            }
            opcodes::FRAME_SUPERCLASS_MESSAGE => {
                format!("superclass message of {}", read_class_tag(reader)?)
            }
            opcodes::FRAME_CLOSURE => format!("closure object {}", reader.read_u32()?),
            tag => {
                return Err(decode::DecodeError::InvalidTag {
                    what: "frame kind",
                    tag,
                })
            }
        };
        let uncontinuable = reader.read_boolean()?;
        frames.push(format!(
            "{kind}, receiver {receiver}, args [{}], temps [{}], pc {program_counter}, \
             sender {sender}, home {home}, current class {current_class}, \
             uncontinuable {uncontinuable}",
            arguments.join(", "),
            temporaries.join(", ")
        ));
    }

    let roots = read_values(reader)?;

    let count = decode::bounded_count(reader.read_u32()? as usize, 1, reader)?;
    let mut tag_named = Vec::new();
    for _ in 0..count {
        let who = match reader.read_u8()? {
            opcodes::HASHED_BUILTIN_CLASS => format!("{:?}", decode::read_builtin_class(reader)?),
            opcodes::HASHED_ROOT_CLASS => "<root class>".to_string(),
            opcodes::HASHED_PLATFORM => "<platform>".to_string(),
            tag => {
                return Err(decode::DecodeError::InvalidTag {
                    what: "tag-named hash",
                    tag,
                })
            }
        };
        tag_named.push(format!("{who} hash {}", reader.read_u32()?));
    }

    Ok(GraphView {
        objects,
        frames,
        roots,
        tag_named,
    })
}

/// Decodes `bytes` far enough to describe it. Every failure is an `Err`, never
/// a panic -- an artifact is untrusted input here exactly as it is in `load`.
pub fn inspect(bytes: &[u8]) -> Result<Report, String> {
    let mut reader = decode::Reader::new(bytes);
    let mut sections = Vec::new();
    let mut offset = 0usize;

    let header = decode::read_header(&mut reader).map_err(|error| error.to_string())?;
    sections.push(Section {
        name: "header",
        start: offset,
        end: reader.position(),
        detail: String::new(),
    });
    offset = reader.position();

    if header.has_source {
        let source = reader.read_str().map_err(|error| error.to_string())?;
        sections.push(Section {
            name: "source",
            start: offset,
            end: reader.position(),
            detail: format!("{} bytes", source.len()),
        });
        offset = reader.position();
    }

    let origin = decode::decode_source_origin(&mut reader).map_err(|error| error.to_string())?;
    sections.push(Section {
        name: "origin",
        start: offset,
        end: reader.position(),
        detail: String::new(),
    });
    offset = reader.position();

    let symbol_table = symbols::decode(&mut reader).map_err(|error| error.to_string())?;
    let symbol_list = symbol_table
        .all()
        .iter()
        .map(|symbol| symbol.to_string())
        .collect::<Vec<_>>();
    sections.push(Section {
        name: "symbols",
        start: offset,
        end: reader.position(),
        detail: format!("{} symbols", symbol_list.len()),
    });
    offset = reader.position();

    let mut table =
        decode::decode_code_table(&mut reader, &symbol_table).map_err(|error| error.to_string())?;
    // A call-site number is no longer in the artifact at all, so without this
    // the `full` disassembly below would show every send's number as the
    // same decode-time placeholder rather than what the compiler actually
    // numbered it. `number_call_site_families` needs only `lexical_parents`
    // to succeed, a strictly weaker precondition than the rest of
    // `verify_code_table` -- but it is still a precondition a hostile file
    // can fail (`SharedBlockBody`, `ClosureArityMismatch`, `WrongCodeKind`),
    // and `inspect` exists to report on exactly the files that fail
    // verification. So a numbering failure is caught here, not propagated:
    // the rest of the report (mixins, graph, every other section) still
    // renders, the bodies below still disassemble with their placeholder
    // call sites, and the code section's own detail says plainly that the
    // numbers are not real and why -- unnumbered output beats no output.
    let numbering_error = decode::number_call_site_families(&mut table).err();
    sections.push(Section {
        name: "code",
        start: offset,
        end: reader.position(),
        detail: match &numbering_error {
            None => format!("{} bodies", table.codes.len()),
            Some(error) => format!(
                "{} bodies (call-site numbers not assigned: {error})",
                table.codes.len()
            ),
        },
    });
    offset = reader.position();

    let decoded = mixins::decode(&mut reader, &symbol_table, &table.codes)
        .map_err(|error| error.to_string())?;
    let mixin_lines = decoded
        .mixins
        .iter()
        .zip(decoded.code.iter())
        .map(|(mixin, code)| {
            format!(
                "{}  slots: {}  factory: {}  methods: {}",
                mixin.name,
                mixin.slots.len(),
                match &mixin.factory {
                    Some(factory) => factory.selector.to_string(),
                    None => "-".to_string(),
                },
                code.methods.len(),
            )
        })
        .collect::<Vec<_>>();
    sections.push(Section {
        name: "mixins",
        start: offset,
        end: reader.position(),
        detail: format!("{} mixins", decoded.mixins.len()),
    });
    let attributed = attribute_code(&decoded, &table.codes);
    let code_lines = attributed
        .iter()
        .map(|(owner, what)| {
            if owner.is_empty() {
                what.clone()
            } else {
                format!("{owner}  {what}")
            }
        })
        .collect::<Vec<_>>();
    let mixin_detail = decoded
        .mixins
        .iter()
        .zip(decoded.code.iter())
        .map(|(mixin, code)| expand_mixin(mixin, code))
        .collect::<Vec<_>>();
    // Disassembled up front rather than on demand: a body is a few hundred
    // instructions at most, and carrying the strings keeps `Report` a plain
    // value that a caller can render either way without holding the decoded
    // tables alive alongside it.
    let bodies = table
        .codes
        .iter()
        .zip(attributed.iter())
        .enumerate()
        .map(|(id, (body, (owner, _)))| {
            let heading = if owner.is_empty() {
                format!("code {id}")
            } else {
                format!("code {id}  {owner}")
            };
            crate::bytecode::disassemble(body.as_ref(), &heading)
        })
        .collect::<Vec<_>>();

    offset = reader.position();

    // The root class's superclass reference. Decoding it needs a `Mutator`
    // only to resolve a builtin tag to a live class, which inspection does not
    // want; the bytes are read and their span recorded without resolving.
    decode::skip_root_superclass(&mut reader).map_err(|error| error.to_string())?;
    sections.push(Section {
        name: "root class",
        start: offset,
        end: reader.position(),
        detail: String::new(),
    });
    offset = reader.position();

    let entry =
        decode::decode_entry(&mut reader, &symbol_table).map_err(|error| error.to_string())?;
    sections.push(Section {
        name: "entry",
        start: offset,
        end: reader.position(),
        detail: match &entry {
            Some(symbol) => symbol.to_string(),
            None => "none".to_string(),
        },
    });
    offset = reader.position();

    // A stateless artifact ends here: it carries code and structure but no
    // object graph, so reading one past the entry field would run off the
    // end of the file.
    let mut graph = GraphView {
        objects: Vec::new(),
        frames: Vec::new(),
        roots: Vec::new(),
        tag_named: Vec::new(),
    };
    if !header.stateless {
        graph = walk_graph(&mut reader, &symbol_table).map_err(|error| error.to_string())?;
        sections.push(Section {
            name: "graph",
            start: offset,
            // The walk consumes the section record by record, so this is the
            // real end rather than a stand-in. That it lands on the file.s
            // last byte is what `graph_walk_consumes_the_whole_section` pins.
            end: reader.position(),
            detail: format!(
                "{} objects, {} frames, {} roots, {} tag-named hashes",
                graph.objects.len(),
                graph.frames.len(),
                graph.roots.len(),
                graph.tag_named.len()
            ),
        });
    }

    Ok(Report {
        format_version: crate::image::encode::FORMAT_VERSION,
        has_source: header.has_source,
        stateless: header.stateless,
        origin: format!("{origin:?}"),
        sections,
        symbols: symbol_list,
        code: code_lines,
        mixins: mixin_lines,
        mixin_detail,
        bodies,
        graph_objects: graph.objects,
        graph_frames: graph.frames,
        graph_roots: graph.roots,
        graph_tag_named_hashes: graph.tag_named,
        entry: entry.map(|selector| selector.to_string()),
        total_length: bytes.len(),
    })
}

pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "format version {}", report.format_version);
    let _ = writeln!(
        out,
        "stateless: {}",
        if report.stateless { "yes" } else { "no" }
    );
    let _ = writeln!(
        out,
        "source blob: {}",
        if report.has_source {
            "present"
        } else {
            "absent"
        }
    );
    let _ = writeln!(out, "origin: {}", report.origin);
    let _ = writeln!(out, "total: {} bytes", report.total_length);

    let _ = writeln!(out, "\nsections");
    for section in &report.sections {
        let _ = writeln!(
            out,
            "  {:<12} {:>8}..{:<8} {}",
            section.name, section.start, section.end, section.detail
        );
    }

    let _ = writeln!(out, "\nsymbols ({})", report.symbols.len());
    for (index, symbol) in report.symbols.iter().enumerate() {
        let _ = writeln!(out, "  {index:>4} {symbol}");
    }

    let _ = writeln!(out, "\ncode table ({} bodies)", report.code.len());
    for (index, code) in report.code.iter().enumerate() {
        let _ = writeln!(out, "  {index:>4} {code}");
    }

    let _ = writeln!(out, "\nmixins ({})", report.mixins.len());
    for (index, mixin) in report.mixins.iter().enumerate() {
        let _ = writeln!(out, "  {index:>4} {mixin}");
    }

    out
}

/// Everything the artifact says, rendered.
///
/// The default `render` is a table of contents; this is the whole book. It
/// replaces the one-line mixin rows with their expanded form and appends every
/// body disassembled, so an artifact can be followed end to end from the text
/// without opening the file again. Kept behind a flag rather than made the
/// default because a real module runs to thousands of lines -- 281 bodies for
/// one corpus module -- and the table of contents is what a reader usually
/// wants first.
pub fn render_full(report: &Report) -> String {
    let mut out = String::new();
    // The head and the sections are identical either way; only what follows
    // the section table differs, so the shared part is not duplicated here.
    let brief = render(report);
    let head_end = brief.find("\nsymbols (").unwrap_or(brief.len());
    out.push_str(&brief[..head_end]);

    let _ = writeln!(out, "\nsymbols ({})", report.symbols.len());
    for (index, symbol) in report.symbols.iter().enumerate() {
        let _ = writeln!(out, "  {index:>4} {symbol}");
    }

    let _ = writeln!(out, "\ncode table ({} bodies)", report.code.len());
    for (index, code) in report.code.iter().enumerate() {
        let _ = writeln!(out, "  {index:>4} {code}");
    }

    let _ = writeln!(out, "\nmixins ({})", report.mixin_detail.len());
    for (index, mixin) in report.mixin_detail.iter().enumerate() {
        let _ = writeln!(out, "  {index:>4} {mixin}");
    }

    // Only an image has one. A binary's whole point is that it does not, so
    // printing an empty heading there would invite the reader to look for
    // state that is deliberately absent.
    if !report.graph_objects.is_empty()
        || !report.graph_frames.is_empty()
        || !report.graph_tag_named_hashes.is_empty()
    {
        let _ = writeln!(out, "\ngraph");
        let _ = writeln!(out, "  objects ({})", report.graph_objects.len());
        for (index, object) in report.graph_objects.iter().enumerate() {
            let _ = writeln!(out, "    {index:>4} {object}");
        }
        let _ = writeln!(out, "  frames ({})", report.graph_frames.len());
        for (index, frame) in report.graph_frames.iter().enumerate() {
            let _ = writeln!(out, "    {index:>4} {frame}");
        }
        let _ = writeln!(out, "  roots  {}", report.graph_roots.join(", "));
        let _ = writeln!(
            out,
            "  tag-named hashes  {}",
            report.graph_tag_named_hashes.join(", ")
        );
    }

    let _ = writeln!(out, "\nbodies ({})", report.bodies.len());
    for body in &report.bodies {
        let _ = writeln!(out);
        out.push_str(body);
    }

    out
}
