//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/design/image-format.md`. Layout: header, optional source text,
//! `SourceOrigin`, symbol table, code table, mixin table, root class,
//! object table, frame table, roots.
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

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::CompiledModule;
use crate::interpreter::Engine;
use crate::types::{Binding, ClassRef, SourceOrigin, Value};
use crate::Interpreter;

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
    source: Option<&str>,
    origin: &SourceOrigin,
    root_class: &ClassRef,
    module: &CompiledModule,
    platform: &Value,
    roots: &[Value],
) -> Result<Vec<u8>, String> {
    let mut w = encode::Writer::new();
    encode::write_header(
        &mut w,
        match source {
            Some(_) => encode::FLAG_HAS_SOURCE,
            None => 0,
        },
    );
    if let Some(source) = source {
        w.write_str(source);
    }
    encode::encode_source_origin(&mut w, origin);

    // The code table, the mixin table and the graph section all reference
    // selectors and names by a symbol-table id, so each is encoded into its
    // own buffer first (collecting those strings into `symbols` along the
    // way) and only then does the finished table get written -- ahead of all
    // three, so a reader can decode it once before any of them asks it to
    // resolve an id.
    let mut symbols = symbols::Builder::new();
    let mut code_w = encode::Writer::new();
    let ids = encode::encode_code_table(&mut code_w, module, root_class, &mut symbols)?;
    // After the code table, whose ids its rows name; before the graph, whose
    // class objects name its rows.
    let mut mixin_w = encode::Writer::new();
    let mixin_ids = mixins::encode(&mut mixin_w, root_class, module, &mut symbols, &ids)?;
    let mut graph_w = encode::Writer::new();
    encode::encode_graph_section(
        &mut graph_w,
        roots,
        platform,
        &ids,
        root_class,
        &mixin_ids,
        &mut symbols,
    )?;

    symbols.write(&mut w);
    w.write_raw(&code_w.into_bytes());
    w.write_raw(&mixin_w.into_bytes());
    // Between the mixin table it completes and the graph that needs the
    // root class to already exist.
    encode::encode_root_class(&mut w, root_class)?;
    w.write_raw(&graph_w.into_bytes());
    Ok(w.into_bytes())
}

pub struct LoadedImage {
    /// Bootstrapped, with `Engine::Bytecode` (the image's own code table)
    /// installed and the tree-walker sealed off
    /// (`interpreter::EngineUnavailable::TreeWalkAfterLoad`).
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
    /// An empty map, allocated once per load, for the `&Rc<HashMap<Span,
    /// Binding>>` every `Interpreter::send`, `instantiate` and closure
    /// activation still takes.
    ///
    /// **It is empty because nothing a loaded world runs can read it, and
    /// that is a property of the bytecode engine, not an omission to be
    /// repaired.** A binding is resolved *by span* at exactly two kinds of
    /// call site, and neither is reachable here:
    ///
    /// - `compiler/lower.rs` -- compile time. Lowering is what turns a
    ///   `Binding` into a register index, an `Instr::LoadOuterLocal`, or a
    ///   `LexicalSend`; it ran before the snapshot, and its results are the
    ///   instructions in the image's code table. Nothing recompiles on
    ///   load.
    /// - `interpreter/eval.rs` -- the tree-walker, which cannot run from an
    ///   image at all: `set_engine(Engine::TreeWalk)` is refused on a
    ///   loaded interpreter.
    ///
    /// So do not "fix" this by reconstructing a real map. Reconstructing
    /// one requires re-resolving the source, which requires re-parsing it,
    /// which is exactly the load path this milestone removed -- and the
    /// map's only readers would still never look at it.
    pub empty_bindings: Rc<HashMap<Span, Binding>>,
}

/// Decodes a world: symbols, code table, mixin table, root class, then the
/// object graph, installing `Engine::Bytecode` and advancing the
/// identity-hash counter past the image's maximum.
///
/// Nothing here parses, resolves or compiles. The root class is rebuilt
/// from mixin-table row 0 (`decode::build_root_class`), which is what makes
/// the front end unnecessary -- and, since an image is untrusted input,
/// what keeps the parser off the load path entirely.
///
/// Both verification passes run before anything the image describes can be
/// executed: `verify_code_table` between decoding and installing the code
/// table, `verify_graph` once the module exists to resolve each frame's
/// body against. A corrupt but version-matching image is an `Err` here,
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
    let empty_bindings = Rc::new(HashMap::new());
    let mut interp = Interpreter::bootstrap();
    let symbols = symbols::decode(&mut r).map_err(|e| e.to_string())?;
    let table = decode::decode_code_table(&mut r, &symbols).map_err(|e| e.to_string())?;
    decode::verify_code_table(&table).map_err(|e| e.to_string())?;
    let mixins = mixins::decode(&mut r, &symbols, &table.codes).map_err(|e| e.to_string())?;
    let module =
        decode::build_module(&table, &mixins, &empty_bindings).map_err(|e| e.to_string())?;
    let root_superclass =
        decode::decode_root_superclass(&mut r, &interp).map_err(|e| e.to_string())?;
    let root_class = decode::build_root_class(&mut interp, &mixins, root_superclass)
        .map_err(|e| e.to_string())?;
    let platform = interp.platform();
    let graph = {
        let mut ctx = decode::DecodeContext {
            interp: &mut interp,
            root_class: &root_class,
            codes: &table.codes,
            bindings: &empty_bindings,
            platform,
            mixins: &mixins.mixins,
        };
        decode::decode_graph_section(&mut r, &mut ctx, &symbols).map_err(|e| e.to_string())?
    };
    decode::verify_graph(&graph, &table, &module).map_err(|e| e.to_string())?;
    interp.advance_identity_hash_past(graph.max_identity_hash);
    interp
        .set_engine(Engine::Bytecode(module))
        .map_err(|e| e.to_string())?;
    // Last, so that no failure above leaves a sealed interpreter behind:
    // from here on the tree-walker is refused on this world.
    interp.mark_decoded_from_image();
    Ok(LoadedImage {
        interp,
        root_class,
        roots: graph.roots,
        origin,
        source,
        empty_bindings,
    })
}
