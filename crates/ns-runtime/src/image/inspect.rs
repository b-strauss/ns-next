//! A read-only structural view of an artifact: where each section starts and
//! ends, what the symbol and mixin tables hold, and how much graph there is.
//!
//! This exists because an artifact is otherwise opaque. `disasm` shows one
//! compiled body; nothing else shows the file. Two things need that. A failed
//! prefix-equality check between two images of the same source
//! reports a differing byte offset, which says nothing on its own and
//! everything once the offset is attributed to a section. And the claim that
//! the shipped world is cut out of an artifact -- the reason an image is kilobytes
//! rather than megabytes -- is otherwise checkable only by reading the encoder.
//!
//! Nothing here bootstraps an interpreter, allocates a class object, or runs
//! anything -- inspecting a hostile file exercises only the decoders below,
//! never `Interpreter::bootstrap`.

use std::fmt::Write as _;

use crate::bytecode::{CodeKind, CompiledCode};
use std::rc::Rc;

use crate::image::shipped_names::{self, ShippedName};
use crate::image::{decode, mixins, modules, opcodes, symbols};
use crate::{ArbitraryInteger, SourceOrigin};

/// One named span of the file. `end` is exclusive.
pub struct Section {
    pub name: &'static str,
    pub start: usize,
    pub end: usize,
    /// A short count or value for the section, empty when it has none.
    pub detail: String,
}

/// One module the module section describes.
pub struct ModuleReport {
    /// Where the module came from: `embedded <name>` or the file's path.
    pub origin: String,
    /// Whether the module carries its source text.
    pub has_source: bool,
    /// Which record of the graph is its root class: `root object {id}`.
    pub root: String,
}

pub struct Report {
    pub format_version: u32,
    pub shipped_world: u64,
    pub start: bool,
    /// Every module the artifact carries, in module order.
    pub modules: Vec<ModuleReport>,
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
    /// The shipped names the graph names the binary's own objects by, in
    /// section order, each as `ShippedName::describe` writes it.
    pub shipped_names: Vec<String>,
    /// The object graph, read record by record rather than rebuilt; a
    /// frame is an object record like any other.
    pub graph_objects: Vec<String>,
    pub graph_roots: Vec<String>,
    /// The identity hashes the graph section records for the objects it
    /// names by tag. Rendered only by `render_full`.
    pub graph_tag_named_hashes: Vec<String>,
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
        for (index, id) in code.mixin_applications.iter().enumerate() {
            if let Some(id) = id {
                claim(
                    *id,
                    &owner,
                    &format!("mixin application clause {} message", index + 1),
                );
            }
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
    for (index, clause) in mixin.mixin_applications.iter().enumerate() {
        let message = match code.mixin_applications.get(index).copied().flatten() {
            Some(id) => format!(" -> code {id}"),
            None if clause.message.has_arguments => " -> (no code)".to_string(),
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "      mixin application {}  {}{message}",
            index + 1,
            clause.message.selector
        );
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
    roots: Vec<String>,
    tag_named: Vec<String>,
}

/// What the graph's tags name by index: the shipped names. A reference
/// past them is shown as such rather than refused: `inspect` reports on the
/// files that fail to load too.
struct TagTargets<'a> {
    names: &'a [ShippedName],
}

/// A shipped name reference, after its tag, as the name describes itself:
/// `Kernel Array`, `Kernel Array class`, `Kernel Kernel level 0`,
/// `CollectionsFactory instance`. An index past the section is shown as
/// such rather than refused: `inspect` reports on the files that fail to
/// load too.
fn read_shipped_name(
    reader: &mut decode::Reader,
    targets: &TagTargets<'_>,
) -> Result<String, decode::DecodeError> {
    let index = reader.read_u32()?;
    Ok(match targets.names.get(index as usize) {
        Some(name) => name.describe(),
        None => format!("<shipped name {index}: out of range>"),
    })
}

fn read_class_tag(
    reader: &mut decode::Reader,
    targets: &TagTargets<'_>,
) -> Result<String, decode::DecodeError> {
    Ok(match reader.read_u8()? {
        opcodes::CLASS_SHIPPED => read_shipped_name(reader, targets)?,
        opcodes::CLASS_RECORD => format!("object {}", reader.read_u32()?),
        tag => return Err(decode::DecodeError::InvalidTag { what: "class", tag }),
    })
}

/// A class record's body, after its tag: a class applying a row of the mixin
/// table, or a level applying a shipped class's mixin, whose enclosing
/// object and declaring class are that class's and so are not in the record.
fn read_class_record(
    reader: &mut decode::Reader,
    targets: &TagTargets<'_>,
) -> Result<String, decode::DecodeError> {
    let applied_by = |reader: &mut decode::Reader| -> Result<String, decode::DecodeError> {
        let row = reader.read_u32()?;
        let index = reader.read_u32()?;
        Ok(format!("clause {index} of row {row}"))
    };
    Ok(match reader.read_u8()? {
        opcodes::MIXIN_ROW => {
            let mixin = reader.read_u32()?;
            let superclass = read_optional_class(reader, targets)?;
            let enclosing = if reader.read_boolean()? {
                read_value(reader, targets)?
            } else {
                "-".to_string()
            };
            let declaring = read_optional_class(reader, targets)?;
            let applied = if reader.read_boolean()? {
                applied_by(reader)?
            } else {
                "-".to_string()
            };
            format!(
                "class mixin row {mixin}, superclass {superclass}, enclosing {enclosing}, \
                 declaring {declaring}, applied by {applied}"
            )
        }
        opcodes::MIXIN_SHIPPED => {
            let shipped = read_shipped_name(reader, targets)?;
            let superclass = read_optional_class(reader, targets)?;
            let applied = applied_by(reader)?;
            format!("class mixin shipped {shipped}, superclass {superclass}, applied by {applied}")
        }
        tag => {
            return Err(decode::DecodeError::InvalidTag {
                what: "mixin reference",
                tag,
            })
        }
    })
}

fn read_optional_class(
    reader: &mut decode::Reader,
    targets: &TagTargets<'_>,
) -> Result<String, decode::DecodeError> {
    if reader.read_boolean()? {
        read_class_tag(reader, targets)
    } else {
        Ok("-".to_string())
    }
}

fn read_frame_reference(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    Ok(if reader.read_boolean()? {
        format!("object {}", reader.read_u32()?)
    } else {
        "-".to_string()
    })
}

/// An `OBJECT_FRAME` body, in the order the wire has it.
fn read_frame_record(
    reader: &mut decode::Reader,
    symbols: &symbols::Table,
    targets: &TagTargets<'_>,
) -> Result<String, decode::DecodeError> {
    let kind = match reader.read_u8()? {
        opcodes::FRAME_METHOD => format!("method {}", symbols.get(reader.read_u32()?)?),
        opcodes::FRAME_TOP_LEVEL => "top level".to_string(),
        opcodes::FRAME_INITIALIZER => {
            format!("initializer of {}", read_class_tag(reader, targets)?)
        }
        opcodes::FRAME_SUPERCLASS_CLAUSE => {
            format!("superclass clause of {}", read_class_tag(reader, targets)?)
        }
        opcodes::FRAME_SUPERCLASS_MESSAGE => {
            format!("superclass message of {}", read_class_tag(reader, targets)?)
        }
        opcodes::FRAME_MIXIN_APPLICATION_MESSAGE => format!(
            "mixin application message of {}",
            read_class_tag(reader, targets)?
        ),
        opcodes::FRAME_CLOSURE => format!("closure object {}", reader.read_u32()?),
        tag => {
            return Err(decode::DecodeError::InvalidTag {
                what: "frame kind",
                tag,
            })
        }
    };
    let program_counter = reader.read_u32()?;
    let uncontinuable = reader.read_boolean()?;
    let receiver = read_value(reader, targets)?;
    let sender = read_frame_reference(reader)?;
    let home = read_frame_reference(reader)?;
    let current_class = read_optional_class(reader, targets)?;
    let registers = read_values(reader, targets)?;
    Ok(format!(
        "frame {kind}, pc {program_counter}, uncontinuable {uncontinuable}, \
         receiver {receiver}, sender {sender}, home {home}, current class {current_class}, \
         registers [{}]",
        registers.join(", ")
    ))
}

fn read_value(
    reader: &mut decode::Reader,
    targets: &TagTargets<'_>,
) -> Result<String, decode::DecodeError> {
    Ok(match reader.read_u8()? {
        opcodes::VALUE_NIL => "nil".to_string(),
        opcodes::VALUE_FALSE => "false".to_string(),
        opcodes::VALUE_TRUE => "true".to_string(),
        opcodes::VALUE_SMALL_INTEGER => reader.read_i64()?.to_string(),
        opcodes::VALUE_FLOAT => crate::numeric::float_text::shortest(reader.read_f64()?),
        opcodes::VALUE_OBJECT_ID => format!("object {}", reader.read_u32()?),
        opcodes::VALUE_CLASS_TAG => format!("class {}", read_class_tag(reader, targets)?),
        opcodes::VALUE_SHIPPED_MODULE => read_shipped_name(reader, targets)?,
        opcodes::VALUE_LARGE_INTEGER => format!("large-int {}", read_large_integer_text(reader)?),
        opcodes::VALUE_FRACTION => {
            let numerator = read_fraction_part_text(reader)?;
            let denominator = read_fraction_part_text(reader)?;
            format!("fraction {numerator}/{denominator}")
        }
        tag => return Err(decode::DecodeError::InvalidTag { what: "value", tag }),
    })
}

/// A `LargeInteger`'s digits, rendered as they are: a structural walk shows
/// what the file says, so it does not refuse a non-canonical integer the
/// way the loader does. The digit count is bounded all the same, and
/// `to_bounded_text` keeps the rendering of a huge one from taking the
/// quadratic decimal conversion.
fn read_large_integer_text(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    let (negative, magnitude) = decode::read_large_integer_parts(reader)?;
    Ok(ArbitraryInteger::from_parts(negative, magnitude).to_bounded_text())
}

/// One part of a fraction, rendered as the two integer tags are; like
/// them, not held to the normal form the loader checks.
fn read_fraction_part_text(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    match reader.read_u8()? {
        opcodes::VALUE_SMALL_INTEGER => Ok(reader.read_i64()?.to_string()),
        opcodes::VALUE_LARGE_INTEGER => read_large_integer_text(reader),
        tag => Err(decode::DecodeError::InvalidTag {
            what: "fraction part",
            tag,
        }),
    }
}

fn read_values(
    reader: &mut decode::Reader,
    targets: &TagTargets<'_>,
) -> Result<Vec<String>, decode::DecodeError> {
    let count = decode::bounded_count(reader.read_u32()? as usize, 1, reader)?;
    (0..count).map(|_| read_value(reader, targets)).collect()
}

fn read_closure_record(
    reader: &mut decode::Reader,
    targets: &TagTargets<'_>,
) -> Result<String, decode::DecodeError> {
    let code = reader.read_u32()?;
    let captured = reader.read_u32()?;
    let count = reader.read_u16()? as usize;
    let mut copied = decode::reserve_for(count, reader);
    for _ in 0..count {
        copied.push(read_value(reader, targets)?);
    }
    Ok(format!(
        "code {code}, captured frame {captured}, copied [{}]",
        copied.join(", ")
    ))
}

/// An `OBJECT_BYTE_ARRAY` body: its size and its first 16 bytes in hex,
/// enough to tell two apart without printing a large one whole.
fn read_byte_array_record(reader: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    let bytes = reader.read_byte_slice()?;
    let shown: Vec<String> = bytes
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let more = if bytes.len() > 16 { " ..." } else { "" };
    Ok(format!(
        "byte array of {} bytes [{}{more}]",
        bytes.len(),
        shown.join(" ")
    ))
}

fn walk_graph(
    reader: &mut decode::Reader,
    symbols: &symbols::Table,
    targets: &TagTargets<'_>,
) -> Result<GraphView, decode::DecodeError> {
    let object_count = decode::bounded_count(
        reader.read_u32()? as usize,
        decode::MIN_OBJECT_RECORD_BYTES,
        reader,
    )?;

    let mut objects = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        let class = read_class_tag(reader, targets)?;
        let hash = reader.read_u32()?;
        let body = match reader.read_u8()? {
            opcodes::OBJECT_INSTANCE => {
                format!("instance {{{}}}", read_values(reader, targets)?.join(", "))
            }
            opcodes::OBJECT_STRING => format!("string {:?}", reader.read_str()?),
            opcodes::OBJECT_BYTE_ARRAY => read_byte_array_record(reader)?,
            opcodes::OBJECT_SYMBOL => format!("symbol #{}", symbols.get(reader.read_u32()?)?),
            opcodes::OBJECT_ARRAY => {
                format!("array [{}]", read_values(reader, targets)?.join(", "))
            }
            opcodes::OBJECT_WEAK_ARRAY => {
                format!("weak array [{}]", read_values(reader, targets)?.join(", "))
            }
            opcodes::OBJECT_EPHEMERON => {
                format!("ephemeron {{{}}}", read_values(reader, targets)?.join(", "))
            }
            opcodes::OBJECT_CLOSURE => format!("closure {}", read_closure_record(reader, targets)?),
            opcodes::OBJECT_CLASS => read_class_record(reader, targets)?,
            opcodes::OBJECT_METACLASS => {
                format!("metaclass of {}", read_class_tag(reader, targets)?)
            }
            opcodes::OBJECT_FRAME => read_frame_record(reader, symbols, targets)?,
            tag => {
                return Err(decode::DecodeError::InvalidTag {
                    what: "object body",
                    tag,
                })
            }
        };
        objects.push(format!("class {class}, hash {hash}, {body}"));
    }

    let roots = read_values(reader, targets)?;

    let count = decode::bounded_count(
        reader.read_u32()? as usize,
        decode::MIN_TAG_NAMED_HASH_BYTES,
        reader,
    )?;
    let mut tag_named = Vec::with_capacity(count);
    for _ in 0..count {
        let who = match reader.read_u8()? {
            opcodes::HASHED_SHIPPED => read_shipped_name(reader, targets)?,
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

    // Each module's origin, source and the object id of its root class
    // record, shown as written: the graph it indexes comes later. A count
    // that disagrees with the mixin table's trees is refused here as it is
    // by `load`: which tree a module's root is held to depends on the
    // count, so a report past it would describe modules that are not there.
    let decoded_modules =
        modules::decode_modules(&mut reader, &decoded.module_rows, header.has_source)
            .map_err(|error| error.to_string())?;
    let module_reports = decoded_modules
        .iter()
        .map(|module| {
            let root = format!("root object {}", module.root);
            ModuleReport {
                origin: match &module.origin {
                    SourceOrigin::Embedded(name) => format!("embedded {name}"),
                    SourceOrigin::File(_) => format!("{:?}", module.origin),
                },
                has_source: module.source.is_some(),
                root,
            }
        })
        .collect::<Vec<_>>();
    sections.push(Section {
        name: "modules",
        start: offset,
        end: reader.position(),
        detail: format!("{} modules", module_reports.len()),
    });
    offset = reader.position();

    let names = shipped_names::decode_section(&mut reader, &symbol_table)
        .map_err(|error| error.to_string())?;
    sections.push(Section {
        name: "shipped names",
        start: offset,
        end: reader.position(),
        detail: format!("{} names", names.len()),
    });
    offset = reader.position();
    let targets = TagTargets { names: &names };
    let graph =
        walk_graph(&mut reader, &symbol_table, &targets).map_err(|error| error.to_string())?;
    sections.push(Section {
        name: "graph",
        start: offset,
        // The walk consumes the section record by record, so this is the
        // real end rather than a stand-in. That it lands on the file.s
        // last byte is what `graph_walk_consumes_the_whole_section` pins.
        end: reader.position(),
        detail: format!(
            "{} objects, {} roots, {} tag-named hashes",
            graph.objects.len(),
            graph.roots.len(),
            graph.tag_named.len()
        ),
    });

    Ok(Report {
        format_version: crate::image::encode::FORMAT_VERSION,
        shipped_world: header.fingerprint,
        start: header.start,
        modules: module_reports,
        sections,
        symbols: symbol_list,
        code: code_lines,
        mixins: mixin_lines,
        mixin_detail,
        bodies,
        shipped_names: names.iter().map(ShippedName::describe).collect(),
        graph_objects: graph.objects,
        graph_roots: graph.roots,
        graph_tag_named_hashes: graph.tag_named,
        total_length: bytes.len(),
    })
}

pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "format version {}", report.format_version);
    let _ = writeln!(out, "shipped world {:016x}", report.shipped_world);
    let _ = writeln!(out, "start: {}", if report.start { "yes" } else { "no" });
    let _ = writeln!(out, "modules: {}", report.modules.len());
    for (index, module) in report.modules.iter().enumerate() {
        let _ = writeln!(
            out,
            "  {index}: {}, source {}, {}",
            module.origin,
            if module.has_source {
                "present"
            } else {
                "absent"
            },
            module.root
        );
    }
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

    // An image with no graph records prints no empty heading that would
    // invite the reader to look for state that is not there.
    if !report.graph_objects.is_empty() || !report.graph_tag_named_hashes.is_empty() {
        let _ = writeln!(out, "\nshipped names ({})", report.shipped_names.len());
        for (index, name) in report.shipped_names.iter().enumerate() {
            let _ = writeln!(out, "  {index:>4} {name}");
        }
        let _ = writeln!(out, "\ngraph");
        let _ = writeln!(out, "  objects ({})", report.graph_objects.len());
        for (index, object) in report.graph_objects.iter().enumerate() {
            let _ = writeln!(out, "    {index:>4} {object}");
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
