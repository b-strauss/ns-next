//! The module section: which top-level modules an artifact carries, each
//! module's identity -- its path relative to its project, its id and its
//! version (`docs/design/modules.md`, decision 10) -- and which record of
//! the graph is its root class. A module's root class, the levels its `<:`
//! clauses add and their metaclasses are ordinary class records; the
//! section names each root by its object id, and the loader holds the
//! record it names to its module's declaration and to the root rule
//! (`root_record_defect`).

use std::rc::Rc;

use super::decode::{self, DecodeError, Reader};
use super::encode::{self, Writer};
use super::mixins;
use super::opcodes::{MODULE_UNVERSIONED, MODULE_VERSIONED, ORIGIN_CORE, ORIGIN_PROJECT};
use crate::Mutator;
use crate::heap::Heap;
use crate::mixin::Mixin;
use crate::modules::{self as identities, ModuleId, ModuleIdentity, Version};
use crate::symbol::Symbol;
use crate::types::{BuiltinClass, ClassReference};

/// One module an artifact carries, as a writer is handed it. `with_source`
/// asks for the module's text in the image, which the encoder reads from
/// the heap, each declaration's own (`Mixin::text`): after an edit the
/// host's file is no longer the program's text. Source travels for every
/// module or for none. `identity` is what the image names the module by,
/// as the program named it: never a directory of the machine that writes
/// it.
pub struct ModuleSource<'a, 'gc> {
    pub class: ClassReference<'gc>,
    pub identity: &'a ModuleIdentity,
    pub with_source: bool,
}

/// The one module of the binary's an image carries: the manifest, which is
/// compiled into the program beside its modules. Every other module of the
/// binary's is named by the shipped names, never by a module record.
const MANIFEST_PATH: &str = "core:Manifest.ns";
const MANIFEST_ID: &str = "newspeak.core.Manifest";

/// Message of a `core:` record that is not the manifest's; `{name}` and
/// `{path}` are replaced.
const NOT_THE_MANIFEST: &str = "`{name}` at `{path}` is no module an image carries from the binary: only `newspeak.core.Manifest` at `core:Manifest.ns`, without a version";

/// Why `identity` cannot name the module whose root class is named
/// `root_name`, if it cannot: the one check of a single module the encoder
/// and the decoder share, so that the encoder never writes a record the
/// loader refuses. Its path is relative to the project or a `core:` file
/// name, its id ends in the root's name, and the namespace `newspeak.` is
/// the binary's modules' and theirs alone (decisions 1, 2 and 9). A
/// `core:` path is the manifest's, `newspeak.core.Manifest` without a
/// version, and nothing else: every module an image carries is program
/// code, so one named as another of the binary's modules would put a
/// program's module in the reserved namespace, where a tool's name would
/// meet the shipped module's.
pub fn identity_defect(identity: &ModuleIdentity, root_name: &str) -> Result<(), String> {
    identities::check_relative_path(&identity.path)?;
    ModuleId::parse(identity.id.as_str(), root_name)?;
    let is_core = identities::core_file(&identity.path).is_some();
    let is_the_manifest = identity.path == MANIFEST_PATH
        && identity.id.as_str() == MANIFEST_ID
        && identity.version.is_none();
    if is_core && !is_the_manifest {
        return Err(NOT_THE_MANIFEST
            .replace("{name}", &identity.display_name())
            .replace("{path}", &identity.path));
    }
    identities::check_namespace(identity)
}

/// Every module's identity and the object id of its root class's record
/// (`root_ids`, in the same order), in module order -- the order of the
/// mixin table's trees. A module's text is not here: the text table holds
/// its declarations' (`texts`), from which it is assembled.
///
/// A record is an origin tag and its path -- `ORIGIN_PROJECT` and the path
/// relative to the project, or `ORIGIN_CORE` and the file name of a
/// `core:` path -- the id, `MODULE_UNVERSIONED` or `MODULE_VERSIONED` and
/// the version's text, and the root's object id.
pub fn encode_modules(writer: &mut Writer, modules: &[ModuleSource<'_, '_>], root_ids: &[u32]) {
    writer.write_u32(encode::u32_field("module count", modules.len()));
    for (module, root_id) in modules.iter().zip(root_ids) {
        let identity = module.identity;
        match identities::core_file(&identity.path) {
            Some(file) => {
                writer.write_u8(ORIGIN_CORE);
                writer.write_str(file);
            }
            None => {
                writer.write_u8(ORIGIN_PROJECT);
                writer.write_str(&identity.path);
            }
        }
        writer.write_str(identity.id.as_str());
        match &identity.version {
            Some(version) => {
                writer.write_u8(MODULE_VERSIONED);
                writer.write_str(&version.to_string());
            }
            None => writer.write_u8(MODULE_UNVERSIONED),
        }
        writer.write_u32(*root_id);
    }
}

/// The smallest a module record can be: an origin tag, an empty path's
/// length, an empty id's length, a version tag, and the root's object id.
const MIN_MODULE_BYTES: usize = 1 + 4 + 4 + 1 + 4;

/// One module as the module section describes it.
pub struct DecodedModule {
    /// Its path, id and version, held to the project's rules
    /// (`decode_modules`); the loader hands it to the module's root mixin.
    pub identity: Rc<ModuleIdentity>,
    /// The object id of the graph record that is the module's root class,
    /// held to the module by `module_root_classes` once the graph exists.
    pub root: u32,
}

/// The inverse of `encode_modules`. The declared count must be the mixin
/// table's module tree count -- its trees but the obsolete ones its count
/// names -- the length of `root_names`, each module tree's root
/// declaration's name, as `mixins::read` found the trees; a surplus tree
/// would be declarations no module names, a missing one a module without a
/// root -- and is bounded by the bytes left before anything is reserved for
/// it. A root's id is only read here: the graph it indexes comes later
/// (`module_root_classes`).
///
/// Every identity is held to the rules a project's modules are held to
/// before any is used (`docs/design/modules.md`, decision 10): each
/// record's by `decode_record`, and then all of them together -- no two
/// modules of one id and one version, nor of one id where either has none
/// (`modules::check_distinct`), since a request could pick between neither
/// pair and a tool would name both alike.
pub fn decode_modules(
    reader: &mut Reader,
    root_names: &[Symbol],
) -> Result<Vec<DecodedModule>, DecodeError> {
    let declared = reader.read_u32()?;
    let trees = u32::try_from(root_names.len()).unwrap_or(u32::MAX);
    if declared != trees {
        return Err(DecodeError::ModuleCountMismatch { declared, trees });
    }
    let count = decode::bounded_count(declared as usize, MIN_MODULE_BYTES, reader)?;
    let mut modules = Vec::with_capacity(count);
    for (index, root_name) in root_names.iter().enumerate().take(count) {
        let module = u32::try_from(index).unwrap_or(u32::MAX);
        let identity =
            decode_record(reader, root_name.as_str()).map_err(|reason| match reason {
                RecordDefect::Malformed(error) => error,
                RecordDefect::Refused(reason) => DecodeError::ModuleIdentity { module, reason },
            })?;
        let root = reader.read_u32()?;
        modules.push(DecodedModule {
            identity: Rc::new(identity),
            root,
        });
    }
    identities::check_distinct(modules.iter().map(|module| &*module.identity))
        .map_err(DecodeError::IndistinctModules)?;
    Ok(modules)
}

/// Why a module record was not read: bytes that are no record, or a record
/// the project's rules refuse.
enum RecordDefect {
    Malformed(DecodeError),
    Refused(String),
}

impl From<DecodeError> for RecordDefect {
    fn from(error: DecodeError) -> RecordDefect {
        RecordDefect::Malformed(error)
    }
}

/// One module record's identity, up to its root's object id, held to the
/// rules of one module (`identity_defect`) against `root_name`, its root
/// declaration's name. A project's path is held to a project's rules
/// before it is joined into the identity, so that a `core:` path cannot
/// pass as a project's; the origin and version tags are read before
/// anything they introduce.
fn decode_record(reader: &mut Reader, root_name: &str) -> Result<ModuleIdentity, RecordDefect> {
    let path = match reader.read_u8()? {
        ORIGIN_PROJECT => {
            let path = reader.read_text()?;
            identities::check_project_path(path).map_err(RecordDefect::Refused)?;
            path.to_string()
        }
        ORIGIN_CORE => identities::core_path(reader.read_text()?),
        tag => {
            return Err(RecordDefect::Malformed(DecodeError::InvalidTag {
                what: "module origin",
                tag,
            }));
        }
    };
    let id = reader.read_text()?;
    let version = match reader.read_u8()? {
        MODULE_UNVERSIONED => None,
        MODULE_VERSIONED => Some(reader.read_text()?),
        tag => {
            return Err(RecordDefect::Malformed(DecodeError::InvalidTag {
                what: "module version",
                tag,
            }));
        }
    };
    let identity = ModuleIdentity {
        path,
        id: ModuleId::parse(id, root_name).map_err(RecordDefect::Refused)?,
        version: version
            .map(Version::parse)
            .transpose()
            .map_err(RecordDefect::Refused)?,
    };
    identity_defect(&identity, root_name).map_err(RecordDefect::Refused)?;
    Ok(identity)
}

/// What disqualifies a class object as a module's root class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootRecordDefect {
    /// It is no class of the module's own root declaration as the
    /// outermost level: a metaclass, a level, a class of another
    /// declaration, or one with an enclosing object or declaring class.
    NotItsRecord,
    /// Its superclass chain is none the root rule builds
    /// (`resolver::root_clause_defect`).
    RootRule,
}

/// Why `class` cannot be the root class of the module whose root
/// declaration is `root_mixin`, if it cannot: the one check the encoder and
/// the decoder share, so the encoder never writes a root the loader
/// refuses.
///
/// A root class applies its module's root declaration and is the outermost
/// level: no metaclass, no level, neither an enclosing object nor a
/// declaring class. Its superclass is what the root rule builds and nothing
/// else -- `Kernel Object` or `Kernel Value` for a root with no `<:`
/// clause, and for one with its one permitted clause the level that clause
/// added, applying `Kernel Value` over `Kernel Object`. A crafted image
/// cannot bring back a root the resolver refuses to build.
///
/// A root without a body, `class R = Object <: Value.`, is the level its
/// clause added (`object-model.md`, "Mixin application";
/// `body_less_root_record_defect`).
pub fn root_record_defect<'gc>(
    mutator: &Mutator<'gc>,
    root_mixin: &Rc<Mixin>,
    class: ClassReference<'gc>,
) -> Option<RootRecordDefect> {
    if !root_mixin.has_body {
        return body_less_root_record_defect(mutator, root_mixin, class);
    }
    let heap: &Heap<'gc> = &mutator.heap;
    // By id, not by address: an edit of the root replaces it under its id,
    // and the level its clause added still names the version that applied
    // it.
    let root_id = heap.known_mixin_id(root_mixin);
    let applies_the_root = heap
        .class_application(class)
        .is_some_and(|application| heap.same_mixin(&application.mixin, root_mixin, root_id));
    if !heap.is_class(class)
        || heap.class_this_class(class).is_some()
        || !applies_the_root
        || heap.class_applied_origin(class).is_some()
        || heap.class_enclosing(class).is_some()
        || heap.class_declaring(class).is_some()
    {
        return Some(RootRecordDefect::NotItsRecord);
    }
    let object = mutator.builtin_class(BuiltinClass::Object);
    let value = mutator.builtin_class(BuiltinClass::Value);
    let Some(superclass) = heap.class_superclass(class) else {
        return Some(RootRecordDefect::RootRule);
    };
    let obeys = match root_mixin.mixin_applications.len() {
        0 => superclass.same(object) || superclass.same(value),
        1 => {
            let value_mixin = heap
                .class_application(value)
                .map(|application| application.mixin.clone());
            let added_by_the_root = heap
                .class_applied_origin(superclass)
                .is_some_and(|applied| {
                    heap.same_mixin(&applied.declaration, root_mixin, root_id) && applied.index == 0
                });
            let applies_value = heap
                .class_application(superclass)
                .zip(value_mixin)
                .is_some_and(|(application, value_mixin)| {
                    Rc::ptr_eq(&application.mixin, &value_mixin)
                });
            added_by_the_root
                && applies_value
                && heap
                    .class_superclass(superclass)
                    .is_some_and(|below| below.same(object))
        }
        _ => false,
    };
    (!obeys).then_some(RootRecordDefect::RootRule)
}

/// `root_record_defect` for a root declaration without a body: its class
/// is the level its one clause added, `Kernel Value`'s mixin over `Kernel
/// Object`, named by the declaration. The rule of the outermost level
/// holds for the declaration's own scope, the one its own code walks out
/// from: §3.3 makes "the enclosing object of a module definition" nil, so
/// the record has neither a declaration enclosing object nor a declaration
/// declaring class. Its level's own enclosing object and declaring class
/// are `Value`'s (§6.2, "the enclosing object of I is the enclosing object
/// of C"), which `Value`'s methods walk out from, as every level applying
/// a shipped class has them.
fn body_less_root_record_defect<'gc>(
    mutator: &Mutator<'gc>,
    root_mixin: &Rc<Mixin>,
    class: ClassReference<'gc>,
) -> Option<RootRecordDefect> {
    let heap: &Heap<'gc> = &mutator.heap;
    let root_id = heap.known_mixin_id(root_mixin);
    let is_its_class_object = heap.is_class(class)
        && heap.class_this_class(class).is_none()
        && heap
            .class_body_less_declaration(class)
            .is_some_and(|declaration| heap.same_mixin(declaration, root_mixin, root_id))
        && heap
            .class_declaration_scope(class)
            .is_some_and(|scope| scope.enclosing.is_none() && scope.declaring_class.is_none());
    if !is_its_class_object {
        return Some(RootRecordDefect::NotItsRecord);
    }
    let object = mutator.builtin_class(BuiltinClass::Object);
    let value = mutator.builtin_class(BuiltinClass::Value);
    let value_mixin = heap
        .class_application(value)
        .map(|application| application.mixin.clone());
    let applies_value = heap
        .class_application(class)
        .zip(value_mixin)
        .is_some_and(|(application, value_mixin)| Rc::ptr_eq(&application.mixin, &value_mixin));
    let obeys = root_mixin.mixin_applications.len() == 1
        && applies_value
        && heap
            .class_named(class)
            .is_some_and(|named| named.same(value))
        && heap
            .class_superclass(class)
            .is_some_and(|below| below.same(object));
    (!obeys).then_some(RootRecordDefect::RootRule)
}

/// Each module's root class: the record its id names in the decoded graph
/// (`objects`), held to the module's root declaration and the root rule
/// (`root_record_defect`). An id past the graph is `BadObjectId`, a record
/// that is no class of the module's declaration as its outermost level
/// `ModuleRootNotItsRecord`, a superclass chain the root rule does not
/// build `RootRule`.
pub fn module_root_classes<'gc>(
    mutator: &Mutator<'gc>,
    mixins: &mixins::Decoded,
    modules: &[DecodedModule],
    objects: &[crate::types::ObjectReference<'gc>],
) -> Result<Vec<ClassReference<'gc>>, DecodeError> {
    let mut roots = Vec::with_capacity(modules.len());
    for (index, module) in modules.iter().enumerate() {
        let module_index = u32::try_from(index).unwrap_or(u32::MAX);
        let class = objects
            .get(module.root as usize)
            .copied()
            .ok_or(DecodeError::BadObjectId(module.root))?;
        // `decode_modules` held the count to the trees, so every module has
        // its root row.
        let root_mixin = mixins.module_root(index).ok_or(DecodeError::NotAClass)?;
        match root_record_defect(mutator, root_mixin, class) {
            None => roots.push(class),
            Some(RootRecordDefect::NotItsRecord) => {
                return Err(DecodeError::ModuleRootNotItsRecord {
                    module: module_index,
                });
            }
            Some(RootRecordDefect::RootRule) => {
                return Err(DecodeError::RootRule {
                    module: module_index,
                });
            }
        }
    }
    Ok(roots)
}
