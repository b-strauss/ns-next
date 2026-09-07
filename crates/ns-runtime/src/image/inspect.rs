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
    /// One line per mixin row: name, slot count, factory, method count.
    pub mixins: Vec<String>,
    pub entry: Option<String>,
    pub total_len: usize,
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

    // A later task adds the entry field to the format; nothing to read yet.
    let entry: Option<String> = None;

    // A later task adds the flag; no artifact can be stateless yet -- every
    // artifact carries a graph section.
    let stateless = false;
    let counts = decode::count_graph_section(&mut r, &symbol_table).map_err(|e| e.to_string())?;
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

    Ok(Report {
        format_version: crate::image::encode::FORMAT_VERSION,
        has_source: header.has_source,
        stateless,
        origin: format!("{origin:?}"),
        sections,
        symbols: symbol_list,
        mixins: mixin_lines,
        entry,
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

    let _ = writeln!(out, "\nmixins ({})", report.mixins.len());
    for (i, m) in report.mixins.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {m}");
    }

    out
}
