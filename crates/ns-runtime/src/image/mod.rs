//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/design/image-format.md`. Layout: header, symbol table, code
//! table, mixin table (one tree per module, in module order), module section
//! (per module: `SourceOrigin`, optional source text, root section), then the
//! platform paths, object table, frame table, roots, tag-named hashes.
//!
//! **Loading never runs the front end.** `load` decodes the world; it does
//! not parse, resolve, or compile anything. The symbol table, the mixin
//! table and the class-object records together describe every declaration
//! the world holds, so there is nothing left for a re-parse to rebuild --
//! and the image's source text, which is attacker-controlled input like
//! every other byte in the file, is never handed to the parser. It is not
//! even required to be present: modules handed as `ModuleSource`s with no
//! `source` write an image with no program text in it at all, and modules
//! that all carry theirs write the debug variant, whose source exists so a
//! loaded world can be shown (and, once a decompiler exists, compared) as
//! `.ns`.

pub mod decode;
pub mod encode;
pub mod inspect;
pub mod mixins;
pub mod modules;
pub mod opcodes;
pub mod platform_paths;
pub mod symbols;

pub use modules::{ModuleClass, ModuleClasses, ModuleSource};

use crate::heap::{Heap, Root};
use crate::types::{ClassReference, SourceOrigin, Value};
use crate::{HostSignal, Interpreter, Mutator, Symbol};

/// The sections that describe code and structure, encoded into their own
/// buffers with the symbol table still open so the caller can intern into it.
///
/// **The symbol table stays open.** `symbols` outlives `encode_describing` on
/// purpose, so the graph can intern whatever it names -- `'foo' asSymbol`
/// mints a symbol at run time whose text need not appear in the code or mixin
/// tables otherwise.
struct Describing {
    symbols: symbols::Builder,
    code: encode::Writer,
    mixins: encode::Writer,
    ids: encode::CodeIds,
    mixin_ids: mixins::MixinIds,
}

/// Encodes the code table and the mixin table into their own buffers,
/// collecting every selector and name either one names into `symbols` along
/// the way. The symbol table cannot be written before both are encoded, since
/// it is the table's *contents* that fill it.
fn encode_describing<'gc>(
    heap: &Heap<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
) -> Result<Describing, String> {
    // Each module's declarations, which carry its code. A root that is not a
    // class object has none and contributes no rows rather than an error:
    // the graph section is what refuses such a root.
    let module_roots: Vec<std::rc::Rc<crate::mixin::Mixin>> = modules
        .iter()
        .filter_map(|module| heap.class_application(module.class))
        .map(|application| application.mixin.clone())
        .collect();
    // A root that can never be instantiated could only fail as an artifact,
    // and refusing it here keeps the format from needing a way to say so.
    for root in &module_roots {
        if let Some(defect) = &root.root_clause_defect {
            return Err(format!(
                "image encoding cannot serialize {}: {}",
                root.name,
                defect.describe()
            ));
        }
    }
    let mut symbols = symbols::Builder::new();
    let mut code = encode::Writer::new();
    let ids = encode::encode_code_table(&mut code, &module_roots, &mut symbols)?;
    // After the code table, whose ids its rows name; before the graph, whose
    // class objects might name its rows.
    let mut mixin_writer = encode::Writer::new();
    let mixin_ids = mixins::encode(&mut mixin_writer, &module_roots, &mut symbols, &ids)?;
    Ok(Describing {
        symbols,
        code,
        mixins: mixin_writer,
        ids,
        mixin_ids,
    })
}

/// Writes header through the module section. Everything after that is the
/// caller's: the graph.
///
/// The header says the modules carry source when any one is handed some;
/// `encode_modules` then refuses a module handed none, so a mix is an `Err`
/// rather than an artifact that silently drops one module's text.
fn write_describing<'gc>(
    heap: &Heap<'gc>,
    writer: &mut encode::Writer,
    modules: &[ModuleSource<'_, 'gc>],
    flags: u8,
    describing: Describing,
) -> Result<(), String> {
    let with_source = modules.iter().any(|module| module.source.is_some());
    encode::write_header(
        writer,
        flags
            | if with_source {
                encode::FLAG_HAS_SOURCE
            } else {
                0
            },
    );
    // The symbol table is written here, ahead of the code and mixin tables it
    // fills, so a reader can decode it once before either asks it to resolve
    // an id. `write_image` is done with `describing.mixin_ids` by this point --
    // it has already used it to encode the graph into its own buffer -- so
    // consuming `describing` here loses nothing.
    describing.symbols.write(writer);
    writer.write_raw(&describing.code.into_bytes());
    writer.write_raw(&describing.mixins.into_bytes());
    modules::encode_modules(heap, writer, modules, with_source)
}

/// Serializes a quiesced world: `roots` (the root instance and, if any,
/// the entry send's result) plus everything reachable from them, together
/// with every module's provenance and compiled code. `modules` lists the
/// modules the image carries, the application's first; a graph reaching a
/// class of any other user module is refused.
///
/// A module's `source` is the debug variant's switch. `None` writes a module
/// whose bytes contain no program text -- what a loaded world needs is the
/// code table, the mixin table and the graph, and none of them quotes the
/// source. `Some(source)` additionally stores `source` verbatim; `load` hands
/// it back on `LoadedModule::source` and does nothing else with it. Source
/// travels for every module or for none. The graph closes with the hashes of
/// the objects it names by tag (`encode::encode_tag_named_hashes`), and is
/// preceded by the platform paths it names objects by (`platform_paths`).
pub fn snapshot<'gc>(
    mutator: &Mutator<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    platform: Value<'gc>,
    roots: &[Value<'gc>],
) -> Result<Vec<u8>, String> {
    write_image(mutator, modules, platform, roots, 0)
}

/// Serializes an application assembled but not yet started: its instance is
/// the graph's one root, and `FLAG_START` tells `load` to send it
/// `main: platform args:`.
///
/// No platform exists for it yet, so none is named: the platform value is
/// `nil` here, which means no platform path is discovered and a graph that
/// reaches the platform or anything shipped meets the encoder's refusal of a
/// shipped object no path names. An application that reached the platform
/// could only have been handed it, and nothing hands one out before
/// `main:args:`.
pub fn package<'gc>(
    mutator: &Mutator<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    application: Value<'gc>,
) -> Result<Vec<u8>, String> {
    write_image(
        mutator,
        modules,
        Value::nil(),
        &[application],
        encode::FLAG_START,
    )
}

/// The one writer of a graph-carrying image; `flags` is what tells `load` how
/// to treat it.
fn write_image<'gc>(
    mutator: &Mutator<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    platform: Value<'gc>,
    roots: &[Value<'gc>],
    flags: u8,
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
    let mut describing = encode_describing(heap, modules)?;
    let classes: Vec<ClassReference<'gc>> = modules.iter().map(|module| module.class).collect();
    let module_classes = ModuleClasses::of(heap, &classes);
    // The graph section is encoded into its own buffer, against the same
    // `symbols` builder the describing sections used, before `write_describing`
    // consumes `describing` -- the graph needs the mixin table's row ids, and
    // those exist only on `describing.mixin_ids`.
    let mut graph_writer = encode::Writer::new();
    let used_paths = encode::encode_graph_section(
        heap,
        &mut graph_writer,
        roots,
        platform,
        &describing.ids,
        &module_classes,
        &describing.mixin_ids,
        &mut describing.symbols,
    )?;
    encode::encode_tag_named_hashes(
        mutator,
        &mut graph_writer,
        &module_classes,
        platform,
        &used_paths.objects,
    );
    // Encoded after the graph, which is what decides which paths are used
    // and in what order, and written ahead of it, since the loader resolves
    // every path before it decodes the graph.
    let mut path_writer = encode::Writer::new();
    platform_paths::encode_section(&mut path_writer, &used_paths.paths, &mut describing.symbols);
    let mut writer = encode::Writer::new();
    write_describing(heap, &mut writer, modules, flags, describing)?;
    writer.write_raw(&path_writer.into_bytes());
    writer.write_raw(&graph_writer.into_bytes());
    Ok(writer.into_bytes())
}

/// One module a loaded artifact carried.
pub struct LoadedModule {
    /// The module's root class, rooted for the host that will instantiate
    /// it or snapshot it again.
    pub class: Root,
    /// The module's root declaration, which carries its code: what a host
    /// after the code alone reads, with no heap to reach it through.
    pub declaration: std::rc::Rc<crate::mixin::Mixin>,
    /// Where the module came from. Provenance, not program text -- it is
    /// in every artifact, source or not.
    pub origin: SourceOrigin,
    /// The module's source text, for the debug variant only -- `None` for a
    /// source-free artifact. Never parsed, never resolved, never executed:
    /// `load` reads it out of the byte stream and hands it over unchanged.
    pub source: Option<String>,
}

pub struct LoadedImage {
    /// Bootstrapped. The image's code lives on the declarations its mixin
    /// table rebuilt.
    pub interpreter: Interpreter,
    /// Every module the artifact carried, in module order: module 0 is the
    /// application's.
    pub modules: Vec<LoadedModule>,
    /// The image's roots, rooted: a host sends to them, which runs code.
    pub roots: Vec<Root>,
    /// `header.start` (`FLAG_START`): the image is an application packaged
    /// just before `main:args:`, and its one root is to be sent
    /// `main: platform args:`.
    pub start: bool,
}

/// Decodes an image: symbols, code table, mixin table, module section, then
/// the platform paths and the object graph. Verifies the decoded declarations
/// against their code and advances the identity-hash counter past the graph's
/// maximum.
///
/// Nothing here parses, resolves or compiles. Each module's root class is
/// rebuilt from the first row of its tree in the mixin table
/// (`modules::build_module_classes`), in module order, which is what makes
/// the front end unnecessary -- and, since an artifact is untrusted input,
/// what keeps the parser off the load path entirely.
///
/// Both verification passes run before anything the image describes can be
/// executed: `verify_code_table` and `verify_mixins` before anything is
/// built on the code, `verify_graph` resolving each frame's
/// body off the decoded mixins that carry it. A corrupt but version-matching
/// image is an `Err` here, never a panic in the VM later.
///
/// An image loads in three phases, because its platform paths are resolved
/// by running code and its graph is decoded by running none
/// (`docs/design/core-library.md`, "Platform paths in the image"). The first
/// decodes everything up to and including the platform path section and
/// roots every module's root class and levels: a run may move them, and the
/// mixins and the code hold no handle. The second, outside any entry, sends
/// each path's members to the loading platform -- each step a public slot's
/// or nested class's accessor, sent as an ordinary public send, what the
/// program's own `platform collections` may do -- and refuses a path that
/// takes any other step, fails or answers no shipped object. The third
/// decodes the graph against what the paths answered, and runs nothing.
pub fn load(bytes: &[u8]) -> Result<LoadedImage, String> {
    let mut reader = decode::Reader::new(bytes);
    let header = decode::read_header(&mut reader).map_err(|error| error.to_string())?;
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
        let decoded_modules = modules::decode_modules(&mut reader, &mixins, header.has_source)
            .map_err(|error| error.to_string())?;
        let module_classes = modules::build_module_classes(mutator, &mixins, &decoded_modules)
            .map_err(|error| error.to_string())?;
        let paths = platform_paths::decode_section(&mut reader, &symbols)
            .map_err(|error| error.to_string())?;
        // Rooted, because the next phase runs code, and a run may move them.
        let module_roots = module_classes
            .roots
            .iter()
            .map(|root| mutator.stash(Value::object(*root)))
            .collect();
        let module_levels = module_classes
            .levels
            .iter()
            .map(|levels| {
                levels
                    .iter()
                    .map(|level| mutator.stash(Value::object(*level)))
                    .collect()
            })
            .collect();
        Ok(Described {
            symbols,
            table,
            mixins,
            decoded_modules,
            module_roots,
            module_levels,
            paths,
        })
    })?;
    // Following a path runs shipped code -- a platform member's lazy
    // initializer, a nested class's accessor and superclass clause -- before
    // the graph restores the identity hashes the image recorded, and
    // `advance_identity_hash_past` moves the counter past them only
    // afterwards. A hash drawn now could therefore equal one the graph hands
    // back to a decoded object. Nothing `PlatformFactory.ns` runs on these paths
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
        decoded_modules,
        module_roots,
        module_levels,
        paths: _,
    } = described;
    let roots = interpreter.enter(|mutator| -> Result<Vec<Root>, String> {
        let class_of = |root: &Root| match mutator.fetch(root).as_object() {
            Some(class) => class,
            _ => unreachable!("internal: a module class is a class object"),
        };
        let module_classes = ModuleClasses {
            roots: module_roots.iter().map(class_of).collect(),
            levels: module_levels
                .iter()
                .map(|levels| levels.iter().map(class_of).collect())
                .collect(),
        };
        let platform_paths: Vec<Value> = resolved.iter().map(|path| mutator.fetch(path)).collect();
        let platform = mutator.platform_value();
        let mut context = decode::DecodeContext {
            mutator,
            modules: &module_classes,
            codes: &table.codes,
            platform,
            mixins: &mixins.mixins,
            platform_paths: &platform_paths,
        };
        let graph = decode::decode_graph_section(&mut reader, &mut context, &symbols)
            .map_err(|error| error.to_string())?;
        if header.start && graph.roots.len() != 1 {
            return Err(decode::DecodeError::StartedImageRootCount {
                count: graph.roots.len(),
            }
            .to_string());
        }
        let tag_named_max =
            decode::decode_tag_named_hashes(mutator, &mut reader, &module_classes, &platform_paths)
                .map_err(|error| error.to_string())?;
        decode::verify_graph(&mutator.heap, &graph, &table).map_err(|error| error.to_string())?;
        mutator.advance_identity_hash_past(graph.max_identity_hash.max(tag_named_max));
        // A byte sequence past the last section is refused, not ignored: an
        // unchecked tail would let a padded image load as something its author
        // did not write.
        if reader.remaining() != 0 {
            return Err(decode::DecodeError::TrailingBytes {
                count: reader.remaining(),
            }
            .to_string());
        }
        // Rooted only now, after every verification pass and the trailing-bytes
        // check: nothing runs while the graph decodes, so nothing moves under
        // the plain handles above, and a refused artifact answers nothing to
        // root (`docs/design/image-format.md`, "Packaging").
        Ok(graph
            .roots
            .into_iter()
            .map(|root| mutator.stash(root))
            .collect())
    })?;
    let modules = module_roots
        .into_iter()
        .zip(decoded_modules)
        .enumerate()
        .map(|(index, (class, decoded))| {
            // `build_module_classes` refused a module without a tree already.
            let declaration = mixins
                .module_root(index)
                .cloned()
                .ok_or_else(|| decode::DecodeError::NotAClass.to_string())?;
            Ok(LoadedModule {
                class,
                declaration,
                origin: decoded.origin,
                source: decoded.source,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(LoadedImage {
        interpreter,
        modules,
        roots,
        start: header.start,
    })
}

/// What `load`'s first phase decoded, carried across the second, which runs
/// code: the tables hold no handle, and every module's root class and levels
/// are rooted.
struct Described {
    symbols: symbols::Table,
    table: decode::DecodedCodeTable,
    mixins: mixins::Decoded,
    decoded_modules: Vec<modules::DecodedModule>,
    module_roots: Vec<Root>,
    module_levels: Vec<Vec<Root>>,
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
            // slot's getter, a public nested class's accessor or a public
            // class accessor of the shipped instance reached so far. Any
            // other public method -- `transcript newLine` -- would run
            // arbitrary shipped code while the image is still being read.
            let is_step = interpreter.enter(|mutator| match mutator.fetch(&current).as_object() {
                Some(object) => platform_paths::is_path_step(&mutator.heap, object, *member),
                _ => false,
            });
            if !is_step {
                return Err(format!(
                    "the image names platform path `{described}`, whose member `{}` is no \
                     public slot, nested class or class accessor of what the path has \
                     reached",
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
        let shipped = interpreter.enter(|mutator| match mutator.fetch(&current).as_object() {
            Some(object) => platform_paths::is_shipped_object(&mutator.heap, object),
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
