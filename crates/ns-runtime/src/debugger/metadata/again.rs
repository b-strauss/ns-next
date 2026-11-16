//! Compiling a declaration's level again for the debugger's metadata
//! (`docs/design/debugger.md`, decisions 5, 11 and 12, "Compiling the
//! metadata again"). The level is found by its declaration's path in its
//! module's current text, compiled with a `MetadataCollector`, and each
//! running body paired with the body compiled again in the same place; a
//! pair gets the tables only when the two compile alike
//! (`bytecode::compiles_alike`). What is compiled is only compared and its
//! tables kept: it is registered nowhere, installed nowhere and never run.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ns_syntax::{ast, Span};

use super::{address, BodyMetadata, MetadataCollector, VariableEntry, VariablePlace};
use crate::bytecode::{compiles_alike, CompiledCode};
use crate::compiler::Lowering;
use crate::heap::MixinId;
use crate::interpreter::core_library::{
    resolved_shipped_module, shipped_module_text, KERNEL_MODULE, SHIPPED_MODULES,
};
use crate::interpreter::edit::find_declaration;
use crate::interpreter::instantiate::{pattern_parameter_count, InitializerBody, ModuleSource};
use crate::interpreter::Mutator;
use crate::mixin::{MethodEntry, Mixin};
use crate::types::{ResolvedModule, SourceOrigin};
use crate::Symbol;

/// Which body of which declaration a frame runs.
pub(crate) struct BodyOrigin {
    /// The version of the mixin the frame's level applies -- possibly one
    /// an edit superseded, which keeps its id (`Heap::known_mixin_id`).
    /// Held while the metadata is looked up, and by a step's recorded
    /// call (`stepping::RecordedCall`) until the step's stop names it:
    /// no cache keeps a mixin, so that a superseded version is dropped as
    /// soon as nothing runs it and no step in progress names a call of it.
    pub(crate) mixin: Rc<Mixin>,
    pub(crate) member: Member,
}

/// The member of a mixin a body belongs to.
#[derive(Clone)]
pub(crate) enum Member {
    /// A method, or a lazy slot's getter, of the mixin's side.
    Method(Symbol),
    Initializer(
        #[expect(
            dead_code,
            reason = "a frame is named alike for every initializer body; a breakpoint's path is to name which"
        )]
        InitializerBody,
    ),
    SuperclassClause,
    /// The primary factory, whose body is synthesized from its arity
    /// (`compiler::factory_body`), so that a frame of it may run a body no
    /// mixin holds: a host's `instantiate` builds its own.
    Factory,
}

/// Each running body of a level, blocks included, with its metadata, or
/// `None` where the body compiled again differs.
pub(crate) type PairedBodies = Vec<(Rc<CompiledCode>, Option<Rc<BodyMetadata>>)>;

/// A level compiled again: its paired bodies, the factory's metadata,
/// which any body of the factory's arity takes, and the module's text it
/// was compiled from, which every span of the metadata counts in.
pub(crate) struct LevelAgain {
    pub(crate) pairs: PairedBodies,
    pub(crate) factory: FactoryAgain,
    pub(crate) text: Rc<str>,
}

/// The factory's metadata as the class header gives it (decision 12), and
/// the arity of the body it belongs to.
pub(crate) struct FactoryAgain {
    pub(crate) argument_count: usize,
    pub(crate) metadata: Rc<BodyMetadata>,
}

impl FactoryAgain {
    /// The factory's metadata when `code` is the factory body of its
    /// arity, as every factory of that arity runs.
    pub(crate) fn metadata_of(&self, code: &CompiledCode) -> Option<Rc<BodyMetadata>> {
        let body =
            crate::compiler::factory_body(u16::try_from(self.argument_count).unwrap_or(u16::MAX));
        compiles_alike(code, &body).then(|| Rc::clone(&self.metadata))
    }
}

/// Compiles the level of `running`'s declaration again -- its own methods,
/// initializer bodies, clause and class side, never its nested
/// declarations -- under `lowering`, collecting, and answers each running
/// body of that level, blocks included, paired with its metadata: those of
/// `running`'s side, and for an instance side those of its class side too.
/// `None` when the module has no text, or the text does not parse, resolve
/// or hold the declaration -- an image's text is untrusted, so none of them
/// panics.
pub(crate) fn compile_level_again(
    mutator: &mut Mutator<'_>,
    running: &Rc<Mixin>,
    lowering: Lowering,
) -> Option<PairedBodies> {
    compile_level(mutator, running, lowering).map(|level| level.pairs)
}

/// `compile_level_again`, with the factory's metadata beside the pairs.
pub(crate) fn compile_level(
    mutator: &mut Mutator<'_>,
    running: &Rc<Mixin>,
    lowering: Lowering,
) -> Option<LevelAgain> {
    let instance_id = declaration_id(mutator, running)?;
    let (module_root, path) = mutator.path_from_module_root(instance_id);
    // Asked of the module's root, not of `running`: a class side
    // synthesized for a shipped declaration is not marked shipped.
    let root_mixin = mutator.heap.mixin(module_root)?;
    let (resolved, source, module_text) = if root_mixin.shipped {
        shipped_resolution(root_mixin.name.as_str())?
    } else {
        let text = mutator.heap.module_source(module_root)?;
        let module = ns_syntax::parse(&text).ok()?;
        let text = mutator.debug_metadata.module_text(module_root, text);
        // Never `resolve_kernel`: only shipped source records mirror
        // sends.
        let resolved = crate::resolver::resolve(
            module,
            Rc::clone(&text),
            SourceOrigin::Embedded("<debugger>".into()),
        )
        .ok()?;
        (Rc::new(resolved), ModuleSource::User, text)
    };
    let root = &resolved.module.root_class;
    let declaration = find_declaration(root, &path)?;
    let collector = MetadataCollector::new();
    let fresh = mutator.mixin_level(
        declaration,
        root,
        &resolved.bindings,
        source,
        &module_text,
        lowering,
        Some(&collector),
        |_| Vec::new(),
    );
    let factory = factory_again(declaration);
    let mut pairing = Pairing {
        collector: &collector,
        taken: HashMap::new(),
        seen: HashSet::new(),
        factory: &factory,
        pairs: Vec::new(),
    };
    if running.is_class_side() {
        pairing.side(running, fresh.class_side.as_deref());
    } else {
        pairing.side(running, Some(&fresh));
        // A declared class side, or the one synthesized for a declaration
        // that declares none, which holds the factory alone.
        let class_side = running
            .class_side
            .clone()
            .or_else(|| mutator.metaclass_mixins.get(&instance_id).cloned());
        if let Some(class_side) = class_side {
            pairing.side(&class_side, fresh.class_side.as_deref());
        }
    }
    Some(LevelAgain {
        pairs: pairing.pairs,
        factory,
        text: module_text,
    })
}

/// Every body of `declaration`'s level -- its methods, initializer
/// bodies, clause and class side, never its nested declarations -- and
/// every block they create, compiled from a resolution no interpreter
/// loaded, each with the metadata collected for it: the bodies a module's
/// `SetBreakpoints` would resolve against had it been loaded, compiled by
/// the same function `compile_level` compiles a loaded level again with.
/// Each body is paired with itself, so that what is answered is exactly
/// what a pairing would keep. `source` is the kind the module is compiled
/// as when it is loaded: a shipped module's is its boot's
/// (`shipped_resolution`). Registers nothing and runs nothing.
pub(crate) fn compile_unloaded_level(
    mutator: &mut Mutator<'_>,
    resolved: &ResolvedModule,
    declaration: &ast::ClassDeclaration,
    source: ModuleSource,
    lowering: Lowering,
) -> PairedBodies {
    let collector = MetadataCollector::new();
    let fresh = mutator.mixin_level(
        declaration,
        &resolved.module.root_class,
        &resolved.bindings,
        source,
        &resolved.source,
        lowering,
        Some(&collector),
        |_| Vec::new(),
    );
    let factory = factory_again(declaration);
    let mut pairing = Pairing {
        collector: &collector,
        taken: HashMap::new(),
        seen: HashSet::new(),
        factory: &factory,
        pairs: Vec::new(),
    };
    pairing.side(&fresh, Some(&fresh));
    if let Some(class_side) = fresh.class_side.as_deref() {
        pairing.side(class_side, Some(class_side));
    }
    pairing.pairs
}

/// The id of the declaration `running` is a version of, or of whose
/// class side it is a version; `None` for a mixin no heap registered.
fn declaration_id(mutator: &Mutator<'_>, running: &Rc<Mixin>) -> Option<MixinId> {
    // A class side's declaration is its instance side's. A class side no
    // metaclass applies yet has no id of its own, but its instance side
    // has one.
    if running.is_class_side() {
        match mutator.heap.known_mixin_id(running) {
            Some(id) => mutator.heap.enclosing_mixin_id(id),
            None => mutator.heap.known_mixin_id(&running.enclosing_mixin()?),
        }
    } else {
        mutator.heap.known_mixin_id(running)
    }
}

/// The module `running` is declared in, as a frame's location names it;
/// `None` for a mixin no heap registered.
pub(crate) fn module_name(
    mutator: &Mutator<'_>,
    running: &Rc<Mixin>,
) -> Option<crate::debugger::ModuleName> {
    let (root, _) = declaration_path(mutator, running)?;
    Some(crate::debugger::ModuleName::of_root(&root))
}

/// The root class of the module `running` is declared in, and the names
/// of the declarations from that root down to `running`'s, the root's
/// own excluded -- empty for the root itself; a class side's is its
/// instance side's. `None` for a mixin no heap registered.
pub(crate) fn declaration_path(
    mutator: &Mutator<'_>,
    running: &Rc<Mixin>,
) -> Option<(Rc<Mixin>, Vec<Symbol>)> {
    let (module_root, path) = mutator.path_from_module_root(declaration_id(mutator, running)?);
    Some((mutator.heap.mixin(module_root)?, path))
}

/// The shipped resolution of the shipped module whose root is named
/// `name`, the boot's own (`interpreter::kernel::resolved_kernel`,
/// `core_library::resolved_shipped_module`), so that the bodies' mirror
/// sends are the same; `None` for a name no shipped module has.
pub(crate) fn shipped_resolution(
    name: &str,
) -> Option<(Rc<ResolvedModule>, ModuleSource, Rc<str>)> {
    if name == KERNEL_MODULE {
        return Some((
            crate::interpreter::kernel::resolved_kernel(),
            ModuleSource::Kernel,
            shipped_module_text(name),
        ));
    }
    SHIPPED_MODULES
        .iter()
        .any(|shipped| shipped.name == name)
        .then(|| {
            (
                resolved_shipped_module(name),
                ModuleSource::CoreLibrary,
                shipped_module_text(name),
            )
        })
}

/// The factory's metadata from `declaration`'s header (decision 12): its
/// parameters, named as the header names them, in the registers a factory
/// body takes its arguments in; no statement start, since the source
/// writes none, and no send; and the header's factory pattern as where it
/// stands, or the class's name for a header that writes none.
fn factory_again(declaration: &ast::ClassDeclaration) -> FactoryAgain {
    let (parameters, argument_count, position) = match &declaration.factory {
        Some(pattern) => (
            crate::resolver::pattern_parameter_names(pattern),
            pattern_parameter_count(pattern),
            pattern_span(pattern),
        ),
        None => (Vec::new(), 0, declaration.name.span),
    };
    let variables = parameters
        .into_iter()
        .enumerate()
        .map_while(|(position, name)| {
            Some(VariableEntry {
                name,
                depth: 0,
                place: VariablePlace::Register(u16::try_from(position).ok()?),
            })
        })
        .collect();
    FactoryAgain {
        argument_count,
        metadata: Rc::new(BodyMetadata {
            statements: Box::new([]),
            sends: Box::new([]),
            block_sends: Box::new([]),
            variables,
            factory_pattern: Some(position),
            header_pattern: None,
        }),
    }
}

/// From a message pattern's first name to its last.
fn pattern_span(pattern: &ast::MessagePattern) -> Span {
    match pattern {
        ast::MessagePattern::Unary { selector, .. } => selector.span,
        ast::MessagePattern::Binary {
            selector,
            parameter,
        } => selector.span.merge(parameter.name.span),
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| part.keyword.span.merge(part.parameter.name.span))
            .reduce(Span::merge)
            .unwrap_or(Span::new(0, 0)),
    }
}

/// Pairs running bodies with the bodies compiled again, member by member.
struct Pairing<'level> {
    collector: &'level MetadataCollector,
    /// Each body compiled again whose tables were taken, by address: a
    /// mixin may hold one body in two fields (a full initializer that is
    /// its own part), and the collector answers its tables only once.
    taken: HashMap<usize, Option<Rc<BodyMetadata>>>,
    /// Each running body already paired, by address.
    seen: HashSet<usize>,
    factory: &'level FactoryAgain,
    pairs: PairedBodies,
}

impl Pairing<'_> {
    /// Pairs every body of `running`, one side of a level, with `fresh`,
    /// the same side compiled again, if it has one: methods by selector,
    /// the initializer bodies and the clause by field, the factory with
    /// the header's. An initializer body stands at the header too, at its
    /// header stop (decision 12): its metadata takes the factory's place.
    fn side(&mut self, running: &Mixin, fresh: Option<&Mixin>) {
        for (selector, entry) in &running.methods {
            match entry {
                MethodEntry::Compiled(method) => {
                    let again = fresh
                        .and_then(|fresh| fresh.compiled_method(*selector))
                        .map(|method| Rc::clone(&method.code));
                    self.family(&method.code, again, None);
                }
                MethodEntry::Factory(factory) => {
                    if self.seen.insert(address(&factory.code)) {
                        let metadata = self.factory.metadata_of(&factory.code);
                        self.pairs.push((Rc::clone(&factory.code), metadata));
                    }
                }
                MethodEntry::SlotAccessor(_) | MethodEntry::Primitive(_) => {}
            }
        }
        for kind in [
            InitializerBody::Full,
            InitializerBody::OwnPart,
            InitializerBody::Standalone,
        ] {
            if let Some(code) = kind.code_of(running) {
                let header = self.factory.metadata.factory_pattern;
                self.family(&code, fresh.and_then(|fresh| kind.code_of(fresh)), header);
            }
        }
        if let Some(code) = &running.superclass_clause_code {
            let again = fresh.and_then(|fresh| fresh.superclass_clause_code.clone());
            self.family(code, again, None);
        }
    }

    /// Pairs `running` and every block it creates with `again` and its
    /// blocks, in creation order. A family whose root does not compile
    /// alike gets no tables at all: a block's tables name its creators'
    /// registers, which only an equal root vouches for. A worklist, not a
    /// recursion: a running body may be an image's, whose nesting the
    /// text compiled again does not bound. `header` is where the root
    /// stands at its header stop, an initializer's; its blocks have none.
    fn family(
        &mut self,
        running: &Rc<CompiledCode>,
        again: Option<Rc<CompiledCode>>,
        header: Option<Span>,
    ) {
        let again = again.filter(|again| compiles_alike(running, again));
        let root = address(running);
        let mut pending = vec![(Rc::clone(running), again)];
        while let Some((running, again)) = pending.pop() {
            if !self.seen.insert(address(&running)) {
                continue;
            }
            let metadata = again.as_ref().and_then(|again| self.take(again));
            let metadata = match (metadata, header) {
                (Some(metadata), Some(header)) if address(&running) == root => {
                    Some(Rc::new(BodyMetadata {
                        header_pattern: Some(header),
                        ..(*metadata).clone()
                    }))
                }
                (metadata, _) => metadata,
            };
            let running_blocks = running.body().created_blocks();
            match &again {
                // `compiles_alike` held every block to its counterpart.
                Some(again) => pending.extend(
                    running_blocks
                        .zip(again.body().created_blocks())
                        .map(|(block, again)| (Rc::clone(block.code), Some(Rc::clone(again.code)))),
                ),
                None => {
                    pending.extend(running_blocks.map(|block| (Rc::clone(block.code), None)));
                }
            }
            self.pairs.push((running, metadata));
        }
    }

    /// The tables collected for `again`, once per body.
    fn take(&mut self, again: &Rc<CompiledCode>) -> Option<Rc<BodyMetadata>> {
        self.taken
            .entry(address(again))
            .or_insert_with(|| self.collector.take(again).map(Rc::new))
            .clone()
    }
}
