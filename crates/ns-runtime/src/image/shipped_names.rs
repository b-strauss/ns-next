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

use crate::interpreter::shipped_world::ShippedWorld;
use crate::types::ObjectReference;
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
/// that decodes a graph section it wrote by hand.
pub fn resolve_in<'gc>(
    mutator: &Mutator<'gc>,
    names: &[ShippedName],
    started: bool,
) -> Result<Vec<ResolvedName<'gc>>, DecodeError> {
    resolve(names, &mutator.shipped_world, started)
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
    /// the bytes.
    fn section_of(names: &[ShippedName]) -> (symbols::Table, Vec<u8>) {
        let mut interpreter = crate::Interpreter::bootstrap();
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
            name("Kernel", "Kernel", ShippedKind::Level(0)),
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

    /// A name answers the very object the boot recorded under it: a level,
    /// which no value can hold, checked here rather than through a graph.
    #[test]
    fn a_level_resolves_to_the_boots_own_level() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let level = name("Kernel", "Kernel", ShippedKind::Level(0));
            let index = world.index_of_name(&level).expect("Kernel Kernel level 0");
            let resolved = resolve(&[level], world, true).expect("a kernel name resolves");
            assert!(resolved[0].object.same(world.entries()[index].object));
            assert_eq!(resolved[0].positions, world.entries()[index].positions);
            assert_eq!(resolved[0].name, level);
        });
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

    /// `Kernel Kernel` has one `<:` clause, so its level 1 names nothing.
    #[test]
    fn a_level_past_the_clauses_is_refused() {
        assert_eq!(
            resolve_error(&[name("Kernel", "Kernel", ShippedKind::Level(1))], false),
            DecodeError::UnknownShippedName("Kernel Kernel level 1".to_string())
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
