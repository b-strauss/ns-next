//! Holds the project file's schema (`schemas/newspeak.schema.json`) and
//! `ns-next`'s reader to one another: every file of the table is valid
//! under both or under neither. The schema is interpreted here with exactly
//! the keywords it uses, and a keyword this interpreter does not know fails
//! the test rather than being skipped, so that the schema cannot gain a rule
//! that nothing checks the reader against.
//!
//! Some duplicates are refused by the reader alone, because JSON Schema
//! cannot state them: `$schema`, `main`, `modules` or `scripts` written
//! twice, and a script name written twice (a schema sees only the last
//! value of a key, as `serde_json` parses it); one module path written in
//! two spellings, such as `lib/A.ns` and `lib\A.ns` (`uniqueItems` compares
//! strings as written); and a `modules` entry naming `main`'s module. None is
//! in the table; `project.rs`'s own tests pin them.

mod common;

use common::ScratchDirectory;
use serde_json::Value;
use std::process::Command;

fn schema() -> Value {
    let path = common::goldens_root().join("../schemas/newspeak.schema.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// Whether `instance` is valid under `schema`, for the keywords the
/// project file's schema uses.
fn valid(schema: &Value, instance: &Value) -> bool {
    let Value::Object(keywords) = schema else {
        panic!("a schema is an object: {schema}")
    };
    keywords
        .iter()
        .all(|(keyword, argument)| match keyword.as_str() {
            "$schema" | "title" | "description" => true,
            "type" => match argument.as_str() {
                Some("object") => instance.is_object(),
                Some("array") => instance.is_array(),
                Some("string") => instance.is_string(),
                other => panic!("the interpreter knows no type {other:?}"),
            },
            "properties" => {
                let (Some(properties), Some(members)) =
                    (argument.as_object(), instance.as_object())
                else {
                    return true;
                };
                members.iter().all(|(name, member)| {
                    properties
                        .get(name)
                        .is_none_or(|property| valid(property, member))
                })
            }
            "required" => {
                let Some(members) = instance.as_object() else {
                    return true;
                };
                argument
                    .as_array()
                    .expect("`required` is an array")
                    .iter()
                    .all(|name| members.contains_key(name.as_str().expect("a required name")))
            }
            "items" => instance
                .as_array()
                .is_none_or(|elements| elements.iter().all(|element| valid(argument, element))),
            "uniqueItems" => {
                assert_eq!(argument, &Value::Bool(true), "only `true` is interpreted");
                instance.as_array().is_none_or(|elements| {
                    elements
                        .iter()
                        .enumerate()
                        .all(|(index, element)| !elements[..index].contains(element))
                })
            }
            "minLength" => instance.as_str().is_none_or(|text| {
                text.chars().count() as u64 >= argument.as_u64().expect("a length")
            }),
            "pattern" => instance
                .as_str()
                .is_none_or(|text| match argument.as_str() {
                    // A literal suffix.
                    Some(r"\.ns$") => text.ends_with(".ns"),
                    // A non-empty text whose first and last characters are no
                    // whitespace; `[\s\S]` is any character, line terminators
                    // included.
                    Some(r"^\S([\s\S]*\S)?$") => {
                        text.chars()
                            .next()
                            .is_some_and(|character| !is_pattern_whitespace(character))
                            && text
                                .chars()
                                .last()
                                .is_some_and(|character| !is_pattern_whitespace(character))
                    }
                    other => panic!("the interpreter knows no pattern {other:?}"),
                }),
            "propertyNames" => instance.as_object().is_none_or(|members| {
                members
                    .keys()
                    .all(|name| valid(argument, &Value::String(name.clone())))
            }),
            "additionalProperties" => {
                assert!(argument.is_object(), "only a schema is interpreted");
                let Some(members) = instance.as_object() else {
                    return true;
                };
                let named = keywords.get("properties").and_then(Value::as_object);
                members
                    .iter()
                    .filter(|(name, _)| named.is_none_or(|named| !named.contains_key(*name)))
                    .all(|(_, member)| valid(argument, member))
            }
            other => panic!("the schema uses `{other}`, which this interpreter does not know"),
        })
}

/// Whether `c` is matched by `\s` in a JSON Schema pattern, an ECMA-262
/// regular expression: Unicode's `White_Space` less U+0085, plus U+FEFF.
fn is_pattern_whitespace(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
}

/// The table: each file, and whether it is a valid project file.
const FILES: &[(&str, bool)] = &[
    (r#"{ "main": "App.ns" }"#, true),
    (r#"{ "main": "App.ns", "modules": [] }"#, true),
    (
        r#"{ "main": "App.ns", "modules": ["A.ns", "lib/B.ns"] }"#,
        true,
    ),
    (r#"{ "$schema": "x", "main": "App.ns" }"#, true),
    (r#"{ }"#, false),
    (r#"{ "root": "App.ns" }"#, false),
    (r#"{ "main": "App.ns", "entry": "x" }"#, true),
    (r#"{ "main": "" }"#, false),
    (r#"{ "main": "App" }"#, false),
    (r#"{ "main": 1 }"#, false),
    (r#"{ "main": "App.ns", "modules": "A.ns" }"#, false),
    (r#"{ "main": "App.ns", "modules": [""] }"#, false),
    (r#"{ "main": "App.ns", "modules": ["A.txt"] }"#, false),
    (
        r#"{ "main": "App.ns", "modules": ["A.ns", "A.ns"] }"#,
        false,
    ),
    (r#"{ "$schema": 1, "main": "App.ns" }"#, false),
    (r#"{ "$schema": null, "main": "App.ns" }"#, false),
    (r#"[]"#, false),
    (r#"["x", "App.ns"]"#, false),
    (r#"{ "main": "App.ns", "scripts": {} }"#, true),
    (
        r#"{ "main": "App.ns", "scripts": { "start": [], "timed": ["--timed", ""] } }"#,
        true,
    ),
    (r#"{ "main": "App.ns", "scripts": ["--timed"] }"#, false),
    (
        r#"{ "main": "App.ns", "scripts": { "timed": "--timed" } }"#,
        false,
    ),
    (
        r#"{ "main": "App.ns", "scripts": { "timed": [1] } }"#,
        false,
    ),
    (
        r#"{ "main": "App.ns", "scripts": { "two words": [], "-x": [], "a": [] } }"#,
        true,
    ),
    (r#"{ "main": "App.ns", "scripts": { "": [] } }"#, false),
    (
        r#"{ "main": "App.ns", "scripts": { " timed": [] } }"#,
        false,
    ),
    (
        r#"{ "main": "App.ns", "scripts": { "timed ": [] } }"#,
        false,
    ),
    (
        r#"{ "main": "App.ns", "scripts": { "timed\n": [] } }"#,
        false,
    ),
    (
        r#"{ "main": "App.ns", "scripts": { "timed ": [] } }"#,
        false,
    ),
];

/// Whether `ns-next run` reads `contents` as a project file: a valid one
/// gets as far as its missing `App.ns`; an invalid one is refused before,
/// with a message that names the project file's path.
fn reader_accepts(contents: &str, index: usize) -> bool {
    let directory = ScratchDirectory::new(&format!("project-schema-{index}"));
    std::fs::write(directory.join("newspeak.json"), contents).expect("write the file");
    let out = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(directory.path())
        .output()
        .expect("run ns-next");
    assert!(
        !out.status.success(),
        "no module exists, so nothing may run"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr.contains("App.ns") && !stderr.contains("newspeak.json")
}

#[test]
fn the_schema_and_the_reader_agree_on_every_file() {
    let schema = schema();
    for (index, (contents, expected)) in FILES.iter().enumerate() {
        let instance: Value = serde_json::from_str(contents).expect("the table holds JSON");
        assert_eq!(
            valid(&schema, &instance),
            *expected,
            "the schema on {contents}"
        );
        assert_eq!(
            reader_accepts(contents, index),
            *expected,
            "the reader on {contents}"
        );
    }
}
