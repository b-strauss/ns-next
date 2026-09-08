//! A read-only structural view of an artifact: where each section starts and
//! ends, what the symbol and mixin tables hold, and how much graph there is.
//!
//! This exists because an artifact was otherwise opaque. `disasm` shows one
//! compiled body; nothing showed the file. Two things need that. A failed
//! prefix-equality check between a `build` and a `snapshot` of the same source
//! reports a differing byte offset, which says nothing on its own and
//! everything once the offset is attributed to a section. And the claim that
//! the platform is cut out of an artifact -- the reason an image is kilobytes
//! rather than megabytes -- was checkable only by reading the encoder.
//!
//! Nothing here installs an engine, allocates a class object, or runs
//! anything -- inspecting a hostile file exercises only the decoders below,
//! never `Interpreter::bootstrap`.

use std::fmt::Write as _;

use crate::bytecode::{CodeKind, CompiledCode};
use std::rc::Rc;

use crate::image::{decode, mixins, symbols};

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
    if !header.stateless {
        let counts = decode::count_graph_section(&mut r).map_err(|e| e.to_string())?;
        sections.push(Section {
            name: "graph",
            start: at,
            // `count_graph_section` reads only the section's two leading counts,
            // not the records after them (see its own doc comment for why), so
            // `r.pos()` here would land just past those two counts, not at the
            // section's real end. The graph section is always last, so the
            // file's own length stands in for the end this can't otherwise see.
            end: bytes.len(),
            detail: format!("{} objects, {} frames", counts.objects, counts.frames),
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

    let _ = writeln!(out, "\nbodies ({})", report.bodies.len());
    for body in &report.bodies {
        let _ = writeln!(out);
        out.push_str(body);
    }

    out
}
