//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! "Image format" section. Layout: header, source text, `SourceOrigin`,
//! symbol table, code table, mixin table, object table, frame table, roots.

pub mod decode;
pub mod encode;
pub mod mixins;
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
/// with the module's source, provenance, and compiled code.
pub fn snapshot(
    source: &str,
    origin: &SourceOrigin,
    root_class: &ClassRef,
    module: &CompiledModule,
    platform: &Value,
    roots: &[Value],
) -> Result<Vec<u8>, String> {
    let mut w = encode::Writer::new();
    encode::write_header(&mut w);
    w.write_str(source);
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
    // class objects will name its rows (Task 13).
    let mut mixin_w = encode::Writer::new();
    let _mixin_ids = mixins::encode(&mut mixin_w, root_class, module, &mut symbols, &ids)?;
    let mut graph_w = encode::Writer::new();
    encode::encode_graph_section(
        &mut graph_w,
        roots,
        platform,
        &ids,
        root_class,
        &mut symbols,
    )?;

    symbols.write(&mut w);
    w.write_raw(&code_w.into_bytes());
    w.write_raw(&mixin_w.into_bytes());
    w.write_raw(&graph_w.into_bytes());
    Ok(w.into_bytes())
}

pub struct LoadedImage {
    /// Bootstrapped, with the module loaded and `Engine::Bytecode` (the
    /// image's own code table) installed.
    pub interp: Interpreter,
    pub root_class: ClassRef,
    pub bindings: Rc<HashMap<Span, Binding>>,
    pub roots: Vec<Value>,
}

/// Bootstrap from source, then decode: re-runs parse -> resolve ->
/// bootstrap -> `load_module` on the stored source (the class tags and
/// method frame kinds resolve against *that*), installs the decoded code
/// table as the engine, decodes the object graph, and advances the
/// identity-hash counter past the image's maximum.
///
/// Both verification passes run before anything the image describes can be
/// executed: `verify_code_table` between decoding and installing the code
/// table, `verify_graph` once the module exists to resolve each frame's
/// body against. A corrupt but version-matching image is an `Err` here,
/// never a panic in the VM later.
pub fn load(bytes: &[u8]) -> Result<LoadedImage, String> {
    let mut r = decode::Reader::new(bytes);
    decode::read_header(&mut r).map_err(|e| e.to_string())?;
    let source = r.read_str().map_err(|e| e.to_string())?;
    let origin = decode::decode_source_origin(&mut r).map_err(|e| e.to_string())?;
    let module_ast = ns_syntax::parse(&source).map_err(|diags| {
        format!(
            "re-parsing the image's source failed: {}",
            diags.first().map(|d| d.message.clone()).unwrap_or_default()
        )
    })?;
    let resolved =
        crate::resolve(module_ast, Rc::from(source.as_str()), origin).map_err(|errors| {
            format!(
                "re-resolving the image's source failed: {}",
                errors
                    .first()
                    .map(|e| e.message.clone())
                    .unwrap_or_default()
            )
        })?;
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let root_class = interp.load_module(&resolved);
    let symbols = symbols::decode(&mut r).map_err(|e| e.to_string())?;
    let table = decode::decode_code_table(&mut r, &symbols).map_err(|e| e.to_string())?;
    decode::verify_code_table(&table).map_err(|e| e.to_string())?;
    let mixins = mixins::decode(&mut r, &symbols, &table.codes).map_err(|e| e.to_string())?;
    let module = decode::install_code_table(&table, &mixins, &root_class, &bindings)
        .map_err(|e| e.to_string())?;
    let platform = interp.platform();
    let graph = {
        let ctx = decode::DecodeContext {
            interp: &interp,
            root_class: &root_class,
            codes: &table.codes,
            bindings: &bindings,
            platform,
        };
        decode::decode_graph_section(&mut r, &ctx, &symbols).map_err(|e| e.to_string())?
    };
    decode::verify_graph(&graph, &table, &module).map_err(|e| e.to_string())?;
    interp.advance_identity_hash_past(graph.max_identity_hash);
    interp.set_engine(Engine::Bytecode(module));
    Ok(LoadedImage {
        interp,
        root_class,
        bindings,
        roots: graph.roots,
    })
}
