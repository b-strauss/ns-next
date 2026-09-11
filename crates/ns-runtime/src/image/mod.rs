//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/design/image-format.md`. Layout: header, optional source text,
//! `SourceOrigin`, symbol table, code table, mixin table, root class, entry
//! field, object table, frame table, roots.
//!
//! **Loading never runs the front end.** `load` decodes the world; it does
//! not parse, resolve, or compile anything. The symbol table, the mixin
//! table and the class-object records together describe every declaration
//! the world holds, so there is nothing left for a re-parse to rebuild --
//! and the image's source text, which is attacker-controlled input like
//! every other byte in the file, is never handed to the parser. It is not
//! even required to be present: `snapshot(None, ...)` writes an image with
//! no source in it at all, and `snapshot(Some(src), ...)` writes the debug
//! variant, whose source blob exists so a loaded world can be shown (and,
//! once a decompiler exists, compared) as `.ns`.

pub mod decode;
pub mod encode;
pub mod inspect;
pub mod mixins;
pub mod opcodes;
pub mod symbols;

use crate::bytecode::CompiledModule;
use crate::heap::Heap;
use crate::types::{ClassRef, SourceOrigin, Value};
use crate::{Interpreter, Symbol};

/// The sections that describe code and structure, encoded into their own
/// buffers with the symbol table still open so the caller can intern into it.
///
/// Both artifacts go through this, which is what makes the section *order*
/// and the *encoding logic* for code, mixins and the root class identical
/// between them by construction rather than by discipline -- a second copy
/// would drift at the first format change, and the test that compares them
/// would only find out afterwards.
///
/// **The symbol table is not covered by that guarantee.** `symbols` stays
/// open past `encode_describing` on purpose, so `build` can intern the entry
/// selector and `snapshot` can intern whatever its object graph names --
/// `'foo' asSymbol` mints a symbol at run time whose text need not appear in
/// either table otherwise. Two calls that are supposed to describe "the same
/// module" therefore end up with identical describing bytes only when both
/// (a) the entry selector `build` is given is already interned by the code or
/// mixin tables, and (b) the roots `snapshot` is given intern no string the
/// describing sections did not already contain. Neither is enforced here:
/// (a) is a build-time validation that does not exist yet; (b) is not
/// enforced at all, is expected to fail for an arbitrary program, and is
/// not something this format tries to guarantee.
struct Describing {
    symbols: symbols::Builder,
    code: encode::Writer,
    mixins: encode::Writer,
    ids: encode::CodeIds,
    mixin_ids: mixins::MixinIds,
}

/// Encodes the code table and the mixin table into their own buffers,
/// collecting every selector and name either one names into `symbols` along
/// the way. Shared by `snapshot` and `build`: both need the same two tables,
/// and both need them encoded before the (still-open) symbol table can be
/// written, since it is the table's *contents* that fill it.
fn encode_describing(
    root_class: &ClassRef,
    module: &CompiledModule,
    heap: &Heap,
) -> Result<Describing, String> {
    let mut symbols = symbols::Builder::new();
    let mut code = encode::Writer::new();
    let ids = encode::encode_code_table(&mut code, heap, module, root_class, &mut symbols)?;
    // After the code table, whose ids its rows name; before the graph (or the
    // entry field), whose class objects and selector respectively might name
    // its rows.
    let mut mixin_w = encode::Writer::new();
    let mixin_ids = mixins::encode(&mut mixin_w, heap, root_class, module, &mut symbols, &ids)?;
    Ok(Describing {
        symbols,
        code,
        mixins: mixin_w,
        ids,
        mixin_ids,
    })
}

/// Writes header through root class. Everything after that is the caller's:
/// the entry field, and for an image the graph.
fn write_describing(
    w: &mut encode::Writer,
    source: Option<&str>,
    origin: &SourceOrigin,
    flags: u8,
    d: Describing,
    root_class: &ClassRef,
    heap: &Heap,
) -> Result<(), String> {
    encode::write_header(
        w,
        flags
            | match source {
                Some(_) => encode::FLAG_HAS_SOURCE,
                None => 0,
            },
    );
    if let Some(source) = source {
        w.write_str(source);
    }
    encode::encode_source_origin(w, origin);
    // The symbol table is written here, ahead of the code and mixin tables it
    // fills, so a reader can decode it once before either asks it to resolve
    // an id. Both callers are done with `d.mixin_ids` by this point --
    // `snapshot` has already used it to encode the graph into its own buffer,
    // `build` never needed it -- so consuming `d` here loses nothing.
    d.symbols.write(w);
    w.write_raw(&d.code.into_bytes());
    w.write_raw(&d.mixins.into_bytes());
    encode::encode_root_class(w, heap, root_class)?;
    Ok(())
}

/// Serializes a quiesced world: `roots` (the root instance and, if any,
/// the entry send's result) plus everything reachable from them, together
/// with the module's provenance and compiled code.
///
/// `source` is the debug variant's switch. `None` writes an image whose
/// bytes contain no program text -- what a loaded world needs is the code
/// table, the mixin table and the graph, and none of them quotes the
/// source. `Some(src)` additionally stores `src` verbatim; `load` hands it
/// back on `LoadedImage::source` and does nothing else with it.
pub fn snapshot(
    heap: &Heap,
    source: Option<&str>,
    origin: &SourceOrigin,
    root_class: &ClassRef,
    module: &CompiledModule,
    platform: &Value,
    roots: &[Value],
) -> Result<Vec<u8>, String> {
    // A frame married to a running activation keeps its registers and `pc`
    // on the activation stack, and the format has no record for the stack,
    // so encoding one would write a frame with empty registers and succeed.
    // `Heap::frame_snapshot` debug-asserts against that per frame; this is
    // the check a release build keeps.
    if heap.activation_depth() != 0 {
        return Err(
            "image encoding cannot snapshot a world mid-execution: a snapshot is \
             taken only between sends, when no activation is running"
                .to_string(),
        );
    }
    let mut d = encode_describing(root_class, module, heap)?;
    // The graph section is encoded into its own buffer, against the same
    // `symbols` builder the describing sections used, before `write_describing`
    // consumes `d` -- the graph needs the mixin table's row ids, and those
    // exist only on `d.mixin_ids`.
    let mut graph_w = encode::Writer::new();
    encode::encode_graph_section(
        &mut graph_w,
        heap,
        roots,
        platform,
        &d.ids,
        root_class,
        &d.mixin_ids,
        &mut d.symbols,
    )?;
    let mut w = encode::Writer::new();
    write_describing(&mut w, source, origin, 0, d, root_class, heap)?;
    // `snapshot` never names an entry selector of its own -- a snapshotted
    // world is resumed by loading it and sending to `roots`, not by starting
    // it fresh -- but the field is written unconditionally so every artifact
    // this format can produce shares one layout from here on.
    encode::encode_entry(&mut w, None);
    w.write_raw(&graph_w.into_bytes());
    Ok(w.into_bytes())
}

/// Serializes a **stateless binary**: code and structure, no instance state,
/// no activations, and the selector to send on start-up.
///
/// The caller has compiled but deliberately not instantiated. That omission is
/// the whole point of this artifact -- the primary factory and every
/// initializer run on load instead, against the loading world's own freshly
/// bootstrapped platform, so the file has never held a capability.
pub fn build(
    heap: &Heap,
    source: Option<&str>,
    origin: &SourceOrigin,
    root_class: &ClassRef,
    module: &CompiledModule,
    entry: Symbol,
) -> Result<Vec<u8>, String> {
    let mut d = encode_describing(root_class, module, heap)?;
    // Interned before `write_describing` writes the table below: an id
    // assigned after the table is on the wire is an id the table does not
    // contain, which the decoder reports as an out-of-range symbol id.
    let entry_id = d.symbols.intern(entry.as_str());
    let mut w = encode::Writer::new();
    write_describing(
        &mut w,
        source,
        origin,
        encode::FLAG_STATELESS,
        d,
        root_class,
        heap,
    )?;
    encode::encode_entry(&mut w, Some(entry_id));
    Ok(w.into_bytes())
}

pub struct LoadedImage {
    /// Bootstrapped, with the image's own code table installed.
    pub interp: Interpreter,
    pub root_class: ClassRef,
    pub roots: Vec<Value>,
    /// Where the module came from. Provenance, not program text -- it is
    /// in every image, source blob or not.
    pub origin: SourceOrigin,
    /// The image's source text, for the debug variant only -- `None` for a
    /// source-free image. Never parsed, never resolved, never executed:
    /// `load` reads it out of the byte stream and hands it over unchanged.
    pub source: Option<String>,
    /// The selector to send after instantiation. Mandatory in a binary,
    /// always `None` in an image today.
    pub entry: Option<Symbol>,
    /// `header.stateless` (`FLAG_STATELESS`), carried through so the caller
    /// can branch on the artifact's own declared shape rather than infer it
    /// from `roots.is_empty()` -- a stateful image snapshotted with an empty
    /// roots slice is a real, tested case that flag-less inference would
    /// misread as a binary.
    pub stateless: bool,
}

/// Decodes an artifact: symbols, code table, mixin table, root class, entry,
/// then -- for an image, never a binary -- the object graph. Installs
/// the code table and, for an image, advances the identity-hash counter
/// past the graph's maximum.
///
/// Nothing here parses, resolves or compiles. The root class is rebuilt
/// from mixin-table row 0 (`decode::build_root_class`), which is what makes
/// the front end unnecessary -- and, since an artifact is untrusted input,
/// what keeps the parser off the load path entirely.
///
/// A binary (`header.stateless`) carries no graph section, so `roots` comes
/// back empty and `verify_graph` has nothing to check; it must carry an
/// entry, since that selector is the only way it says how to start once a
/// caller instantiates `root_class`, so a stateless artifact with no entry
/// is refused here rather than accepted and left to fail wherever a caller
/// later looks for something to send.
///
/// Both verification passes run before anything the image describes can be
/// executed: `verify_code_table` between decoding and installing the code
/// table, `verify_graph` (image only) once the module exists to resolve
/// each frame's body against. A corrupt but version-matching artifact is an
/// `Err` here,
/// never a panic in the VM later.
pub fn load(bytes: &[u8]) -> Result<LoadedImage, String> {
    let mut r = decode::Reader::new(bytes);
    let header = decode::read_header(&mut r).map_err(|e| e.to_string())?;
    let source = if header.has_source {
        Some(r.read_str().map_err(|e| e.to_string())?)
    } else {
        None
    };
    let origin = decode::decode_source_origin(&mut r).map_err(|e| e.to_string())?;
    let mut interp = Interpreter::bootstrap();
    let symbols = symbols::decode(&mut r).map_err(|e| e.to_string())?;
    let table = decode::decode_code_table(&mut r, &symbols).map_err(|e| e.to_string())?;
    decode::verify_code_table(&table).map_err(|e| e.to_string())?;
    let mixins = mixins::decode(&mut r, &symbols, &table.codes).map_err(|e| e.to_string())?;
    let module = decode::build_module(&table, &mixins).map_err(|e| e.to_string())?;
    let root_superclass =
        decode::decode_root_superclass(&mut r, &interp).map_err(|e| e.to_string())?;
    let root_class = decode::build_root_class(&mut interp, &mixins, root_superclass)
        .map_err(|e| e.to_string())?;
    // `snapshot` always writes this field (as absent); a binary requires it
    // present, since it is the only place a binary records how to start
    // itself once instantiated.
    let entry = decode::decode_entry(&mut r, &symbols).map_err(|e| e.to_string())?;
    if header.stateless && entry.is_none() {
        // A binary that cannot say how it starts is not a deployment
        // artifact -- refusing here keeps the failure where the artifact is
        // read, rather than wherever a caller eventually fails to find
        // something to send.
        return Err("a stateless artifact must record an entry selector".to_string());
    }
    let platform = interp.platform();
    // A binary carries no graph section at all -- `decode_graph_section`
    // would read the next artifact's bytes (there are none) as if they were
    // one. `DecodedGraph::empty()` is the graph a binary has: no roots, no
    // objects, no frames, nothing for `verify_graph` to check.
    let graph = if header.stateless {
        decode::DecodedGraph::empty()
    } else {
        let mut ctx = decode::DecodeContext {
            interp: &mut interp,
            root_class: &root_class,
            codes: &table.codes,
            platform,
            mixins: &mixins.mixins,
        };
        decode::decode_graph_section(&mut r, &mut ctx, &symbols).map_err(|e| e.to_string())?
    };
    if !header.stateless {
        decode::verify_graph(&graph, &table, &module, &interp.heap).map_err(|e| e.to_string())?;
        interp.advance_identity_hash_past(graph.max_identity_hash);
    }
    // A byte sequence past the last section is refused, not ignored: with
    // two artifact shapes sharing one format, an unchecked tail would let a
    // binary with a graph appended -- or a truncated, then padded, image --
    // load as something its author did not write.
    if r.remaining() != 0 {
        return Err(decode::DecodeError::TrailingBytes {
            count: r.remaining(),
        }
        .to_string());
    }
    interp.install_code(module);
    Ok(LoadedImage {
        interp,
        root_class,
        roots: graph.roots,
        origin,
        source,
        entry,
        stateless: header.stateless,
    })
}
