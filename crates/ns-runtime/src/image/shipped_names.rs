//! The shipped-name section: how an image names an object the loading
//! binary owns -- a shipped module's instance, a class its root declares,
//! that class's metaclass, a level its `<:` clauses add and that level's
//! metaclass -- by where it is declared (`docs/design/image-format.md`,
//! "Shipped names").
//!
//! The encoder lists every name its graph uses once, in the order it first
//! used each; a reference in the graph is a tag and a `u32` index into that
//! list. The loader resolves the whole section against its own shipped world
//! by table lookup before the graph is decoded, so it sends nothing and runs
//! nothing the image chooses, and every name answers the very object the
//! loading boot built.

use std::collections::HashSet;
use std::rc::Rc;

use crate::heap::Root;
use crate::interpreter::core_library::{booted, LAZY_SHIPPED_MODULES, SHIPPED_MODULES};
use crate::interpreter::shipped_world::ShippedWorld;
use crate::mixin::Mixin;
use crate::types::{ObjectReference, Value};
use crate::{Mutator, Symbol};

pub use crate::interpreter::shipped_world::{Positions, ShippedKind, ShippedName};

use super::decode::{bounded_count, DecodeError, Reader};
use super::encode::{u32_field, Writer};
use super::symbols;

/// The most names one section may list. The shipped world has a few
/// hundred; the count is bounded before anything is reserved for it, as
/// every count an image carries is.
pub const MAX_SHIPPED_NAMES: usize = 65_536;

/// The smallest entry of the section: the module's symbol (4), the
/// declaration's symbol (4) and the kind (1). A level's carries its clause
/// index (4) more.
pub const MIN_SHIPPED_NAME_BYTES: usize = 9;

/// A section entry's kind byte.
const KIND_MODULE_INSTANCE: u8 = 0;
const KIND_CLASS: u8 = 1;
const KIND_METACLASS: u8 = 2;
const KIND_LEVEL: u8 = 3;
const KIND_LEVEL_METACLASS: u8 = 4;

/// Where a reference in the graph stands, which decides what it may name
/// (`Positions`; the rule of `docs/design/image-format.md`, "Shipped
/// names").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamePosition {
    /// A slot's, an element's, a root's or a copied value's, or a class as
    /// any value is (`VALUE_CLASS_TAG`).
    Value,
    /// A class record's superclass, or the class whose mixin a level applies
    /// (`MIXIN_SHIPPED`).
    Superclass,
    /// A record's class.
    Header,
    /// A frame's `current_class`, the class its kind names, or a class
    /// record's declaring class.
    FrameClass,
    /// A frame record's receiver or register (`VALUE_SHIPPED_MODULE`).
    FrameRegister,
}

impl NamePosition {
    /// Whether an object that may stand in `positions` may stand here.
    pub fn allows(self, positions: Positions) -> bool {
        match self {
            NamePosition::Value => positions.value,
            NamePosition::Superclass => positions.superclass,
            NamePosition::Header => positions.header,
            NamePosition::FrameClass => positions.frame_class,
            NamePosition::FrameRegister => positions.frame_register,
        }
    }

    /// The position as a refusal names it.
    pub fn describe(self) -> &'static str {
        match self {
            NamePosition::Value => "a value",
            NamePosition::Superclass => "a superclass or an applied mixin",
            NamePosition::Header => "a record's class",
            NamePosition::FrameClass => "a frame's class",
            NamePosition::FrameRegister => "a frame's register",
        }
    }
}

/// One entry of an image's section, resolved against the loading world: the
/// object it names, where that object may stand, and the name itself, for a
/// refusal to show.
pub struct ResolvedName<'gc> {
    pub object: ObjectReference<'gc>,
    pub positions: Positions,
    pub name: ShippedName,
}

/// The section: a `u32` count, then per entry the module's and the
/// declaration's symbol ids, the kind byte and, for a level or a level's
/// metaclass, the clause's `u32` index. `used` lists positions in
/// `world.entries()`, in the order the graph first used each, which is the
/// order its references number them.
pub(crate) fn encode_section(
    writer: &mut Writer,
    world: &ShippedWorld<'_>,
    used: &[usize],
    symbols: &mut symbols::Builder,
) {
    assert!(
        used.len() <= MAX_SHIPPED_NAMES,
        "internal: an image names more shipped objects than the shipped world has"
    );
    writer.write_u32(u32_field("shipped name count", used.len()));
    for index in used {
        let name = world.entries()[*index].name;
        writer.write_u32(symbols.intern(name.module.as_str()));
        writer.write_u32(symbols.intern(name.declaration.as_str()));
        match name.kind {
            ShippedKind::ModuleInstance => writer.write_u8(KIND_MODULE_INSTANCE),
            ShippedKind::Class => writer.write_u8(KIND_CLASS),
            ShippedKind::Metaclass => writer.write_u8(KIND_METACLASS),
            ShippedKind::Level(clause) => {
                writer.write_u8(KIND_LEVEL);
                writer.write_u32(clause);
            }
            ShippedKind::LevelMetaclass(clause) => {
                writer.write_u8(KIND_LEVEL_METACLASS);
                writer.write_u32(clause);
            }
        }
    }
}

/// The inverse of `encode_section`, its structure only: no world is asked,
/// so `inspect` reads it too. Refuses a count past the bytes left or past
/// `MAX_SHIPPED_NAMES`, a symbol id past the symbol table, a kind byte no
/// kind has, and a clause index no class object can number.
pub(crate) fn decode_section(
    reader: &mut Reader,
    symbols: &symbols::Table,
) -> Result<Vec<ShippedName>, DecodeError> {
    let declared = reader.read_u32()? as usize;
    if declared > MAX_SHIPPED_NAMES {
        return Err(DecodeError::TooManyShippedNames { count: declared });
    }
    let count = bounded_count(declared, MIN_SHIPPED_NAME_BYTES, reader)?;
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        let module = symbols.get(reader.read_u32()?)?;
        let declaration = symbols.get(reader.read_u32()?)?;
        let kind = match reader.read_u8()? {
            KIND_MODULE_INSTANCE => ShippedKind::ModuleInstance,
            KIND_CLASS => ShippedKind::Class,
            KIND_METACLASS => ShippedKind::Metaclass,
            KIND_LEVEL => ShippedKind::Level(read_clause(reader, module, declaration, false)?),
            KIND_LEVEL_METACLASS => {
                ShippedKind::LevelMetaclass(read_clause(reader, module, declaration, true)?)
            }
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "shipped name kind",
                    tag,
                })
            }
        };
        names.push(ShippedName {
            module,
            declaration,
            kind,
        });
    }
    Ok(names)
}

/// A level's clause index, refused at `CLAUSE_INDEX_LIMIT` or beyond: no
/// class object can number such a clause, so no shipped object has that
/// name.
fn read_clause(
    reader: &mut Reader,
    module: Symbol,
    declaration: Symbol,
    metaclass: bool,
) -> Result<u32, DecodeError> {
    let clause = reader.read_u32()?;
    if clause as usize >= crate::mixin::CLAUSE_INDEX_LIMIT {
        let kind = if metaclass {
            ShippedKind::LevelMetaclass(clause)
        } else {
            ShippedKind::Level(clause)
        };
        let name = ShippedName {
            module,
            declaration,
            kind,
        };
        return Err(DecodeError::UnknownShippedName(name.describe()));
    }
    Ok(clause)
}

/// Every name of a section, in section order, resolved against `world` by
/// table lookup alone: a name the world does not have, and a name listed
/// twice, are refused, and so, in a started image (`started`), is a name of
/// any module but `Kernel` and any module's instance, the kernel's
/// included, since before `main:args:` a program reaches nothing of the
/// other modules and no program reaches a module's instance -- a frame the
/// started image holds, through a block in a slot, would hand one over.
pub(crate) fn resolve<'gc>(
    names: &[ShippedName],
    world: &ShippedWorld<'gc>,
    started: bool,
) -> Result<Vec<ResolvedName<'gc>>, DecodeError> {
    let kernel = Symbol::intern("Kernel");
    let mut seen: HashSet<usize> = HashSet::with_capacity(names.len());
    let mut resolved = Vec::with_capacity(names.len());
    for name in names {
        let index = world
            .index_of_name(name)
            .ok_or_else(|| DecodeError::UnknownShippedName(name.describe()))?;
        if !seen.insert(index) {
            return Err(DecodeError::DuplicateShippedName(name.describe()));
        }
        if started && (name.module != kernel || name.kind == ShippedKind::ModuleInstance) {
            return Err(DecodeError::ShippedNameInStartedImage(name.describe()));
        }
        let entry = &world.entries()[index];
        resolved.push(ResolvedName {
            object: entry.object,
            positions: entry.positions,
            name: *name,
        });
    }
    Ok(resolved)
}

/// `resolve` against `mutator`'s own shipped world, for a host or a test
/// that decodes a graph section it wrote by hand. The world holds what has
/// been built so far: a name of a module or class not yet built is refused,
/// so a host resolving names a fresh boot may not have built builds them
/// first (`Interpreter::build_shipped_names`), as `image::load` does.
pub fn resolve_in<'gc>(
    mutator: &Mutator<'gc>,
    names: &[ShippedName],
    started: bool,
) -> Result<Vec<ResolvedName<'gc>>, DecodeError> {
    resolve(names, &mutator.shipped_world, started)
}

impl crate::Interpreter {
    /// Builds the shipped modules and classes `names` names that this
    /// interpreter has not built yet, so that `resolve` finds each in its
    /// shipped world: every lazy module named and not recorded, in
    /// `SHIPPED_MODULES` order, then every class of such a module named and
    /// not yet named, in the order of `names`. Each is built at most once.
    ///
    /// First, before anything is built, every name is checked, and each
    /// refusal `resolve` would give is given here: a module that is no
    /// shipped module, a declaration its module's root does not declare, or
    /// a level past its declaration's `<:` clauses is `UnknownShippedName`,
    /// a name given twice `DuplicateShippedName`, and in a started image
    /// (`started`) a name of any module but the kernel, or of a module's
    /// instance, is `ShippedNameInStartedImage` -- so a started image builds
    /// nothing. The started rule is applied as soon as the module is known,
    /// before its root is looked at, so a started image naming an unknown
    /// declaration of another module is refused as started.
    /// What is built is the binary's own code, the same build a program's
    /// first demand runs: the names choose which of the binary's modules
    /// and classes exist, never what runs.
    #[doc(hidden)]
    pub fn build_shipped_names(
        &mut self,
        names: &[ShippedName],
        started: bool,
    ) -> Result<(), DecodeError> {
        let root_mixins = self.enter(|mutator| check_names(mutator, names, started))?;
        for (shipped, root_mixin) in SHIPPED_MODULES.iter().zip(root_mixins) {
            let Some(root_mixin) = root_mixin else {
                continue;
            };
            let module = Symbol::intern(shipped.name);
            let recorded =
                self.enter(|mutator| mutator.shipped_world.module_instance(module).is_some());
            if LAZY_SHIPPED_MODULES.contains(&shipped.name) && !recorded {
                self.build_shipped_module(shipped.name, Some(root_mixin));
            }
        }
        for name in names {
            if name.declaration == name.module
                || !LAZY_SHIPPED_MODULES.contains(&name.module.as_str())
            {
                continue;
            }
            let Some((instance, nested)) = self.enter(|mutator| unbuilt_class(mutator, name))
            else {
                continue;
            };
            let class = booted(
                self.build_nested_class(&instance, &nested),
                &format!("building {}", name.describe()),
            );
            self.enter(|mutator| {
                let class = mutator
                    .fetch(&class)
                    .as_object()
                    .unwrap_or_else(|| panic!("internal: {} is a class object", name.describe()));
                mutator.record_shipped_class(name.module, &nested, class);
            });
        }
        Ok(())
    }
}

/// `build_shipped_names`' check of every name, which builds nothing.
/// Answers, per `SHIPPED_MODULES` entry, the root mixin of the module when
/// a name names it: the recorded instance's or the pending class's, else the
/// one taken from the shipped code, which the build then uses. One slot per
/// module however many names the image lists; only the set that finds a
/// name given twice grows with the section, as `resolve`'s does.
fn check_names(
    mutator: &mut Mutator<'_>,
    names: &[ShippedName],
    started: bool,
) -> Result<Vec<Option<Rc<Mixin>>>, DecodeError> {
    let kernel = Symbol::intern("Kernel");
    let mut kernel_mixin: Option<Rc<Mixin>> = None;
    let mut root_mixins: Vec<Option<Rc<Mixin>>> = vec![None; SHIPPED_MODULES.len()];
    // Bounded by the section's count, which the decoder bounded.
    let mut seen: HashSet<ShippedName> = HashSet::with_capacity(names.len());
    for name in names {
        let unknown = || DecodeError::UnknownShippedName(name.describe());
        let slot = if name.module == kernel {
            &mut kernel_mixin
        } else {
            let position = SHIPPED_MODULES
                .iter()
                .position(|shipped| shipped.name == name.module.as_str())
                .ok_or_else(unknown)?;
            &mut root_mixins[position]
        };
        // Before the root mixin is taken, so that a started image does not
        // even compile a lazy module's root.
        if started && (name.module != kernel || name.kind == ShippedKind::ModuleInstance) {
            return Err(DecodeError::ShippedNameInStartedImage(name.describe()));
        }
        let root = match slot {
            Some(root) => Rc::clone(root),
            None => Rc::clone(slot.insert(root_mixin_of(mutator, name.module))),
        };
        let declared = if name.declaration == name.module {
            Some(root)
        } else if name.kind == ShippedKind::ModuleInstance {
            None
        } else {
            root.nested
                .iter()
                .find(|nested| nested.name == name.declaration)
                .cloned()
        };
        let Some(declared) = declared else {
            return Err(unknown());
        };
        // A level is named by the index of the `<:` clause that adds it.
        if let ShippedKind::Level(clause) | ShippedKind::LevelMetaclass(clause) = name.kind {
            if clause as usize >= declared.mixin_applications.len() {
                return Err(unknown());
            }
        }
        if !seen.insert(*name) {
            return Err(DecodeError::DuplicateShippedName(name.describe()));
        }
    }
    Ok(root_mixins)
}

/// The root mixin of the shipped module `module`, which `check_names` has
/// found in the binary's table: its recorded instance's, its pending class's,
/// or else taken from the shipped code, which builds no object.
fn root_mixin_of(mutator: &mut Mutator<'_>, module: Symbol) -> Rc<Mixin> {
    let heap = mutator.heap();
    let class = match mutator.shipped_world.module_instance(module) {
        Some(instance) => Some(heap.object_class(instance)),
        None => mutator.shipped_world.pending_class(module),
    };
    match class {
        Some(class) => Rc::clone(
            &heap
                .class_application(class)
                .expect("internal: a shipped module's root class is a class object")
                .mixin,
        ),
        None => mutator.shipped_root_mixin(module.as_str()),
    }
}

/// The recorded instance of `name`'s module, rooted, and the declaration
/// `name` names in its root, when that class is not named yet; `None` when
/// it is, which `build_shipped_names` then leaves as it is.
fn unbuilt_class(mutator: &mut Mutator<'_>, name: &ShippedName) -> Option<(Root, Rc<Mixin>)> {
    let class_name = ShippedName {
        kind: ShippedKind::Class,
        ..*name
    };
    if mutator.shipped_world.index_of_name(&class_name).is_some() {
        return None;
    }
    let instance = mutator
        .shipped_world
        .module_instance(name.module)
        .unwrap_or_else(|| {
            panic!(
                "internal: {} is built before its classes",
                name.module.as_str()
            )
        });
    let heap = mutator.heap();
    let root = heap
        .class_application(heap.object_class(instance))
        .expect("internal: a shipped module's root class is a class object");
    let nested = root
        .mixin
        .nested
        .iter()
        .find(|nested| nested.name == name.declaration)
        .cloned()
        .unwrap_or_else(|| panic!("internal: {} was checked to be declared", name.describe()));
    Some((mutator.stash(Value::object(instance)), nested))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(module: &str, declaration: &str, kind: ShippedKind) -> ShippedName {
        ShippedName {
            module: Symbol::intern(module),
            declaration: Symbol::intern(declaration),
            kind,
        }
    }

    /// `names` as a section, against a fresh symbol table: the table and
    /// the bytes. The names are built first, as a program's demand would
    /// build them, since a boot builds only the kernel and the platform.
    fn section_of(names: &[ShippedName]) -> (symbols::Table, Vec<u8>) {
        let mut interpreter = crate::Interpreter::bootstrap();
        if let Err(error) = interpreter.build_shipped_names(names, false) {
            panic!("the names build: {error}");
        }
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let used: Vec<usize> = names
                .iter()
                .map(|shipped| world.index_of_name(shipped).expect("a shipped name"))
                .collect();
            let mut symbols = symbols::Builder::new();
            let mut writer = Writer::new();
            encode_section(&mut writer, world, &used, &mut symbols);
            (symbols.build(), writer.into_bytes())
        })
    }

    /// Hand-written section bytes over a table holding `Kernel` (0),
    /// `Array` (1) and `Nothing` (2).
    fn crafted(entries: impl FnOnce(&mut Writer)) -> (symbols::Table, Vec<u8>) {
        let mut table = symbols::Builder::new();
        for text in ["Kernel", "Array", "Nothing"] {
            table.intern(text);
        }
        let mut writer = Writer::new();
        entries(&mut writer);
        (table.build(), writer.into_bytes())
    }

    fn decode_error(table: &symbols::Table, bytes: &[u8]) -> DecodeError {
        match decode_section(&mut Reader::new(bytes), table) {
            Ok(_) => panic!("expected the section to be refused"),
            Err(error) => error,
        }
    }

    fn resolve_error(names: &[ShippedName], started: bool) -> DecodeError {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(
            |mutator| match resolve(names, &mutator.shipped_world, started) {
                Ok(_) => panic!("expected the names to be refused"),
                Err(error) => error,
            },
        )
    }

    /// Three names of three kinds survive the section, in order.
    #[test]
    fn three_names_round_trip() {
        let names = [
            name("Kernel", "Array", ShippedKind::Metaclass),
            name("Kernel", "Integer", ShippedKind::Class),
            name(
                "CollectionsFactory",
                "CollectionsFactory",
                ShippedKind::ModuleInstance,
            ),
        ];
        let (table, bytes) = section_of(&names);
        let mut reader = Reader::new(&bytes);
        let decoded = decode_section(&mut reader, &table).expect("the section decodes");
        assert_eq!(decoded, names);
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn a_count_past_the_maximum_is_refused() {
        let (table, bytes) = crafted(|writer| {
            writer.write_u32(u32_field("count", MAX_SHIPPED_NAMES + 1));
        });
        assert_eq!(
            decode_error(&table, &bytes),
            DecodeError::TooManyShippedNames {
                count: MAX_SHIPPED_NAMES + 1
            }
        );
    }

    #[test]
    fn a_count_past_the_bytes_left_is_refused() {
        let (table, bytes) = crafted(|writer| {
            writer.write_u32(2);
            writer.write_u32(0);
            writer.write_u32(1);
            writer.write_u8(KIND_CLASS);
        });
        assert_eq!(
            decode_error(&table, &bytes),
            DecodeError::UnexpectedEndOfFile
        );
    }

    #[test]
    fn a_symbol_past_the_table_is_refused() {
        let (table, bytes) = crafted(|writer| {
            writer.write_u32(1);
            writer.write_u32(0);
            writer.write_u32(7);
            writer.write_u8(KIND_CLASS);
        });
        assert_eq!(decode_error(&table, &bytes), DecodeError::BadSymbolId(7));
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        let (table, bytes) = crafted(|writer| {
            writer.write_u32(1);
            writer.write_u32(0);
            writer.write_u32(1);
            writer.write_u8(5);
        });
        assert_eq!(
            decode_error(&table, &bytes),
            DecodeError::InvalidTag {
                what: "shipped name kind",
                tag: 5
            }
        );
    }

    #[test]
    fn a_clause_index_no_class_can_number_is_refused() {
        let limit = u32_field("clause", crate::mixin::CLAUSE_INDEX_LIMIT);
        let (table, bytes) = crafted(|writer| {
            writer.write_u32(1);
            writer.write_u32(0);
            writer.write_u32(0);
            writer.write_u8(KIND_LEVEL);
            writer.write_u32(limit);
        });
        assert_eq!(
            decode_error(&table, &bytes),
            DecodeError::UnknownShippedName(format!("Kernel Kernel level {limit}"))
        );
    }

    #[test]
    fn an_unknown_module_or_declaration_is_refused() {
        assert_eq!(
            resolve_error(&[name("Nothing", "Array", ShippedKind::Class)], false),
            DecodeError::UnknownShippedName("Nothing Array".to_string())
        );
        assert_eq!(
            resolve_error(&[name("Kernel", "Nothing", ShippedKind::Class)], false),
            DecodeError::UnknownShippedName("Kernel Nothing".to_string())
        );
    }

    /// A level past a declaration's `<:` clauses names nothing: `Array`
    /// applies one mixin, so its level 0 is named
    /// (`shipped_world`'s
    /// `a_shipped_level_is_named_and_resolves_to_the_boots_own_object`)
    /// and its level 1 is not, nor any level of the kernel root, which is
    /// `= Value` and applies none.
    #[test]
    fn a_level_past_the_clauses_is_refused() {
        assert_eq!(
            resolve_error(&[name("Kernel", "Array", ShippedKind::Level(1))], false),
            DecodeError::UnknownShippedName("Kernel Array level 1".to_string())
        );
        assert_eq!(
            resolve_error(
                &[name("Kernel", "Array", ShippedKind::LevelMetaclass(1))],
                false
            ),
            DecodeError::UnknownShippedName("Kernel Array level 1 class".to_string())
        );
        assert_eq!(
            resolve_error(&[name("Kernel", "Kernel", ShippedKind::Level(0))], false),
            DecodeError::UnknownShippedName("Kernel Kernel level 0".to_string())
        );
    }

    #[test]
    fn a_name_given_twice_is_refused() {
        let array = name("Kernel", "Array", ShippedKind::Class);
        assert_eq!(
            resolve_error(&[array, array], false),
            DecodeError::DuplicateShippedName("Kernel Array".to_string())
        );
    }

    #[test]
    fn a_started_image_names_only_the_kernel() {
        assert_eq!(
            resolve_error(
                &[name("PlatformFactory", "Platform", ShippedKind::Class)],
                true
            ),
            DecodeError::ShippedNameInStartedImage("PlatformFactory Platform".to_string())
        );
    }

    /// Not even the kernel's module instance: no program reaches a shipped
    /// root's instance, and a started image's frames would hand it over.
    #[test]
    fn a_started_image_names_no_module_instance() {
        assert_eq!(
            resolve_error(
                &[name("Kernel", "Kernel", ShippedKind::ModuleInstance)],
                true
            ),
            DecodeError::ShippedNameInStartedImage("Kernel instance".to_string())
        );
    }
}
