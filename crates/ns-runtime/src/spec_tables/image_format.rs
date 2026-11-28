//! The specification's image-format chapter (`docs/spec/image-format.md`)
//! held to the encoder: its tag table to `image/opcodes.rs`, the header
//! flags of `image/encode.rs`, the shipped-name kinds of
//! `image/shipped_names.rs` and the accessibilities of `image/mixins.rs`.

use std::collections::BTreeSet;

use ns_syntax::ast::AccessModifier;

use super::image_table;
use crate::image::encode::{FLAG_HAS_SOURCE, FLAG_START, RETIRED_HEADER_FLAGS};
use crate::image::opcodes::*;
use crate::image::shipped_names::{
    KIND_CLASS, KIND_LEVEL, KIND_LEVEL_METACLASS, KIND_METACLASS, KIND_MODULE_INSTANCE,
};

/// The declarations of the graph section's tags, read for the names of
/// their constants.
const OPCODES: &str = include_str!("../image/opcodes.rs");

/// The tag spaces of the chapter's table, each with the prefix its
/// constants' names begin with. A longer prefix comes before a shorter one
/// it begins with, so that a constant is given the space of its longest.
const SPACES: [(&str, &str); 13] = [
    ("header-flag", "FLAG_"),
    ("value", "VALUE_"),
    ("object-body", "OBJECT_"),
    ("mixin-side", "MIXIN_SIDE_"),
    ("mixin-reference", "MIXIN_"),
    ("class-reference", "CLASS_"),
    ("frame-body", "FRAME_BODY_"),
    ("frame-kind", "FRAME_"),
    ("hashed", "HASHED_"),
    ("slot-kind", "SLOT_"),
    ("origin", "ORIGIN_"),
    ("module-version", "MODULE_"),
    ("shipped-kind", "KIND_"),
];

/// The tag space whose numbers no constant names: a member's
/// accessibility, which `mixins::access_tag` writes.
const ACCESS_SPACE: &str = "access";

/// Every accessibility, with the name the table gives it. The match has no
/// `_` arm, so that a new accessibility must be named here.
fn access_name(access: AccessModifier) -> &'static str {
    match access {
        AccessModifier::Public => "public",
        AccessModifier::Protected => "protected",
        AccessModifier::Private => "private",
    }
}

/// The prefixes of `opcodes.rs`'s tags that the bytecode chapter's table
/// gives, not this one.
const BYTECODE_PREFIXES: [&str; 2] = ["OPCODE_", "LITERAL_"];

/// The space whose prefix `constant` begins with, and the name the table
/// gives it there: the rest of the constant's name, in kebab case.
fn space_and_name(constant: &str) -> (&'static str, String) {
    let (space, prefix) = SPACES
        .iter()
        .find(|(_, prefix)| constant.starts_with(prefix))
        .unwrap_or_else(|| panic!("`{constant}` begins with no tag space's prefix"));
    let name = constant[prefix.len()..].to_lowercase().replace('_', "-");
    (space, name)
}

/// Each constant's name beside the named row the table gives it, from the
/// constant's name and value.
macro_rules! named {
    ($($constant:ident),* $(,)?) => {
        [$({
            let (space, name) = space_and_name(stringify!($constant));
            (stringify!($constant), space.to_string(), name, $constant)
        }),*]
    };
}

/// The names of the tag constants `opcodes.rs` declares for this chapter:
/// every `pub const` of type `u8` but the bytecode chapter's, which a new
/// tag joins without a change here, so that the book cannot leave it out.
fn declared_tag_constants() -> BTreeSet<String> {
    OPCODES
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub const "))
        .filter_map(|declaration| declaration.split_once(": u8 ="))
        .map(|(name, _)| name.trim().to_string())
        .filter(|name| {
            !BYTECODE_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .collect()
}

// spec: image.tags
#[test]
fn the_books_image_tags_are_the_encoders() {
    let spaces: BTreeSet<String> = SPACES
        .iter()
        .map(|(space, _)| *space)
        .chain([ACCESS_SPACE])
        .map(str::to_string)
        .collect();
    let mut named = BTreeSet::new();
    let mut retired = BTreeSet::new();
    for row in image_table("image-tags") {
        assert_eq!(row.len(), 3, "a row of three cells: {row:?}");
        assert!(spaces.contains(&row[0]), "no tag space `{}`", row[0]);
        let number: u8 = row[2].parse().expect("a tag is a number");
        if row[1] == "retired" {
            retired.insert((row[0].clone(), number));
        } else {
            named.insert((row[0].clone(), row[1].clone(), number));
        }
    }

    let graph_constants = named!(
        VALUE_NIL,
        VALUE_FALSE,
        VALUE_TRUE,
        VALUE_SMALL_INTEGER,
        VALUE_FLOAT,
        VALUE_OBJECT_ID,
        VALUE_CLASS_TAG,
        VALUE_LARGE_INTEGER,
        VALUE_FRACTION,
        VALUE_SHIPPED_MODULE,
        OBJECT_INSTANCE,
        OBJECT_STRING,
        OBJECT_SYMBOL,
        OBJECT_ARRAY,
        OBJECT_CLOSURE,
        OBJECT_CLASS,
        OBJECT_METACLASS,
        OBJECT_FRAME,
        OBJECT_BYTE_ARRAY,
        OBJECT_WEAK_ARRAY,
        OBJECT_EPHEMERON,
        OBJECT_MIXIN,
        OBJECT_BODY_LESS_CLASS,
        OBJECT_INSTANCE_UNDER_CONSTRUCTION,
        MIXIN_ROW,
        MIXIN_SHIPPED,
        MIXIN_SIDE_INSTANCE,
        MIXIN_SIDE_CLASS,
        CLASS_RECORD,
        CLASS_SHIPPED,
        FRAME_METHOD,
        FRAME_CLOSURE,
        FRAME_TOP_LEVEL,
        FRAME_INITIALIZER,
        FRAME_SUPERCLASS_CLAUSE,
        FRAME_OWN_INITIALIZER,
        FRAME_STANDALONE_INITIALIZER,
        FRAME_FACTORY,
        FRAME_BODY_NONE,
        FRAME_BODY_CODE,
        FRAME_BODY_DECLARED,
        HASHED_SHIPPED,
        SLOT_IMMUTABLE,
        SLOT_MUTABLE,
        SLOT_LAZY_IMMUTABLE,
        SLOT_LAZY_MUTABLE,
        ORIGIN_PROJECT,
        ORIGIN_CORE,
        ORIGIN_NONE,
        MODULE_UNVERSIONED,
        MODULE_VERSIONED,
    );
    let listed: BTreeSet<String> = graph_constants
        .iter()
        .map(|(constant, ..)| constant.to_string())
        .collect();
    assert_eq!(
        listed,
        declared_tag_constants(),
        "every tag `opcodes.rs` declares for the graph and module sections is compared"
    );

    let expected: BTreeSet<_> = graph_constants
        .into_iter()
        .chain(named!(FLAG_HAS_SOURCE, FLAG_START))
        .chain(named!(
            KIND_MODULE_INSTANCE,
            KIND_CLASS,
            KIND_METACLASS,
            KIND_LEVEL,
            KIND_LEVEL_METACLASS,
        ))
        .map(|(_, space, name, number)| (space, name, number))
        .chain(
            [
                AccessModifier::Public,
                AccessModifier::Protected,
                AccessModifier::Private,
            ]
            .into_iter()
            .map(|access| {
                (
                    ACCESS_SPACE.to_string(),
                    access_name(access).to_string(),
                    crate::image::mixins::access_tag(access),
                )
            }),
        )
        .collect();
    assert_eq!(named, expected, "the named tags");

    let retired_in = |space: &str, numbers: &[u8]| {
        numbers
            .iter()
            .map(|number| (space.to_string(), *number))
            .collect::<Vec<_>>()
    };
    let retired_flags: Vec<u8> = (0..u8::BITS)
        .map(|bit| 1u8 << bit)
        .filter(|flag| RETIRED_HEADER_FLAGS & flag != 0)
        .collect();
    let retired_expected: BTreeSet<_> = [
        retired_in("header-flag", &retired_flags),
        retired_in("value", &RETIRED_VALUE_TAGS),
        retired_in("object-body", &RETIRED_OBJECT_BODIES),
        retired_in("mixin-reference", &RETIRED_MIXIN_TAGS),
        retired_in("class-reference", &RETIRED_CLASS_TAGS),
        retired_in("frame-kind", &RETIRED_FRAME_KINDS),
        retired_in("hashed", &RETIRED_HASHED_TAGS),
        retired_in("origin", &RETIRED_ORIGIN_TAGS),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(retired, retired_expected, "the retired tags");
    for (space, name, number) in &named {
        assert!(
            !retired.contains(&(space.clone(), *number)),
            "{space} {name} takes the retired number {number}"
        );
    }
}
