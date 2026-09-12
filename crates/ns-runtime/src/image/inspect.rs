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
    pub entry: Option<String>,
    pub total_len: usize,
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
        Some(f) => {
            let _ = writeln!(out, "      factory   {}/{}", f.selector, f.num_args);
        }
        None => {
            let _ = writeln!(out, "      factory   -");
        }
    }
    if mixin.slots.is_empty() {
        let _ = writeln!(out, "      slots     -");
    } else {
        let names: Vec<String> = mixin.slots.iter().map(|s| s.name.to_string()).collect();
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
/// class tags against live builtins and therefore needs an `Interpreter`. A
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
}

fn read_class_tag(r: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    Ok(match r.read_u8()? {
        opcodes::CLS_BUILTIN => format!("{:?}", decode::read_builtin_class(r)?),
        opcodes::CLS_USER_ROOT => "<root class>".to_string(),
        opcodes::CLS_RECORD => format!("object {}", r.read_u32()?),
        tag => {
            return Err(decode::DecodeError::InvalidTag {
                what: "class tag",
                tag,
            })
        }
    })
}

fn read_optional_class(r: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    if r.read_bool()? {
        read_class_tag(r)
    } else {
        Ok("-".to_string())
    }
}

fn read_frame_ref(r: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    Ok(if r.read_bool()? {
        format!("frame {}", r.read_u32()?)
    } else {
        "-".to_string()
    })
}

fn read_value(r: &mut decode::Reader) -> Result<String, decode::DecodeError> {
    Ok(match r.read_u8()? {
        opcodes::VAL_NIL => "nil".to_string(),
        opcodes::VAL_FALSE => "false".to_string(),
        opcodes::VAL_TRUE => "true".to_string(),
        opcodes::VAL_SMALL_INT => r.read_i64()?.to_string(),
        opcodes::VAL_FLOAT => r.read_f64()?.to_string(),
        opcodes::VAL_OBJECT_ID => format!("object {}", r.read_u32()?),
        opcodes::VAL_CLASS_TAG => format!("class {}", read_class_tag(r)?),
        opcodes::VAL_PLATFORM => "<platform>".to_string(),
        tag => return Err(decode::DecodeError::InvalidTag { what: "value", tag }),
    })
}

fn read_values(r: &mut decode::Reader) -> Result<Vec<String>, decode::DecodeError> {
    let n = decode::bounded_count(r.read_u32()? as usize, 1, r)?;
    (0..n).map(|_| read_value(r)).collect()
}

fn read_closure_record(
    r: &mut decode::Reader,
    symbols: &symbols::Table,
) -> Result<String, decode::DecodeError> {
    let params = r.read_u16()? as usize;
    let mut names = Vec::with_capacity(params);
    for _ in 0..params {
        names.push(symbols.get(r.read_u32()?)?.to_string());
    }
    let code = r.read_u32()?;
    let captured = r.read_u32()?;
    let home = r.read_u32()?;
    Ok(format!(
        "[{}] code {code}, captured frame {captured}, home frame {home}",
        names.join(", ")
    ))
}

fn walk_graph(
    r: &mut decode::Reader,
    symbols: &symbols::Table,
) -> Result<GraphView, decode::DecodeError> {
    let object_count = decode::bounded_count(r.read_u32()? as usize, 1, r)?;
    let frame_count = decode::bounded_count(r.read_u32()? as usize, 1, r)?;

    let mut objects = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        let class = read_class_tag(r)?;
        let hash = r.read_u32()?;
        let body = match r.read_u8()? {
            opcodes::OBJ_INSTANCE => format!("instance {{{}}}", read_values(r)?.join(", ")),
            opcodes::OBJ_STR => format!("string {:?}", r.read_str()?),
            opcodes::OBJ_SYM => format!("symbol #{}", symbols.get(r.read_u32()?)?),
            opcodes::OBJ_ARRAY => format!("array [{}]", read_values(r)?.join(", ")),
            opcodes::OBJ_CLOSURE => format!("closure {}", read_closure_record(r, symbols)?),
            opcodes::OBJ_CLASS => {
                let mixin = r.read_u32()?;
                let superclass = read_optional_class(r)?;
                let enclosing = if r.read_bool()? {
                    read_value(r)?
                } else {
                    "-".to_string()
                };
                let declaring = read_optional_class(r)?;
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
        let receiver = read_value(r)?;
        let args = read_values(r)?;
        let temps = read_values(r)?;
        let pc = r.read_u32()?;
        let sender = read_frame_ref(r)?;
        let home = read_frame_ref(r)?;
        let current_class = read_optional_class(r)?;
        let kind = match r.read_u8()? {
            opcodes::FRAME_METHOD => format!("method {}", symbols.get(r.read_u32()?)?),
            opcodes::FRAME_CLOSURE => format!("closure {}", read_closure_record(r, symbols)?),
            opcodes::FRAME_TOP_LEVEL => "top level".to_string(),
            opcodes::FRAME_INITIALIZER => format!("initializer of {}", read_class_tag(r)?),
            opcodes::FRAME_SUPERCLASS_CLAUSE => {
                format!("superclass clause of {}", read_class_tag(r)?)
            }
            opcodes::FRAME_SUPERCLASS_MESSAGE => {
                format!("superclass message of {}", read_class_tag(r)?)
            }
            tag => {
                return Err(decode::DecodeError::InvalidTag {
                    what: "frame kind",
                    tag,
                })
            }
        };
        let uncontinuable = r.read_bool()?;
        frames.push(format!(
            "{kind}, receiver {receiver}, args [{}], temps [{}], pc {pc}, \
             sender {sender}, home {home}, current class {current_class}, \
             uncontinuable {uncontinuable}",
            args.join(", "),
            temps.join(", ")
        ));
    }

    let roots = read_values(r)?;
    Ok(GraphView {
        objects,
        frames,
        roots,
    })
}

/// Decodes `bytes` far enough to describe it. Every failure is an `Err`, never
/// a panic -- an artifact is untrusted input here exactly as it is in `load`.
pub fn inspect(bytes: &[u8]) -> Result<Report, String> {
    let mut r = decode::Reader::new(bytes);
    let mut sections = Vec::new();
    let mut at = 0usize;

    let header = decode::read_header(&mut r).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "header",
        start: at,
        end: r.pos(),
        detail: String::new(),
    });
    at = r.pos();

    if header.has_source {
        let source = r.read_str().map_err(|e| e.to_string())?;
        sections.push(Section {
            name: "source",
            start: at,
            end: r.pos(),
            detail: format!("{} bytes", source.len()),
        });
        at = r.pos();
    }

    let origin = decode::decode_source_origin(&mut r).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "origin",
        start: at,
        end: r.pos(),
        detail: String::new(),
    });
    at = r.pos();

    let symbol_table = symbols::decode(&mut r).map_err(|e| e.to_string())?;
    let symbol_list = symbol_table
        .all()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    sections.push(Section {
        name: "symbols",
        start: at,
        end: r.pos(),
        detail: format!("{} symbols", symbol_list.len()),
    });
    at = r.pos();

    let table = decode::decode_code_table(&mut r, &symbol_table).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "code",
        start: at,
        end: r.pos(),
        detail: format!("{} bodies", table.codes.len()),
    });
    at = r.pos();

    let decoded = mixins::decode(&mut r, &symbol_table, &table.codes).map_err(|e| e.to_string())?;
    let mixin_lines = decoded
        .mixins
        .iter()
        .zip(decoded.code.iter())
        .map(|(m, c)| {
            format!(
                "{}  slots: {}  factory: {}  methods: {}",
                m.name,
                m.slots.len(),
                match &m.factory {
                    Some(f) => f.selector.to_string(),
                    None => "-".to_string(),
                },
                c.methods.len(),
            )
        })
        .collect::<Vec<_>>();
    sections.push(Section {
        name: "mixins",
        start: at,
        end: r.pos(),
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
        .map(|(m, c)| expand_mixin(m, c))
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

    at = r.pos();

    // The root class's superclass reference. Decoding it needs an interpreter
    // only to resolve a builtin tag to a live class, which inspection does not
    // want; the bytes are read and their span recorded without resolving.
    decode::skip_root_superclass(&mut r).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "root class",
        start: at,
        end: r.pos(),
        detail: String::new(),
    });
    at = r.pos();

    let entry = decode::decode_entry(&mut r, &symbol_table).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "entry",
        start: at,
        end: r.pos(),
        detail: match &entry {
            Some(sym) => sym.to_string(),
            None => "none".to_string(),
        },
    });
    at = r.pos();

    // A stateless artifact ends here: it carries code and structure but no
    // object graph, so reading one past the entry field would run off the
    // end of the file.
    let mut graph = GraphView {
        objects: Vec::new(),
        frames: Vec::new(),
        roots: Vec::new(),
    };
    if !header.stateless {
        graph = walk_graph(&mut r, &symbol_table).map_err(|e| e.to_string())?;
        sections.push(Section {
            name: "graph",
            start: at,
            // The walk consumes the section record by record, so this is the
            // real end rather than a stand-in. That it lands on the file.s
            // last byte is what `graph_walk_consumes_the_whole_section` pins.
            end: r.pos(),
            detail: format!(
                "{} objects, {} frames, {} roots",
                graph.objects.len(),
                graph.frames.len(),
                graph.roots.len()
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
        entry: entry.map(|s| s.to_string()),
        total_len: bytes.len(),
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
    let _ = writeln!(out, "total: {} bytes", report.total_len);

    let _ = writeln!(out, "\nsections");
    for s in &report.sections {
        let _ = writeln!(
            out,
            "  {:<12} {:>8}..{:<8} {}",
            s.name, s.start, s.end, s.detail
        );
    }

    let _ = writeln!(out, "\nsymbols ({})", report.symbols.len());
    for (i, s) in report.symbols.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {s}");
    }

    let _ = writeln!(out, "\ncode table ({} bodies)", report.code.len());
    for (i, c) in report.code.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {c}");
    }

    let _ = writeln!(out, "\nmixins ({})", report.mixins.len());
    for (i, m) in report.mixins.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {m}");
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
    for (i, s) in report.symbols.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {s}");
    }

    let _ = writeln!(out, "\ncode table ({} bodies)", report.code.len());
    for (i, c) in report.code.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {c}");
    }

    let _ = writeln!(out, "\nmixins ({})", report.mixin_detail.len());
    for (i, m) in report.mixin_detail.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {m}");
    }

    // Only an image has one. A binary's whole point is that it does not, so
    // printing an empty heading there would invite the reader to look for
    // state that is deliberately absent.
    if !report.graph_objects.is_empty() || !report.graph_frames.is_empty() {
        let _ = writeln!(out, "\ngraph");
        let _ = writeln!(out, "  objects ({})", report.graph_objects.len());
        for (i, o) in report.graph_objects.iter().enumerate() {
            let _ = writeln!(out, "    {i:>4} {o}");
        }
        let _ = writeln!(out, "  frames ({})", report.graph_frames.len());
        for (i, f) in report.graph_frames.iter().enumerate() {
            let _ = writeln!(out, "    {i:>4} {f}");
        }
        let _ = writeln!(out, "  roots  {}", report.graph_roots.join(", "));
    }

    let _ = writeln!(out, "\nbodies ({})", report.bodies.len());
    for body in &report.bodies {
        let _ = writeln!(out);
        out.push_str(body);
    }

    out
}
