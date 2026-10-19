//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/design/image-format.md`. Layout: header (magic, format version,
//! flags, the shipped world's fingerprint), symbol table, code
//! table, text table (only with `FLAG_HAS_SOURCE`: each declaration's own
//! text, `texts`), mixin table (one tree per module, in module order, each
//! row naming its text when the image carries source), module section
//! (per module: `SourceOrigin`, the object id of its root class record),
//! then the shipped names, object table (frames included), roots,
//! tag-named hashes.
//!
//! **Loading never runs the front end.** `load` decodes the world; it does
//! not parse, resolve, or compile anything. The symbol table, the mixin
//! table and the class-object records together describe every declaration
//! the world holds, so there is nothing left for a re-parse to rebuild --
//! and the image's source text, which is attacker-controlled input like
//! every other byte in the file, is never handed to the parser by the
//! load; only an edit after it parses a text. It is not even required to
//! be present: modules handed as `ModuleSource`s without `with_source`
//! write an image with no program text in it at all, and modules that all
//! ask for theirs write an image that carries each declaration's text.

pub mod decode;
pub mod encode;
pub mod fingerprint;
pub mod inspect;
pub mod mixins;
pub mod modules;
pub mod opcodes;
pub mod shipped_names;
pub mod symbols;
pub mod texts;

pub use modules::ModuleSource;

use crate::heap::{Heap, Root};
use crate::types::{ClassReference, SourceOrigin, Value};
use crate::{Interpreter, Mutator};

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
    /// The text table, when the modules carry source.
    texts: Option<encode::Writer>,
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
    // The header has one flag for the source of every module, so a module
    // without text while another carries its own could only be written by
    // inventing an empty text for it.
    let with_source = modules.iter().any(|module| module.with_source);
    if with_source && !modules.iter().all(|module| module.with_source) {
        return Err(
            "image encoding carries source for every module or for none: one module has none"
                .to_string(),
        );
    }
    let mut symbols = symbols::Builder::new();
    let mut code = encode::Writer::new();
    // Planned once, from each declaration's current version, so that the
    // code table, the text table and the mixin table describe one program:
    // the program as an edit left it, each declaration's text once.
    let rows = mixins::plan(heap, &module_roots)?;
    let ids = encode::encode_code_table(&mut code, &rows, &[], &mut symbols)?;
    // The texts the heap holds, each declaration's own -- never the host's
    // file, which after an edit is no longer the program's text.
    let (texts, text_ids) = if with_source {
        let mut texts = encode::Writer::new();
        let text_ids = texts::encode(&mut texts, &rows)?;
        (Some(texts), Some(text_ids))
    } else {
        (None, None)
    };
    // After the code table and the text table, whose ids its rows name;
    // before the graph, whose class objects might name its rows.
    let mut mixin_writer = encode::Writer::new();
    let mixin_ids = mixins::encode(
        &mut mixin_writer,
        &rows,
        &mut symbols,
        &ids,
        text_ids.as_ref(),
    )?;
    Ok(Describing {
        symbols,
        code,
        texts,
        mixins: mixin_writer,
        ids,
        mixin_ids,
    })
}

/// The current text of the module whose root declaration is `root`, as
/// the heap holds it: assembled from its declarations' current versions
/// (`Heap::module_source`). `None` for a module with no text.
fn module_text(heap: &Heap<'_>, root: &std::rc::Rc<crate::mixin::Mixin>) -> Option<String> {
    match heap.known_mixin_id(root) {
        Some(id) => heap.module_source(id),
        None => heap.assembled_text(root),
    }
}

/// Writes header through the module section. Everything after that is the
/// caller's: the graph.
///
/// The header says the image carries source when `encode_describing`
/// wrote the text table, which it does for every module or for none.
fn write_describing(
    writer: &mut encode::Writer,
    modules: &[ModuleSource<'_, '_>],
    module_root_ids: &[u32],
    flags: u8,
    describing: Describing,
) {
    encode::write_header(
        writer,
        flags
            | if describing.texts.is_some() {
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
    if let Some(texts) = describing.texts {
        writer.write_raw(&texts.into_bytes());
    }
    writer.write_raw(&describing.mixins.into_bytes());
    modules::encode_modules(writer, modules, module_root_ids);
}

/// Serializes a quiesced world: `roots` (the root instance and, if any,
/// the entry send's result) plus everything reachable from them, together
/// with every module's provenance and compiled code. `modules` lists the
/// modules the image carries, the application's first; a graph reaching a
/// class of any other user module is refused.
///
/// A module's `with_source` says whether the image carries its source.
/// `false` writes a module whose bytes contain no program text -- what a
/// loaded world needs is the code table, the mixin table and the graph,
/// and none of them quotes the source. `true` additionally stores the text
/// of each of its declarations as the heap holds it (`Mixin::text`), in
/// the text table; `load` gives each decoded mixin its own and hands the
/// module's text, assembled from them, back on `LoadedModule::source`.
/// Source travels for every module or for none, and a module built
/// without it cannot be written with it. The graph closes with the hashes of
/// the objects it names by tag (`encode::encode_tag_named_hashes`), and is
/// preceded by the shipped names it names the binary's own objects by
/// (`shipped_names`); the platform, if the graph reaches it, is an ordinary
/// record.
pub fn snapshot<'gc>(
    mutator: &Mutator<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    roots: &[Value<'gc>],
) -> Result<Vec<u8>, String> {
    write_image(mutator, modules, roots, 0)
}

/// Serializes an application assembled but not yet started: its instance is
/// the graph's one root, and `FLAG_START` tells `load` to send it
/// `main: platform args:`.
///
/// No platform exists for it yet, and before `main:args:` a program reaches
/// nothing of the shipped world but the kernel, so a graph naming anything
/// of another shipped module -- `PlatformFactory`, `CollectionsFactory`,
/// `TypedArraysFactory` and the rest -- is refused, and so is one
/// naming a module's instance, the kernel's included, which no program
/// reaches: the loader refuses such a started image too.
pub fn package<'gc>(
    mutator: &Mutator<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    application: Value<'gc>,
) -> Result<Vec<u8>, String> {
    write_image(mutator, modules, &[application], encode::FLAG_START)
}

/// The one writer of a graph-carrying image; `flags` is what tells `load` how
/// to treat it.
fn write_image<'gc>(
    mutator: &Mutator<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    roots: &[Value<'gc>],
    flags: u8,
) -> Result<Vec<u8>, String> {
    let heap = &mutator.heap;
    // A frame married to a running activation keeps its registers and
    // `program_counter` on the activation stack, and the format has no record
    // for the stack, so encoding one would write a frame with empty registers
    // and succeed. `Heap::frame_record` debug-asserts against that per frame;
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
    // The graph section is encoded into its own buffer, against the same
    // `symbols` builder the describing sections used, before `write_describing`
    // consumes `describing` -- the graph needs the mixin table's row ids, and
    // those exist only on `describing.mixin_ids`. It answers the ids of the
    // module roots' records, which the module section names.
    let mut graph_writer = encode::Writer::new();
    let encode::EncodedGraph {
        mut used,
        module_roots,
    } = encode::encode_graph_section(
        mutator,
        &mut graph_writer,
        roots,
        &describing.ids,
        &classes,
        &describing.mixin_ids,
        &mut describing.symbols,
    )?;
    encode::encode_tag_named_hashes(mutator, &mut graph_writer, &mut used);
    // Before `main:args:` nothing hands a program the platform, so nothing
    // of the shipped world but the kernel can be in its graph, and no
    // module's instance, the kernel's included, since a frame the graph
    // holds would hand it to the program; the loader refuses a started
    // image naming more, so none is written.
    if flags & encode::FLAG_START != 0 {
        let kernel = crate::Symbol::intern(crate::interpreter::core_library::KERNEL_MODULE);
        if let Some(name) = used.names(mutator).into_iter().find(|name| {
            name.module != kernel || name.kind == shipped_names::ShippedKind::ModuleInstance
        }) {
            return Err(format!(
                "image encoding refuses {} in a started application: nothing before \
                 main:args: reaches it",
                name.describe()
            ));
        }
    }
    // Encoded after the graph and its hashes, which decide which names are
    // used and in what order, and written ahead of the graph, since the
    // loader resolves every name before it decodes the graph.
    let mut names_writer = encode::Writer::new();
    shipped_names::encode_section(
        &mut names_writer,
        &used.names(mutator),
        &mut describing.symbols,
    );
    let mut writer = encode::Writer::new();
    write_describing(&mut writer, modules, &module_roots, flags, describing);
    writer.write_raw(&names_writer.into_bytes());
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
    /// The module's source text, assembled once at load from the texts of
    /// its declarations (`Heap::module_source`) -- `None` for a
    /// source-free artifact. Never parsed, never resolved, never executed
    /// by the load.
    pub source: Option<std::rc::Rc<str>>,
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
/// the shipped names and the object graph. Verifies the decoded declarations
/// against their code and advances the identity-hash counter past the graph's
/// maximum.
///
/// Nothing here parses, resolves or compiles. Each module's root class is a
/// record of the graph like any class, built over the first row of its tree
/// in the mixin table, which is what makes the front end unnecessary -- and,
/// since an artifact is untrusted input, what keeps the parser off the load
/// path entirely. The record the module section names is held to its
/// module's root row and to the root rule (`modules::module_root_classes`).
///
/// Both verification passes run before anything the image describes can be
/// executed: `verify_code_table` and `verify_mixins` before anything is
/// built on the code, `verify_graph` resolving each frame's
/// body off the decoded mixins that carry it. A corrupt but version-matching
/// image is an `Err` here, never a panic in the VM later.
///
/// An image loads in two phases, and nothing the image describes runs
/// between them or in either. The first decodes everything up to and
/// including the shipped-name section; it builds no object and keeps the
/// names, not handles. Then the loader builds the binary's own shipped
/// modules and classes the section names that the boot has not built
/// (`Interpreter::build_shipped_names`) -- by the binary's own code, the
/// build a program's first demand runs, after a check of every name that
/// refuses an unknown module or declaration, and in a started image a name
/// of any module but the kernel, before anything is built -- and resolves
/// every name against the shipped world by table lookup alone
/// (`shipped_names::resolve`), refusing a name the world does not have and
/// a name given twice. Both happen before the tenure, so the builds'
/// objects are old with the rest of the shipped world. After the tenure,
/// the second phase resolves the names again inside its own entry -- a
/// lookup, no allocation -- and decodes the graph against them.
pub fn load(bytes: &[u8]) -> Result<LoadedImage, String> {
    let mut reader = decode::Reader::new(bytes);
    let header = decode::read_header(&mut reader).map_err(|error| error.to_string())?;
    let mut interpreter = Interpreter::bootstrap();
    let described = decode_described(&mut reader, header)?;
    // Outside any entry, since a build runs the binary's own initializers
    // and accessors. The check of every name comes first, so a name this
    // binary does not have builds nothing.
    interpreter
        .build_shipped_names(&described.names, header.start)
        .map_err(|error| error.to_string())?;
    interpreter.enter(|mutator| {
        shipped_names::resolve(&described.names, &mutator.shipped_world, header.start)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })?;
    // Every object the graph will name is old from here on: the shipped
    // world (decided with the user on 2026-10-04;
    // `docs/design/image-format.md`, "The image under generations"). The
    // graph pass then allocates old, and its stores are old-to-old.
    interpreter.tenure_now();
    let Described {
        symbols,
        table,
        mixins,
        decoded_modules,
        names,
    } = described;
    let (roots, module_classes) =
        interpreter.enter(|mutator| -> Result<(Vec<Root>, Vec<Root>), String> {
            mutator.heap.set_allocating_old(true);
            let decoded = decode_graph_phase(
                mutator,
                &mut reader,
                header,
                &symbols,
                &table,
                &mixins,
                &decoded_modules,
                &names,
            );
            // Cleared on the refusal path too: the interpreter of a refused
            // image is dropped, but nothing should rest on that.
            mutator.heap.set_allocating_old(false);
            decoded
        })?;
    let modules = module_classes
        .into_iter()
        .zip(decoded_modules)
        .enumerate()
        .map(|(index, (class, decoded))| {
            // `decode_modules` refused a module without a tree already.
            let declaration = mixins
                .module_root(index)
                .cloned()
                .ok_or_else(|| decode::DecodeError::NotAClass.to_string())?;
            let source = interpreter
                .enter(|mutator| module_text(&mutator.heap, &declaration))
                .map(std::rc::Rc::from);
            Ok(LoadedModule {
                class,
                declaration,
                origin: decoded.origin,
                source,
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

/// The first phase of `load`: everything up to and including the
/// shipped-name section, decoded and verified. It reads bytes only -- no
/// heap, no world -- so it needs no entry.
fn decode_described(
    reader: &mut decode::Reader<'_>,
    header: decode::Header,
) -> Result<Described, String> {
    let symbols = symbols::decode(reader).map_err(|error| error.to_string())?;
    let mut table =
        decode::decode_code_table(reader, &symbols).map_err(|error| error.to_string())?;
    decode::verify_code_table(&table).map_err(|error| error.to_string())?;
    // Every body in the table is read and structurally sound now, so a
    // family's call-site numbers -- a block's depend on where its
    // `MakeClosure` sits in its root's emission order -- can be assigned.
    decode::number_call_site_families(&mut table).map_err(|error| error.to_string())?;
    let texts = if header.has_source {
        texts::decode(reader).map_err(|error| error.to_string())?
    } else {
        Vec::new()
    };
    let read = mixins::read(reader, &symbols, &table.codes, header.has_source)
        .map_err(|error| error.to_string())?;
    let decoded_modules =
        modules::decode_modules(reader, &read.module_rows).map_err(|error| error.to_string())?;
    let mixins = mixins::build(read, &table.codes, &texts).map_err(|error| error.to_string())?;
    decode::verify_mixins(&mixins).map_err(|error| error.to_string())?;
    let names =
        shipped_names::decode_section(reader, &symbols).map_err(|error| error.to_string())?;
    Ok(Described {
        symbols,
        table,
        mixins,
        decoded_modules,
        names,
    })
}

/// The second phase of `load`: the graph against the shipped names,
/// resolved again in this entry, run with the heap allocating old. Answers
/// the graph's roots and every module's root class record, rooted.
#[allow(clippy::too_many_arguments)]
fn decode_graph_phase<'gc>(
    mutator: &mut Mutator<'gc>,
    reader: &mut decode::Reader<'_>,
    header: decode::Header,
    symbols: &symbols::Table,
    table: &decode::DecodedCodeTable,
    mixins: &mixins::Decoded,
    decoded_modules: &[modules::DecodedModule],
    names: &[shipped_names::ShippedName],
) -> Result<(Vec<Root>, Vec<Root>), String> {
    let shipped = shipped_names::resolve(names, &mutator.shipped_world, header.start)
        .map_err(|error| error.to_string())?;
    let module_root_ids: Vec<u32> = decoded_modules.iter().map(|module| module.root).collect();
    let mut context = decode::DecodeContext {
        mutator,
        module_roots: &module_root_ids,
        codes: &table.codes,
        mixins: &mixins.mixins,
        shipped: &shipped,
    };
    let graph = decode::decode_graph_section(reader, &mut context, symbols)
        .map_err(|error| error.to_string())?;
    if header.start && graph.roots.len() != 1 {
        return Err(decode::DecodeError::StartedImageRootCount {
            count: graph.roots.len(),
        }
        .to_string());
    }
    let module_classes =
        modules::module_root_classes(mutator, mixins, decoded_modules, &graph.objects)
            .map_err(|error| error.to_string())?;
    let tag_named_max = decode::decode_tag_named_hashes(mutator, reader, &shipped)
        .map_err(|error| error.to_string())?;
    decode::verify_graph(&mutator.heap, &graph, table).map_err(|error| error.to_string())?;
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
    let roots = graph
        .roots
        .into_iter()
        .map(|root| mutator.stash(root))
        .collect();
    let module_classes = module_classes
        .into_iter()
        .map(|class| mutator.stash(Value::object(class)))
        .collect();
    Ok((roots, module_classes))
}

/// What the first phase of `load` decoded, carried across the tenure, which
/// moves objects: the tables, the modules and the names hold no handle.
struct Described {
    symbols: symbols::Table,
    table: decode::DecodedCodeTable,
    mixins: mixins::Decoded,
    decoded_modules: Vec<modules::DecodedModule>,
    names: Vec<shipped_names::ShippedName>,
}
