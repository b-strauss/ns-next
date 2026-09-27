//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/design/image-format.md`. Layout: header, optional source text,
//! `SourceOrigin`, symbol table, code table, mixin table, root class, entry
//! field, then for an image the platform paths, object table, frame table,
//! roots, tag-named hashes.
//!
//! **Loading never runs the front end.** `load` decodes the world; it does
//! not parse, resolve, or compile anything. The symbol table, the mixin
//! table and the class-object records together describe every declaration
//! the world holds, so there is nothing left for a re-parse to rebuild --
//! and the image's source text, which is attacker-controlled input like
//! every other byte in the file, is never handed to the parser. It is not
//! even required to be present: `snapshot(None, ...)` writes an image with
//! no source in it at all, and `snapshot(Some(source), ...)` writes the debug
//! variant, whose source blob exists so a loaded world can be shown (and,
//! once a decompiler exists, compared) as `.ns`.

pub mod decode;
pub mod encode;
pub mod inspect;
pub mod mixins;
pub mod opcodes;
pub mod platform_paths;
pub mod symbols;

use crate::heap::{Heap, Root};
use crate::types::{ClassReference, SourceOrigin, Value};
use crate::{HostSignal, Interpreter, Mutator, Symbol};

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
fn encode_describing<'gc>(
    root_class: ClassReference<'gc>,
    heap: &Heap<'gc>,
) -> Result<Describing, String> {
    // A root that can never be instantiated could only fail as an artifact,
    // and refusing it here keeps the format from needing a way to say so.
    if let Some(application) = heap.class_application(root_class) {
        if let Some(defect) = &application.mixin.root_clause_defect {
            return Err(format!(
                "image encoding cannot serialize {}: {}",
                application.mixin.name,
                defect.describe()
            ));
        }
    }
    let mut symbols = symbols::Builder::new();
    let mut code = encode::Writer::new();
    let ids = encode::encode_code_table(&mut code, heap, root_class, &mut symbols)?;
    // After the code table, whose ids its rows name; before the graph (or the
    // entry field), whose class objects and selector respectively might name
    // its rows.
    let mut mixin_writer = encode::Writer::new();
    let mixin_ids = mixins::encode(&mut mixin_writer, heap, root_class, &mut symbols, &ids)?;
    Ok(Describing {
        symbols,
        code,
        mixins: mixin_writer,
        ids,
        mixin_ids,
    })
}

/// Writes header through root class. Everything after that is the caller's:
/// the entry field, and for an image the graph.
fn write_describing<'gc>(
    writer: &mut encode::Writer,
    source: Option<&str>,
    origin: &SourceOrigin,
    flags: u8,
    describing: Describing,
    root_class: ClassReference<'gc>,
    heap: &Heap<'gc>,
) -> Result<(), String> {
    encode::write_header(
        writer,
        flags
            | match source {
                Some(_) => encode::FLAG_HAS_SOURCE,
                None => 0,
            },
    );
    if let Some(source) = source {
        writer.write_str(source);
    }
    encode::encode_source_origin(writer, origin);
    // The symbol table is written here, ahead of the code and mixin tables it
    // fills, so a reader can decode it once before either asks it to resolve
    // an id. Both callers are done with `describing.mixin_ids` by this point --
    // `snapshot` has already used it to encode the graph into its own buffer,
    // `build` never needed it -- so consuming `describing` here loses nothing.
    describing.symbols.write(writer);
    writer.write_raw(&describing.code.into_bytes());
    writer.write_raw(&describing.mixins.into_bytes());
    encode::encode_root_class(writer, heap, root_class)?;
    Ok(())
}

/// Serializes a quiesced world: `roots` (the root instance and, if any,
/// the entry send's result) plus everything reachable from them, together
/// with the module's provenance and compiled code.
///
/// `source` is the debug variant's switch. `None` writes an image whose bytes
/// contain no program text -- what a loaded world needs is the code table, the
/// mixin table and the graph, and none of them quotes the source.
/// `Some(source)` additionally stores `source` verbatim; `load` hands it back
/// on `LoadedImage::source` and does nothing else with it. The graph closes
/// with the hashes of the objects it names by tag
/// (`encode::encode_tag_named_hashes`), and is preceded by the platform paths
/// it names objects by (`platform_paths`).
pub fn snapshot<'gc>(
    mutator: &Mutator<'gc>,
    source: Option<&str>,
    origin: &SourceOrigin,
    root_class: ClassReference<'gc>,
    platform: &Value<'gc>,
    roots: &[Value<'gc>],
) -> Result<Vec<u8>, String> {
    let heap = &mutator.heap;
    // A frame married to a running activation keeps its registers and
    // `program_counter` on the activation stack, and the format has no record
    // for the stack, so encoding one would write a frame with empty registers
    // and succeed. `Heap::frame_snapshot` debug-asserts against that per frame;
    // this is the check a release build keeps.
    if heap.activation_depth() != 0 {
        return Err(
            "image encoding cannot snapshot a world mid-execution: a snapshot is \
             taken only between sends, when no activation is running"
                .to_string(),
        );
    }
    let mut describing = encode_describing(root_class, heap)?;
    // The graph section is encoded into its own buffer, against the same
    // `symbols` builder the describing sections used, before `write_describing`
    // consumes `describing` -- the graph needs the mixin table's row ids, and
    // those exist only on `describing.mixin_ids`.
    let mut graph_writer = encode::Writer::new();
    let used_paths = encode::encode_graph_section(
        &mut graph_writer,
        heap,
        roots,
        platform,
        &describing.ids,
        root_class,
        &describing.mixin_ids,
        &mut describing.symbols,
    )?;
    encode::encode_tag_named_hashes(
        &mut graph_writer,
        mutator,
        root_class,
        platform,
        &used_paths.objects,
    );
    // Encoded after the graph, which is what decides which paths are used
    // and in what order, and written ahead of it, since the loader resolves
    // every path before it decodes the graph.
    let mut path_writer = encode::Writer::new();
    platform_paths::encode_section(&mut path_writer, &used_paths.paths, &mut describing.symbols);
    let mut writer = encode::Writer::new();
    write_describing(&mut writer, source, origin, 0, describing, root_class, heap)?;
    // `snapshot` never names an entry selector of its own -- a snapshotted
    // world is resumed by loading it and sending to `roots`, not by starting
    // it fresh -- but the field is written unconditionally so every artifact
    // this format can produce shares one layout from here on.
    encode::encode_entry(&mut writer, None);
    writer.write_raw(&path_writer.into_bytes());
    writer.write_raw(&graph_writer.into_bytes());
    Ok(writer.into_bytes())
}

/// Serializes a **stateless binary**: code and structure, no instance state,
/// no activations, and the selector to send on start-up.
///
/// The caller has compiled but deliberately not instantiated. That omission is
/// the whole point of this artifact -- the primary factory and every
/// initializer run on load instead, against the loading world's own freshly
/// bootstrapped platform, so the file has never held a capability.
pub fn build<'gc>(
    heap: &Heap<'gc>,
    source: Option<&str>,
    origin: &SourceOrigin,
    root_class: ClassReference<'gc>,
    entry: Symbol,
) -> Result<Vec<u8>, String> {
    let mut describing = encode_describing(root_class, heap)?;
    // Interned before `write_describing` writes the table below: an id
    // assigned after the table is on the wire is an id the table does not
    // contain, which the decoder reports as an out-of-range symbol id.
    let entry_id = describing.symbols.intern(entry.as_str());
    let mut writer = encode::Writer::new();
    write_describing(
        &mut writer,
        source,
        origin,
        encode::FLAG_STATELESS,
        describing,
        root_class,
        heap,
    )?;
    encode::encode_entry(&mut writer, Some(entry_id));
    Ok(writer.into_bytes())
}

pub struct LoadedImage {
    /// Bootstrapped. The image's code lives on the declarations its mixin
    /// table rebuilt.
    pub interpreter: Interpreter,
    /// The module's root class, rooted for the host that will instantiate
    /// it or snapshot it again.
    pub root_class: Root,
    /// The image's roots, rooted: a host sends to them, which runs code.
    pub roots: Vec<Root>,
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
/// then -- for an image, never a binary -- the platform paths and the object
/// graph. Verifies the decoded declarations against their code and, for an
/// image, advances the identity-hash counter past the graph's maximum.
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
/// executed: `verify_code_table` and `verify_mixins` before anything is
/// built on the code, `verify_graph` (image only) resolving each frame's
/// body off the decoded mixins that carry it. A corrupt but version-matching
/// artifact is an `Err` here, never a panic in the VM later.
///
/// An image loads in three phases, because its platform paths are resolved
/// by running code and its graph is decoded by running none
/// (`docs/design/core-library.md`, "Platform paths in the image"). The first
/// decodes everything up to and including the platform path section and
/// roots the root class and its levels: a run may move them, and the mixins
/// and the code hold no handle. The second, outside any entry, sends each
/// path's members to the loading platform -- each step a public slot's or
/// nested class's accessor, sent as an ordinary public send, what the
/// program's own `platform collections` may do -- and refuses a path that
/// takes any other step, fails or answers no shipped object. The third decodes the graph
/// against what the paths answered, and runs nothing.
pub fn load(bytes: &[u8]) -> Result<LoadedImage, String> {
    let mut reader = decode::Reader::new(bytes);
    let header = decode::read_header(&mut reader).map_err(|error| error.to_string())?;
    let source = if header.has_source {
        Some(reader.read_str().map_err(|error| error.to_string())?)
    } else {
        None
    };
    let origin = decode::decode_source_origin(&mut reader).map_err(|error| error.to_string())?;
    let mut interpreter = Interpreter::bootstrap();
    let described = interpreter.enter(|mutator| -> Result<Described, String> {
        let symbols = symbols::decode(&mut reader).map_err(|error| error.to_string())?;
        let mut table =
            decode::decode_code_table(&mut reader, &symbols).map_err(|error| error.to_string())?;
        decode::verify_code_table(&table).map_err(|error| error.to_string())?;
        // Every body in the table is read and structurally sound now, so a
        // family's call-site numbers -- a block's depend on where its
        // `MakeClosure` sits in its root's emission order -- can be assigned.
        decode::number_call_site_families(&mut table).map_err(|error| error.to_string())?;
        let mixins = mixins::decode(&mut reader, &symbols, &table.codes)
            .map_err(|error| error.to_string())?;
        decode::verify_mixins(&mixins).map_err(|error| error.to_string())?;
        let root_section =
            decode::decode_root_section(&mut reader, &mixins).map_err(|error| error.to_string())?;
        let (root_class, root_levels) = decode::build_root_class(mutator, &mixins, &root_section)
            .map_err(|error| error.to_string())?;
        // `snapshot` always writes this field (as absent); a binary requires it
        // present, since it is the only place a binary records how to start
        // itself once instantiated.
        let entry =
            decode::decode_entry(&mut reader, &symbols).map_err(|error| error.to_string())?;
        if header.stateless && entry.is_none() {
            // A binary that cannot say how it starts is not a deployment
            // artifact -- refusing here keeps the failure where the artifact is
            // read, rather than wherever a caller eventually fails to find
            // something to send.
            return Err("a stateless artifact must record an entry selector".to_string());
        }
        // A binary carries no graph, and so names no path.
        let paths = if header.stateless {
            Vec::new()
        } else {
            platform_paths::decode_section(&mut reader, &symbols)
                .map_err(|error| error.to_string())?
        };
        // Rooted, because the next phase runs code, and a run may move them.
        Ok(Described {
            symbols,
            table,
            mixins,
            root_class: mutator.stash(Value::Object(root_class)),
            root_levels: root_levels
                .into_iter()
                .map(|level| mutator.stash(Value::Object(level)))
                .collect(),
            entry,
            paths,
        })
    })?;
    // Following a path runs shipped code -- a platform member's lazy
    // initializer, a nested class's accessor and superclass clause -- before
    // the graph restores the identity hashes the image recorded, and
    // `advance_identity_hash_past` moves the counter past them only
    // afterwards. A hash drawn now could therefore equal one the graph hands
    // back to a decoded object. Nothing `Platform.ns` runs on these paths
    // draws one today; this is what notices the day something does.
    #[cfg(debug_assertions)]
    let hash_before_paths = interpreter.enter(|mutator| mutator.heap().next_identity_hash());
    let resolved = resolve_platform_paths(&mut interpreter, &described.paths)?;
    #[cfg(debug_assertions)]
    {
        let hash_after_paths = interpreter.enter(|mutator| mutator.heap().next_identity_hash());
        debug_assert_eq!(
            hash_before_paths, hash_after_paths,
            "internal: following the image's platform paths drew an identity hash before the \
             image's own hashes were restored"
        );
    }
    let Described {
        symbols,
        table,
        mixins,
        root_class,
        root_levels,
        entry,
        paths: _,
    } = described;
    let roots = interpreter.enter(|mutator| -> Result<Vec<Root>, String> {
        let Value::Object(root_class_object) = mutator.fetch(&root_class) else {
            unreachable!("internal: the root class is a class object");
        };
        let root_levels: Vec<ClassReference> = root_levels
            .iter()
            .map(|level| match mutator.fetch(level) {
                Value::Object(level) => level,
                _ => unreachable!("internal: a root level is a class object"),
            })
            .collect();
        let platform_paths: Vec<Value> = resolved.iter().map(|path| mutator.fetch(path)).collect();
        let platform = mutator.platform_value();
        // A binary carries no graph section at all -- `decode_graph_section`
        // would read the next artifact's bytes (there are none) as if they were
        // one. `DecodedGraph::empty()` is the graph a binary has: no roots, no
        // objects, no frames, nothing for `verify_graph` to check.
        let graph = if header.stateless {
            decode::DecodedGraph::empty()
        } else {
            let mut context = decode::DecodeContext {
                mutator,
                root_class: &root_class_object,
                codes: &table.codes,
                platform,
                mixins: &mixins.mixins,
                root_levels: &root_levels,
                platform_paths: &platform_paths,
            };
            decode::decode_graph_section(&mut reader, &mut context, &symbols)
                .map_err(|error| error.to_string())?
        };
        if !header.stateless {
            let tag_named_max = decode::decode_tag_named_hashes(
                &mut reader,
                mutator,
                root_class_object,
                &platform_paths,
            )
            .map_err(|error| error.to_string())?;
            decode::verify_graph(&graph, &table, &mutator.heap)
                .map_err(|error| error.to_string())?;
            mutator.advance_identity_hash_past(graph.max_identity_hash.max(tag_named_max));
        }
        // A byte sequence past the last section is refused, not ignored: with
        // two artifact shapes sharing one format, an unchecked tail would let a
        // binary with a graph appended -- or a truncated, then padded, image --
        // load as something its author did not write.
        if reader.remaining() != 0 {
            return Err(decode::DecodeError::TrailingBytes {
                count: reader.remaining(),
            }
            .to_string());
        }
        // Rooted only now, after every verification pass and the trailing-bytes
        // check: nothing runs while the graph decodes, so nothing moves under
        // the plain handles above, and a refused artifact answers nothing to
        // root (`docs/design/image-format.md`, "Two artifacts").
        Ok(graph
            .roots
            .into_iter()
            .map(|root| mutator.stash(root))
            .collect())
    })?;
    Ok(LoadedImage {
        interpreter,
        root_class,
        roots,
        origin,
        source,
        entry,
        stateless: header.stateless,
    })
}

/// What `load`'s first phase decoded, carried across the second, which runs
/// code: the tables hold no handle, and the root class and its levels are
/// rooted.
struct Described {
    symbols: symbols::Table,
    table: decode::DecodedCodeTable,
    mixins: mixins::Decoded,
    root_class: Root,
    root_levels: Vec<Root>,
    entry: Option<Symbol>,
    paths: Vec<Vec<Symbol>>,
}

/// Sends each path's members to the loading interpreter's platform, one
/// unary send each, and answers what every path reached, rooted, in section
/// order. Each step must be one `discover` could take
/// (`platform_paths::is_path_step`), checked before it is sent. A step that
/// is none, or a send that fails, refuses the image, naming the path, and so
/// does a
/// final answer that is no shipped object
/// (`platform_paths::is_shipped_object`): what an image names by a path must
/// be something the loading binary owns, never an object a send made on the
/// spot.
fn resolve_platform_paths(
    interpreter: &mut Interpreter,
    paths: &[Vec<Symbol>],
) -> Result<Vec<Root>, String> {
    let mut resolved = Vec::with_capacity(paths.len());
    for path in paths {
        let described = platform_paths::describe(path);
        let mut current = interpreter.platform();
        for member in path {
            // Only a step `discover` could have taken is sent: a public
            // slot's getter or a public nested class's accessor of the
            // shipped instance reached so far. Any other public method --
            // `transcript newLine` -- would run arbitrary shipped code while
            // the image is still being read.
            let is_step = interpreter.enter(|mutator| match mutator.fetch(&current) {
                Value::Object(object) => {
                    platform_paths::is_path_step(&mutator.heap, object, *member)
                }
                _ => false,
            });
            if !is_step {
                return Err(format!(
                    "the image names platform path `{described}`, whose member `{}` is no \
                     public slot or nested class of what the path has reached",
                    member.as_str()
                ));
            }
            current = match interpreter.send(&current, *member, &[]) {
                Ok(answer) => answer,
                Err(HostSignal::Error(error)) => {
                    return Err(format!(
                        "the image names platform path `{described}`, which the loading \
                         platform cannot follow: {}",
                        error.message
                    ))
                }
                Err(HostSignal::Return) => {
                    return Err(format!(
                        "the image names platform path `{described}`, whose sends escaped \
                         with a non-local return"
                    ))
                }
            };
        }
        let shipped = interpreter.enter(|mutator| match mutator.fetch(&current) {
            Value::Object(object) => platform_paths::is_shipped_object(&mutator.heap, object),
            _ => false,
        });
        if !shipped {
            return Err(format!(
                "the image names platform path `{described}`, which answers no object of the \
                 loading binary's core library"
            ));
        }
        resolved.push(current);
    }
    Ok(resolved)
}
