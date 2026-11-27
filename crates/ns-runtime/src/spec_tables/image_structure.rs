//! The specification's image structure (`docs/spec/image-format.md`,
//! "Structure") held to the encoder: a walker reads real images by the
//! chapter's table `image-structures` -- and a body by the bytecode
//! chapter's `wire-body`, `wire-tags` and `wire-layouts` -- and must end
//! exactly at each image's last byte, every row and tagged variant of the
//! table reached by some sample.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use indoc::indoc;

use super::{bytecode_table, image_table};
use crate::heap::ActivationHeader;
use crate::image::ModuleSource;
use crate::interpreter::{Arguments, FeedbackBase, HostResult, HostSignal, Interpreter};
use crate::modules::{ModuleId, ModuleIdentity, Version};
use crate::types::{
    ActivationKind, BuiltinClass, ClassReference, Continuation, ObjectReference, SourceOrigin,
    Value,
};
use crate::{Mutator, Root, Symbol};

/// How an item repeats or is present: `<type>*<field>`, `<type>?<field>`
/// and `<type>?flag:<name>`.
#[derive(Clone)]
enum Occurrence {
    Once,
    Repeated(String),
    PresentWhen(String),
    PresentWithFlag(String),
}

/// One `<field>:<type>` of a row.
#[derive(Clone)]
struct Item {
    field: String,
    kind: String,
    occurrence: Occurrence,
}

fn item(cell: &str) -> Item {
    let (field, rest) = cell
        .split_once(':')
        .unwrap_or_else(|| panic!("`{cell}` is no `<field>:<type>` item"));
    let (kind, occurrence) = if let Some((kind, flag)) = rest.split_once("?flag:") {
        (kind, Occurrence::PresentWithFlag(flag.to_string()))
    } else if let Some((kind, field)) = rest.split_once('?') {
        (kind, Occurrence::PresentWhen(field.to_string()))
    } else if let Some((kind, field)) = rest.split_once('*') {
        (kind, Occurrence::Repeated(field.to_string()))
    } else {
        (rest, Occurrence::Once)
    };
    Item {
        field: field.to_string(),
        kind: kind.to_string(),
        occurrence,
    }
}

/// The table `image-structures`, read into its structures, and the tags of
/// `image-tags` it selects among.
struct Grammar {
    /// The first row's structure: the whole file.
    first: String,
    plain: HashMap<String, Vec<Item>>,
    /// Each tagged structure's tag space.
    tagged: HashMap<String, String>,
    /// Each variant's items, by structure and tag name.
    variants: HashMap<(String, String), Vec<Item>>,
    /// Every row's name, which the samples must reach.
    rows: BTreeSet<String>,
    /// Every repeated or optional item, as `<row> <field>`, which the
    /// samples must read at least once.
    optional_items: BTreeSet<String>,
    /// Every named tag of `image-tags`, by space and number.
    tags: HashMap<(String, u8), String>,
    /// The header flags' bits, by name.
    flags: HashMap<String, u8>,
}

impl Grammar {
    fn read() -> Grammar {
        let mut tags = HashMap::new();
        let mut flags = HashMap::new();
        for row in image_table("image-tags") {
            let number: u8 = row[2].parse().expect("a tag is a number");
            if row[1] == "retired" {
                continue;
            }
            if row[0] == "header-flag" {
                flags.insert(row[1].clone(), number);
            }
            tags.insert((row[0].clone(), number), row[1].clone());
        }
        let table = image_table("image-structures");
        let first = table
            .first()
            .map(|row| row[0].clone())
            .expect("the table has rows");
        let mut grammar = Grammar {
            first,
            plain: HashMap::new(),
            tagged: HashMap::new(),
            variants: HashMap::new(),
            rows: BTreeSet::new(),
            optional_items: BTreeSet::new(),
            tags,
            flags,
        };
        for row in table {
            let name = row[0].clone();
            let items: Vec<Item> = row[1..].iter().map(|cell| item(cell)).collect();
            assert!(grammar.rows.insert(name.clone()), "the row `{name}` twice");
            for item in &items {
                if !matches!(item.occurrence, Occurrence::Once) {
                    grammar
                        .optional_items
                        .insert(format!("{name} {}", item.field));
                }
            }
            let parts: Vec<&str> = name.split('/').collect();
            match parts.as_slice() {
                [structure] => {
                    grammar.plain.insert(structure.to_string(), items);
                }
                [structure, space] => {
                    assert!(items.is_empty(), "`{name}` declares a tag and no fields");
                    grammar
                        .tagged
                        .insert(structure.to_string(), space.to_string());
                }
                [structure, space, tag] => {
                    assert_eq!(
                        grammar.tagged.get(*structure).map(String::as_str),
                        Some(*space),
                        "`{name}` follows its structure's row `{structure}/{space}`"
                    );
                    assert!(
                        grammar
                            .tags
                            .iter()
                            .any(|((tag_space, _), named)| tag_space == space && named == tag),
                        "`{name}` names no tag of the space `{space}` of `image-tags`"
                    );
                    grammar
                        .variants
                        .insert((structure.to_string(), tag.to_string()), items);
                }
                _ => panic!("`{name}` is no structure, tagged structure or variant"),
            }
        }
        grammar
    }
}

/// The bytecode chapter's tables a body is read by.
struct BodyGrammar {
    body: Vec<Item>,
    /// Every named tag of `wire-tags`, by space and number.
    tags: HashMap<(String, u8), String>,
    /// An instruction's or a literal's operands, by its layout row's name.
    layouts: HashMap<String, Vec<Item>>,
}

impl BodyGrammar {
    fn read() -> BodyGrammar {
        let body = bytecode_table("wire-body")
            .first()
            .expect("the body's row")
            .iter()
            .map(|cell| item(cell))
            .collect();
        let tags = bytecode_table("wire-tags")
            .into_iter()
            .filter(|row| row[1] != "retired")
            .map(|row| {
                let number: u8 = row[2].parse().expect("a tag is a number");
                ((row[0].clone(), number), row[1].clone())
            })
            .collect();
        let layouts = bytecode_table("wire-layouts")
            .into_iter()
            .map(|row| {
                let operands = row[1..].iter().map(|cell| item(cell)).collect();
                (row[0].clone(), operands)
            })
            .collect();
        BodyGrammar {
            body,
            tags,
            layouts,
        }
    }
}

/// A field's value as the walker read it.
#[derive(Clone, Debug, PartialEq)]
enum Field {
    Number(u64),
    Signed(i64),
    Float(f64),
    Text(String),
    Bytes(usize),
}

/// The ids of the image, each the `u32` index of an entry of the section
/// whose count field the second gives.
const IDS: [(&str, &str); 6] = [
    ("symbol", "symbols.count"),
    ("code-id", "code.count"),
    ("text-index", "texts.count"),
    ("row-id", "mixins.count"),
    ("shipped-index", "shipped-names.count"),
    ("object-id", "graph.count"),
];

/// Reads one image by the grammar, recording each field under its path.
struct Walker<'bytes, 'grammar> {
    bytes: &'bytes [u8],
    position: usize,
    grammar: &'grammar Grammar,
    body: &'grammar BodyGrammar,
    /// Every field read, in the order read.
    fields: Vec<(String, Field)>,
    /// The same, by path.
    by_path: BTreeMap<String, Field>,
    /// Every row the walk reached, and every repeated or optional item it
    /// read at least once, as `<row> <field>`.
    reached: BTreeSet<String>,
    /// Each id's bound, once its section's count is read.
    counts: HashMap<&'static str, u64>,
    /// Every id read, checked against its bound once the walk ends.
    references: Vec<(&'static str, u64, String)>,
    /// The symbols the code and mixin tables name, in the order first named.
    symbol_order: Vec<u64>,
}

impl<'bytes, 'grammar> Walker<'bytes, 'grammar> {
    fn walk(
        bytes: &'bytes [u8],
        grammar: &'grammar Grammar,
        body: &'grammar BodyGrammar,
    ) -> Walker<'bytes, 'grammar> {
        let mut walker = Walker {
            bytes,
            position: 0,
            grammar,
            body,
            fields: Vec::new(),
            by_path: BTreeMap::new(),
            reached: BTreeSet::new(),
            counts: HashMap::new(),
            references: Vec::new(),
            symbol_order: Vec::new(),
        };
        let first = grammar.first.clone();
        walker.structure(&first, "");
        assert_eq!(
            walker.position,
            bytes.len(),
            "the walk ends at the image's last byte"
        );
        for (kind, value, path) in &walker.references {
            let bound = walker
                .counts
                .get(kind)
                .unwrap_or_else(|| panic!("`{path}` is a {kind} of a section the image lacks"));
            assert!(
                value < bound,
                "`{path}` is {kind} {value}, past the {bound} it indexes"
            );
        }
        walker
    }

    fn take(&mut self, length: usize) -> &'bytes [u8] {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .unwrap_or_else(|| panic!("the image ends at {} inside a field", self.bytes.len()));
        let taken = &self.bytes[self.position..end];
        self.position = end;
        taken
    }

    fn unsigned(&mut self, width: usize) -> u64 {
        let mut word = [0u8; 8];
        word[..width].copy_from_slice(self.take(width));
        u64::from_le_bytes(word)
    }

    fn record(&mut self, path: &str, field: Field) {
        self.fields.push((path.to_string(), field.clone()));
        self.by_path.insert(path.to_string(), field);
    }

    fn number(&self, path: &str) -> u64 {
        match self.by_path.get(path) {
            Some(Field::Number(number)) => *number,
            _ => panic!("`{path}` is no number the walk read"),
        }
    }

    /// The header's flag byte, as read.
    fn has_flag(&self, name: &str) -> bool {
        let bit = self
            .grammar
            .flags
            .get(name)
            .unwrap_or_else(|| panic!("no header flag `{name}`"));
        self.number("header.flags") & u64::from(*bit) != 0
    }

    fn structure(&mut self, name: &str, path: &str) {
        if let Some(space) = self.grammar.tagged.get(name).cloned() {
            let tag = self.take(1)[0];
            let tag_name = self
                .grammar
                .tags
                .get(&(space.clone(), tag))
                .unwrap_or_else(|| panic!("`{path}`: {tag} is no {space} tag"))
                .clone();
            let variant = (name.to_string(), tag_name.clone());
            let items = self
                .grammar
                .variants
                .get(&variant)
                .unwrap_or_else(|| {
                    panic!("`{path}`: the table has no row `{name}/{space}/{tag_name}`")
                })
                .clone();
            self.reached.insert(format!("{name}/{space}"));
            self.reached.insert(format!("{name}/{space}/{tag_name}"));
            self.record(&format!("{path}.tag"), Field::Text(tag_name.clone()));
            self.items(&format!("{name}/{space}/{tag_name}"), &items, path);
            if (space.as_str(), tag_name.as_str()) == ("value", "large-integer") {
                self.check_large_integer(path);
            }
            return;
        }
        let items = self
            .grammar
            .plain
            .get(name)
            .unwrap_or_else(|| panic!("`{path}`: the table has no structure `{name}`"))
            .clone();
        self.reached.insert(name.to_string());
        self.items(name, &items, path);
    }

    fn items(&mut self, row: &str, items: &[Item], path: &str) {
        for item in items {
            let field_path = if path.is_empty() {
                item.field.clone()
            } else {
                format!("{path}.{}", item.field)
            };
            let earlier = |walker: &Walker, field: &str| {
                walker.number(&if path.is_empty() {
                    field.to_string()
                } else {
                    format!("{path}.{field}")
                })
            };
            match &item.occurrence {
                Occurrence::Once => self.value(&item.kind, &field_path),
                Occurrence::Repeated(count) => {
                    let count = earlier(self, count);
                    if count > 0 {
                        self.reached.insert(format!("{row} {}", item.field));
                    }
                    for index in 0..count {
                        self.value(&item.kind, &format!("{field_path}[{index}]"));
                    }
                }
                Occurrence::PresentWhen(field) => {
                    if earlier(self, field) != 0 {
                        self.reached.insert(format!("{row} {}", item.field));
                        self.value(&item.kind, &field_path);
                    }
                }
                Occurrence::PresentWithFlag(flag) => {
                    if self.has_flag(flag) {
                        self.reached.insert(format!("{row} {}", item.field));
                        self.value(&item.kind, &field_path);
                    }
                }
            }
        }
    }

    fn value(&mut self, kind: &str, path: &str) {
        match kind {
            "u8" => self.scalar(1, path),
            "u16" => self.scalar(2, path),
            "u32" => self.scalar(4, path),
            "u64" => self.scalar(8, path),
            "i64" => {
                let bits = self.unsigned(8);
                self.record(path, Field::Signed(i64::from_le_bytes(bits.to_le_bytes())));
            }
            "f64" => {
                let bits = self.unsigned(8);
                self.record(path, Field::Float(f64::from_bits(bits)));
            }
            "magic" => {
                let magic = self.take(4);
                assert_eq!(magic, b"NSNX", "`{path}` is the magic");
                self.record(path, Field::Text("NSNX".to_string()));
            }
            "bytes" => {
                let length = usize::try_from(self.unsigned(4)).expect("a u32 fits a usize");
                self.take(length);
                self.record(path, Field::Bytes(length));
            }
            "text" => {
                let length = usize::try_from(self.unsigned(4)).expect("a u32 fits a usize");
                let text = std::str::from_utf8(self.take(length))
                    .unwrap_or_else(|_| panic!("`{path}` is no UTF-8"))
                    .to_string();
                self.record(path, Field::Text(text));
            }
            "body" => self.body(path),
            id if IDS.iter().any(|(name, _)| *name == id) => {
                let value = self.unsigned(4);
                self.reference(id, value, path);
            }
            structure => self.structure(structure, path),
        }
    }

    fn scalar(&mut self, width: usize, path: &str) {
        let value = self.unsigned(width);
        self.record(path, Field::Number(value));
        if let Some((kind, _)) = IDS.iter().find(|(_, count)| *count == path) {
            self.counts.insert(kind, value);
        }
    }

    fn reference(&mut self, kind: &str, value: u64, path: &str) {
        let (kind, _) = IDS
            .iter()
            .find(|(name, _)| *name == kind)
            .expect("an id of the table");
        self.record(path, Field::Number(value));
        self.references.push((kind, value, path.to_string()));
        if *kind == "symbol"
            && (path.starts_with("code.") || path.starts_with("mixins."))
            && !self.symbol_order.contains(&value)
        {
            self.symbol_order.push(value);
        }
    }

    /// A `value/large-integer` is written so only when no `i64` holds it,
    /// its most significant digit not 0.
    fn check_large_integer(&self, path: &str) {
        let count = self.number(&format!("{path}.digit_count"));
        assert!(count > 0, "`{path}` has digits");
        let top = self.number(&format!("{path}.digit[{}]", count - 1));
        assert_ne!(top, 0, "`{path}`'s most significant digit is not 0");
        if count == 1 {
            let negative = self.number(&format!("{path}.negative")) != 0;
            let fits = top <= i64::MAX as u64 || (negative && top == 1 << 63);
            assert!(!fits, "`{path}` fits an i64 and is written as one");
        }
    }

    /// A body, as the bytecode chapter's tables give it; `path` is
    /// `code.body[<index>]`, and every `code` operand names an earlier body.
    fn body(&mut self, path: &str) {
        let own_index: u64 = path
            .rsplit_once('[')
            .and_then(|(_, index)| index.trim_end_matches(']').parse().ok())
            .expect("a body is an entry of the code table");
        let items = self.body.body.clone();
        for item in &items {
            let field_path = format!("{path}.{}", item.field);
            match item.kind.as_str() {
                "statements" => {
                    let count = self.unsigned(4);
                    self.record(&field_path, Field::Number(count));
                    for index in 0..count {
                        for part in ["statement", "start", "end"] {
                            self.scalar(4, &format!("{field_path}[{index}].{part}"));
                        }
                    }
                }
                "instructions" => {
                    let count = self.number(&format!("{path}.instruction_count"));
                    for index in 0..count {
                        self.instruction(&format!("{field_path}[{index}]"), own_index);
                    }
                }
                // `wire-body` types its kind `u8`; the field's name, `kind`,
                // is what says the byte is a tag of the space `code-kind`.
                _ if item.field == "kind" => {
                    let tag = self.take(1)[0];
                    let kind = self
                        .body
                        .tags
                        .get(&("code-kind".to_string(), tag))
                        .unwrap_or_else(|| panic!("`{field_path}`: {tag} is no code kind"))
                        .clone();
                    self.record(&field_path, Field::Text(kind));
                }
                "u8" => self.scalar(1, &field_path),
                "u16" => self.scalar(2, &field_path),
                "u32" => self.scalar(4, &field_path),
                other => panic!("`{field_path}`: a body field of type `{other}`"),
            }
        }
    }

    fn instruction(&mut self, path: &str, own_index: u64) {
        let tag = self.take(1)[0];
        let name = self
            .body
            .tags
            .get(&("instruction".to_string(), tag))
            .unwrap_or_else(|| panic!("`{path}`: {tag} is no instruction"))
            .clone();
        self.record(&format!("{path}.tag"), Field::Text(name.clone()));
        let operands = self.body.layouts[&name].clone();
        self.operands(&operands, path, own_index);
    }

    fn operands(&mut self, operands: &[Item], path: &str, own_index: u64) {
        for operand in operands {
            let operand_path = format!("{path}.{}", operand.field);
            match operand.kind.as_str() {
                "u8" => self.scalar(1, &operand_path),
                "u16" => self.scalar(2, &operand_path),
                "u32" => self.scalar(4, &operand_path),
                "i64" | "f64" | "bytes" => self.value(&operand.kind, &operand_path),
                "symbol" => {
                    let value = self.unsigned(4);
                    self.reference("symbol", value, &operand_path);
                }
                "registers" => {
                    let count = self.unsigned(2);
                    self.record(&operand_path, Field::Number(count));
                    for index in 0..count {
                        self.scalar(2, &format!("{operand_path}[{index}]"));
                    }
                }
                "symbols" => {
                    let count = self.unsigned(2);
                    self.record(&operand_path, Field::Number(count));
                    for index in 0..count {
                        let value = self.unsigned(4);
                        self.reference("symbol", value, &format!("{operand_path}[{index}]"));
                    }
                }
                "code" => {
                    let value = self.unsigned(4);
                    assert!(
                        value < own_index,
                        "`{operand_path}` names body {value}, not one before body {own_index}"
                    );
                    self.reference("code-id", value, &operand_path);
                }
                "large-integer" => {
                    self.scalar(1, &format!("{operand_path}.negative"));
                    let count = self.unsigned(4);
                    self.record(&format!("{operand_path}.digit_count"), Field::Number(count));
                    for index in 0..count {
                        self.scalar(8, &format!("{operand_path}.digit[{index}]"));
                    }
                }
                "literal" => {
                    let tag = self.take(1)[0];
                    let name = self
                        .body
                        .tags
                        .get(&("literal".to_string(), tag))
                        .unwrap_or_else(|| panic!("`{operand_path}`: {tag} is no literal"))
                        .clone();
                    self.record(&format!("{operand_path}.tag"), Field::Text(name.clone()));
                    let layout = self.body.layouts[&format!("literal/{name}")].clone();
                    self.operands(&layout, &operand_path, own_index);
                }
                other => panic!("`{operand_path}`: an operand of type `{other}`"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The samples, built as `tests/image.rs`'s helpers build theirs.
// ---------------------------------------------------------------------------

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

fn load_into(interpreter: &mut Interpreter, source: &str, path: &str) -> Root {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from(path)),
    )
    .expect("resolve");
    interpreter.load_module(&resolved)
}

fn class_reference<'gc>(mutator: &Mutator<'gc>, root: &Root) -> ClassReference<'gc> {
    mutator
        .fetch(root)
        .as_object()
        .expect("expected a class object")
}

fn identity(path: &str, id: &str, class_name: &str, version: Option<&str>) -> ModuleIdentity {
    ModuleIdentity {
        path: Some(path.to_string()),
        id: ModuleId::parse(id, class_name).unwrap_or_else(|error| panic!("{error}")),
        version: version.map(|text| Version::parse(text).unwrap_or_else(|error| panic!("{error}"))),
    }
}

/// One image the test walks, with what it knows of it.
struct Sample {
    name: &'static str,
    bytes: Vec<u8>,
    /// The flag byte the image was built with.
    flags: u8,
    /// Each module's path, id and version as the host named it.
    modules: Vec<ModuleIdentity>,
}

const APPLICATION: &str = indoc! {"
    class App = Object new (| public count ::= 41. |) (
      public main: platform args: arguments = ( count:: count + 1. ^count )
    ) : ()
"};

const MANIFEST: &str = indoc! {"
    class Manifest = Object new () () : ()
"};

/// An application of two modules, one versioned and one the binary's
/// own, packaged with its source or without.
fn packaged_application(with_source: bool) -> Sample {
    let mut interpreter = Interpreter::bootstrap();
    let application = load_into(&mut interpreter, APPLICATION, "src/App.ns");
    let manifest = load_into(&mut interpreter, MANIFEST, "Manifest.ns");
    let instance = expect_ok(interpreter.instantiate(&application, &[]), "App new");
    let identities = vec![
        identity("src/App.ns", "com.acme.App", "App", Some("1.2.0")),
        identity(
            "core:Manifest.ns",
            "newspeak.core.Manifest",
            "Manifest",
            None,
        ),
    ];
    let bytes = interpreter
        .enter(|mutator| {
            let modules: Vec<ModuleSource> = [&application, &manifest]
                .into_iter()
                .zip(&identities)
                .map(|(class, identity)| ModuleSource {
                    class: class_reference(mutator, class),
                    identity,
                    with_source,
                })
                .collect();
            crate::image::package(mutator, &modules, mutator.fetch(&instance))
        })
        .unwrap_or_else(|error| panic!("{error}"));
    let source_flag = if with_source { 8 } else { 0 };
    Sample {
        name: if with_source {
            "the packaged application with source"
        } else {
            "the packaged application without source"
        },
        bytes,
        flags: 4 | source_flag,
        modules: identities,
    }
}

/// A module whose instance holds a value of every kind the graph writes
/// that a program makes: closures over a method's, a block's and an
/// initializer's activation, a string, a symbol, an array of the
/// constants, integers in and past an `i64`, a fraction, an immediate and
/// a boxed float, the instances of a body-less declaration and of levels
/// `<:` clauses added, a closure that copied a value, and every kind and
/// access of slot and method.
const EVERYTHING: &str = indoc! {"
    class Everything usingPlatform: platform = Object new (|
      public held ::= nil.
      public block = [ 2 ].
      public nested = [ [ 3 ] ] value.
      public text = 'a string'.
      public symbol = #aSymbol.
      public constants = { true. false. nil. 7 }.
      public wide = 4611686018427387904.
      public large = 1000000000000 * 1000000000000 * 1000000000000.
      public negative = 0 - (1000000000000 * 1000000000000 * 1000000000000).
      public fraction = 1 / 3.
      public huge = 1.0e300.
      public small = 1.5.
      public applied = Applied new.
      public both = Both new.
      public again = Again new.
      public added = adder: 4.
      public markerClass = Marker.
      protected guarded = 9.
      private hidden = 10.
    |) (
      public class Marker = Object new (| public kept = [ 4 ]. |) () : (
        public make = ( ^self new )
      )
      public class Applied = Object new <: Marker.
      public class Both = Object new <: Marker (| public own = [ 5 ]. |) (
        public answer = ( ^own value )
      )
      public class Again = Object new <: Both () ()
      lazy public computed = 7.
      lazy public counter ::= 8.
      public makeClosure = ( ^[ 1 ] )
      public adder: number = ( ^[ number + 1 ] )
      protected guardedMethod = ( ^1 )
      private hiddenMethod = ( ^2 )
    ) : (
      public build: platform = ( ^self usingPlatform: platform )
    )
"};

/// The frame object of an activation of `header` pushed with nil
/// arguments, reified and popped: a returned frame of any kind, as a
/// program leaves one behind when a block outlives its activation.
fn returned_frame<'gc>(
    mutator: &mut Mutator<'gc>,
    header: ActivationHeader<'gc>,
) -> ObjectReference<'gc> {
    let nils = vec![Value::nil(); usize::from(header.code.argument_count)];
    let arguments = nils.as_slice();
    let temporaries = header.code.registers_above_header();
    let heap = mutator.heap_mut();
    let activation = heap
        .push_activation(header, Arguments::Values(&arguments), temporaries)
        .unwrap_or_else(|_| panic!("the stack holds the frame"));
    let frame = heap.reify(activation);
    heap.pop_activation(activation);
    heap.frame_object(frame)
}

fn header<'gc>(
    receiver: Value<'gc>,
    current_class: Option<ClassReference<'gc>>,
    kind: ActivationKind<'gc>,
    method: Option<Rc<crate::mixin::MixinMethod>>,
    code: Rc<crate::bytecode::CompiledCode>,
) -> ActivationHeader<'gc> {
    ActivationHeader {
        receiver,
        current_class,
        kind,
        method,
        feedback_base: FeedbackBase::NOT_ASSIGNED,
        code,
        then: Continuation::Deliver,
        counted: true,
    }
}

/// A frame of every kind and body the frame record spells that no program
/// leaves reachable: an initializer's own part and its standalone body,
/// each at a level a `<:` clause added that applies the body's mixin, as
/// the runtime enters them, a superclass clause's, a factory's, a shipped
/// method's, and two top-level frames, the one the other's sender, holding
/// the kernel's module instance in a register.
fn every_frame<'gc>(
    mutator: &mut Mutator<'gc>,
    module: ClassReference<'gc>,
    marker: ClassReference<'gc>,
    both: ClassReference<'gc>,
    again: ClassReference<'gc>,
) -> Vec<Value<'gc>> {
    use crate::interpreter::instantiate::{InitializerBody, factory_code};
    let mixin_of = |mutator: &Mutator<'gc>, class: ClassReference<'gc>| {
        mutator
            .heap()
            .class_application(class)
            .expect("a class object")
            .mixin
            .clone()
    };
    let module_mixin = mixin_of(mutator, module);
    let marker_mixin = mixin_of(mutator, marker);
    let both_mixin = mixin_of(mutator, both);
    let full = module_mixin
        .initializer_code
        .clone()
        .expect("a full initializer");
    let own_part = marker_mixin
        .own_initializer_code
        .clone()
        .expect("an own part");
    let standalone = both_mixin
        .standalone_initializer_code
        .clone()
        .expect("a standalone initializer");
    // The level `Both`'s clause adds applies `Marker`, the one `Again`'s
    // adds applies `Both`.
    let level_of =
        |mutator: &Mutator<'gc>, class: ClassReference<'gc>, mixin: &Rc<crate::mixin::Mixin>| {
            let level = mutator
                .heap()
                .class_superclass(class)
                .expect("a level below the class");
            assert!(
                Rc::ptr_eq(&mixin_of(mutator, level), mixin),
                "the level applies the mixin"
            );
            level
        };
    let marker_level = level_of(mutator, both, &marker_mixin);
    let both_level = level_of(mutator, again, &both_mixin);
    let clause = module_mixin
        .superclass_clause_code
        .clone()
        .expect("a superclass clause body");
    let factory = factory_code(mutator.heap(), module).expect("a factory body");
    let object_class = mutator.builtin_class(BuiltinClass::Object);
    let object_mixin = mixin_of(mutator, object_class);
    let shipped_method = object_mixin
        .compiled_methods()
        .next()
        .cloned()
        .expect("Object declares a method");
    let initializer = |class, body| ActivationKind::Initializer(class, body);
    let headers = vec![
        header(
            Value::nil(),
            Some(module),
            initializer(module, InitializerBody::Full),
            None,
            full,
        ),
        header(
            Value::nil(),
            Some(marker_level),
            initializer(marker_level, InitializerBody::OwnPart),
            None,
            own_part,
        ),
        header(
            Value::nil(),
            Some(both_level),
            initializer(both_level, InitializerBody::Standalone),
            None,
            standalone,
        ),
        header(
            Value::nil(),
            Some(module),
            ActivationKind::SuperclassClause(module),
            None,
            clause,
        ),
        header(
            Value::object(module),
            Some(module),
            ActivationKind::Factory(module),
            None,
            factory,
        ),
        header(
            Value::nil(),
            Some(object_class),
            ActivationKind::Method(shipped_method.selector),
            Some(shipped_method.clone()),
            shipped_method.code.clone(),
        ),
    ];
    let mut frames: Vec<Value<'gc>> = headers
        .into_iter()
        .map(|header| Value::object(returned_frame(mutator, header)))
        .collect();
    let kernel = mutator.kernel.expect("the bootstrap made the kernel");
    let sender =
        mutator
            .heap_mut()
            .allocate_returned_top_level_frame(Value::nil(), &[kernel], None, None);
    let top_level = mutator.heap_mut().allocate_returned_top_level_frame(
        Value::small_integer(8),
        &[],
        Some(sender),
        Some(module),
    );
    frames.push(Value::object(mutator.heap().frame_object(top_level)));
    frames
}

/// `EVERYTHING`'s instance, and an array of what no program makes: a
/// frame of every kind, a `ByteArray`, a `WeakArray`, an `Ephemeron`, the
/// reified mixins of both sides of the module's class and of a shipped
/// one, and classes as values -- with the identity hashes of a shipped
/// level, its metaclass and a shipped metaclass given -- then a small
/// integer, as three roots.
fn everything() -> Sample {
    let mut interpreter = Interpreter::bootstrap();
    let class = load_into(&mut interpreter, EVERYTHING, "Everything.ns");
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "Everything usingPlatform:",
    );
    let marker = expect_ok(
        interpreter.send(&instance, Symbol::intern("Marker"), &[]),
        "Marker",
    );
    let both = expect_ok(
        interpreter.send(&instance, Symbol::intern("Both"), &[]),
        "Both",
    );
    let again = expect_ok(
        interpreter.send(&instance, Symbol::intern("Again"), &[]),
        "Again",
    );
    let closure = expect_ok(
        interpreter.send(&instance, Symbol::intern("makeClosure"), &[]),
        "makeClosure",
    );
    let identities = vec![identity("Everything.ns", "Everything", "Everything", None)];
    let bytes = interpreter.enter(|mutator| {
        let module = class_reference(mutator, &class);
        let marker = class_reference(mutator, &marker);
        let both = class_reference(mutator, &both);
        let again = class_reference(mutator, &again);
        let mut held = every_frame(mutator, module, marker, both, again);
        held.push(Value::object(mutator.new_byte_array(&[1, 2, 3])));
        let weak_class = mutator.builtin_class(BuiltinClass::WeakArray);
        let weak = mutator.heap_mut().allocate_weak_array(weak_class, 2);
        assert!(
            mutator
                .heap_mut()
                .weak_array_set_element(weak, 0, Value::object(module))
        );
        held.push(Value::object(weak));
        let ephemeron_class = mutator.builtin_class(BuiltinClass::Ephemeron);
        let ephemeron = mutator.heap_mut().allocate_ephemeron(ephemeron_class, 3);
        assert!(
            mutator
                .heap_mut()
                .ephemeron_set_field(ephemeron, 0, Value::object(module))
        );
        held.push(Value::object(ephemeron));
        let module_mixin = mutator
            .heap()
            .class_application(module)
            .expect("a class")
            .mixin
            .clone();
        let class_side = mutator
            .heap()
            .class_metaclass(module)
            .and_then(|metaclass| mutator.heap().class_application(metaclass))
            .expect("a metaclass")
            .mixin
            .clone();
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let object_mixin = mutator
            .heap()
            .class_application(object_class)
            .expect("a class")
            .mixin
            .clone();
        for mixin in [module_mixin, class_side, object_mixin] {
            held.push(Value::object(mutator.reified_mixin(&mixin)));
        }
        let array_class = mutator.builtin_class(BuiltinClass::Array);
        held.push(Value::object(array_class));
        held.push(Value::object(marker));
        held.push(mutator.fetch(&closure));
        let level = mutator
            .heap()
            .class_superclass(array_class)
            .expect("Array has a superclass");
        let level_metaclass = mutator
            .heap()
            .class_metaclass(level)
            .expect("a level has a metaclass");
        let object_metaclass = mutator
            .heap()
            .class_metaclass(object_class)
            .expect("Object has a metaclass");
        for hashed in [level, level_metaclass, object_metaclass] {
            mutator.heap_mut().object_ensure_identity_hash(hashed);
        }
        let array = mutator.new_array(held);
        let modules = [ModuleSource {
            class: module,
            identity: &identities[0],
            with_source: true,
        }];
        let roots = [
            mutator.fetch(&instance),
            Value::object(array),
            Value::small_integer(3),
        ];
        crate::image::snapshot(mutator, &modules, &roots).unwrap_or_else(|error| panic!("{error}"))
    });
    Sample {
        name: "everything a snapshot holds",
        bytes,
        flags: 8,
        modules: identities,
    }
}

/// A level a nested class's `<:` clause added over a shipped class, whose
/// record names its mixin by the shipped name, in a snapshot without
/// source of a module without a path, as a module a program declared from
/// source is written.
fn level_over_a_shipped_class() -> Sample {
    let source = indoc! {"
        class Host usingPlatform: p = Object <: Value () (
          public class Point = Object new <: Value () (
            public answer = ( ^7 )
          )
        ) : ()
    "};
    let mut interpreter = Interpreter::bootstrap();
    let class = load_into(&mut interpreter, source, "Host.ns");
    let platform = interpreter.platform();
    let host = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "Host usingPlatform:",
    );
    let point_class = expect_ok(
        interpreter.send(&host, Symbol::intern("Point"), &[]),
        "Point",
    );
    let point = expect_ok(interpreter.instantiate(&point_class, &[]), "Point new");
    let identities = vec![ModuleIdentity {
        path: None,
        ..identity("Host.ns", "Host", "Host", None)
    }];
    let bytes = interpreter.enter(|mutator| {
        let modules = [ModuleSource {
            class: class_reference(mutator, &class),
            identity: &identities[0],
            with_source: false,
        }];
        crate::image::snapshot(mutator, &modules, &[mutator.fetch(&point)])
            .unwrap_or_else(|error| panic!("{error}"))
    });
    Sample {
        name: "a level over a shipped class",
        bytes,
        flags: 0,
        modules: identities,
    }
}

fn text_at(walker: &Walker, path: &str) -> String {
    match walker.by_path.get(path) {
        Some(Field::Text(text)) => text.clone(),
        _ => panic!("`{path}` is no text the walk read"),
    }
}

/// What the sections' rules say of an image beyond its layout, held to
/// the fields `walker` read.
fn check_sections(walker: &Walker, name: &str) {
    // `image.symbols`: each text once.
    let symbol_count = walker.number("symbols.count");
    let texts: BTreeSet<String> = (0..symbol_count)
        .map(|index| text_at(walker, &format!("symbols.symbol[{index}]")))
        .collect();
    assert_eq!(texts.len() as u64, symbol_count, "{name}: a symbol twice");

    // `image.mixins`: a forest, each tree in preorder -- a row, its nested
    // rows, then its class side -- the trees in module order.
    let row_count = walker.number("mixins.count");
    let row = |index: u64| format!("mixins.row[{index}]");
    let children = |index: u64| -> Vec<u64> {
        let nested = (0..walker.number(&format!("{}.nested_count", row(index))))
            .map(|child| walker.number(&format!("{}.nested[{child}]", row(index))));
        let class_side = (walker.number(&format!("{}.has_class_side", row(index))) != 0)
            .then(|| walker.number(&format!("{}.class_side", row(index))));
        nested.chain(class_side).collect()
    };
    let child_rows: BTreeSet<u64> = (0..row_count).flat_map(children).collect();
    let roots: Vec<u64> = (0..row_count)
        .filter(|index| !child_rows.contains(index))
        .collect();
    let mut preorder = Vec::new();
    for root in &roots {
        let mut pending = vec![*root];
        while let Some(index) = pending.pop() {
            preorder.push(index);
            pending.extend(children(index).into_iter().rev());
        }
    }
    assert_eq!(
        preorder,
        (0..row_count).collect::<Vec<_>>(),
        "{name}: the rows in preorder, tree after tree"
    );

    // `image.modules`: one record per tree, in module order, its root the
    // class record of the tree's root row.
    let module_count = walker.number("modules.count");
    assert_eq!(
        module_count,
        roots.len() as u64,
        "{name}: a tree per module"
    );
    for (index, root_row) in roots.iter().enumerate() {
        let module = format!("modules.module[{index}]");
        let id = text_at(walker, &format!("{module}.id"));
        let root_name = text_at(
            walker,
            &format!(
                "symbols.symbol[{}]",
                walker.number(&format!("{}.name", row(*root_row)))
            ),
        );
        assert!(
            id.rsplit('.').next() == Some(root_name.as_str()),
            "{name}: `{module}` is {id}, and its tree's root {root_name}"
        );
        let record = format!(
            "graph.object[{}].body",
            walker.number(&format!("{module}.root"))
        );
        assert_eq!(text_at(walker, &format!("{record}.tag")), "class", "{name}");
        assert_eq!(
            text_at(walker, &format!("{record}.record.tag")),
            "row",
            "{name}"
        );
        assert_eq!(
            walker.number(&format!("{record}.record.row")),
            *root_row,
            "{name}: `{module}`'s root class applies its tree's root row"
        );
    }

    // `image.texts`: one text per row that is no class side, a class
    // side's row naming its declaration's.
    if walker.by_path.contains_key("texts.count") {
        let class_sides: Vec<(u64, u64)> = (0..row_count)
            .filter(|index| walker.number(&format!("{}.has_class_side", row(*index))) != 0)
            .map(|index| (index, walker.number(&format!("{}.class_side", row(index)))))
            .collect();
        assert_eq!(
            walker.number("texts.count"),
            row_count - class_sides.len() as u64,
            "{name}: a text per declaration"
        );
        for (instance_side, class_side) in class_sides {
            assert_eq!(
                walker.number(&format!("{}.text", row(class_side))),
                walker.number(&format!("{}.text", row(instance_side))),
                "{name}: a class side names its declaration's text"
            );
        }
    }

    // `image.shipped-names`: each name once, and each named by the graph
    // or the hashes.
    let name_count = walker.number("shipped-names.count");
    let names: BTreeSet<(u64, u64, String, Option<u64>)> = (0..name_count)
        .map(|index| {
            let entry = format!("shipped-names.name[{index}]");
            (
                walker.number(&format!("{entry}.module")),
                walker.number(&format!("{entry}.declaration")),
                text_at(walker, &format!("{entry}.kind.tag")),
                walker
                    .by_path
                    .get(&format!("{entry}.kind.clause"))
                    .map(|_| walker.number(&format!("{entry}.kind.clause"))),
            )
        })
        .collect();
    assert_eq!(
        names.len() as u64,
        name_count,
        "{name}: a shipped name twice"
    );
    let used: BTreeSet<u64> = walker
        .references
        .iter()
        .filter(|(kind, ..)| *kind == "shipped-index")
        .map(|(_, value, _)| *value)
        .collect();
    assert_eq!(
        used,
        (0..name_count).collect(),
        "{name}: every shipped name is named"
    );

    // `image.hashes`: never 0.
    for index in 0..walker.number("hashes.count") {
        assert_ne!(
            walker.number(&format!("hashes.entry[{index}].hash")),
            0,
            "{name}: a hash entry of 0"
        );
    }
}

/// The body kind the code id at `path` names.
fn body_kind(walker: &Walker, path: &str) -> String {
    text_at(walker, &format!("code.body[{}].kind", walker.number(path)))
}

/// The body tag of the record the object id at `path` names.
fn record_tag(walker: &Walker, path: &str) -> String {
    text_at(
        walker,
        &format!("graph.object[{}].body.tag", walker.number(path)),
    )
}

/// That the two fields of each range the mixin and text tables write as
/// `start` and `end` come in that order, and that each id names what its
/// field says: a method's body a `Method`, an initializer's an
/// `Initializer`, a clause's a `SuperclassClause`, a closure's a `Block`,
/// a closure's frame a frame record.
fn check_ranges_and_references(walker: &Walker, name: &str) {
    for (path, _) in &walker.fields {
        if let Some(range) = path.strip_suffix(".start")
            && (range.starts_with("mixins.") || range.starts_with("texts."))
        {
            assert!(
                walker.number(path) <= walker.number(&format!("{range}.end")),
                "{name}: `{range}` ends before it starts"
            );
        }
    }
    for row in 0..walker.number("mixins.count") {
        let row = format!("mixins.row[{row}]");
        for method in 0..walker.number(&format!("{row}.method_count")) {
            assert_eq!(
                body_kind(walker, &format!("{row}.method[{method}].code")),
                "Method",
                "{name}: `{row}`'s method {method}"
            );
        }
        for (field, kind) in [
            ("initializer", "Initializer"),
            ("own_initializer", "Initializer"),
            ("standalone_initializer", "Initializer"),
            ("superclass_clause", "SuperclassClause"),
        ] {
            if walker.number(&format!("{row}.has_{field}")) != 0 {
                assert_eq!(
                    body_kind(walker, &format!("{row}.{field}")),
                    kind,
                    "{name}: `{row}.{field}`"
                );
            }
        }
    }
    for object in 0..walker.number("graph.count") {
        let body = format!("graph.object[{object}].body");
        if text_at(walker, &format!("{body}.tag")) == "closure" {
            assert_eq!(
                body_kind(walker, &format!("{body}.code")),
                "Block",
                "{name}: `{body}`'s code"
            );
            assert_eq!(
                record_tag(walker, &format!("{body}.frame")),
                "frame",
                "{name}: `{body}`'s frame"
            );
        }
    }
}

/// That every frame record's references name what they say -- its sender
/// and home frame records, a closure frame's closure a closure record --
/// and that a frame has a home exactly when it runs a closure, as the
/// runtime builds them; answers how many frames have a sender, each a
/// top-level one, so that a sender and a home written in each other's
/// place fail.
fn check_frames(walker: &Walker, name: &str) -> usize {
    let mut senders = 0;
    for object in 0..walker.number("graph.count") {
        let body = format!("graph.object[{object}].body");
        if text_at(walker, &format!("{body}.tag")) != "frame" {
            continue;
        }
        let frame = format!("{body}.frame");
        let kind = text_at(walker, &format!("{frame}.kind.tag"));
        let has = |field: &str| walker.number(&format!("{frame}.has_{field}")) != 0;
        assert_eq!(
            has("home"),
            kind == "closure",
            "{name}: `{frame}`, a {kind} frame, has a home exactly when it runs a closure"
        );
        if has("sender") {
            assert_eq!(
                kind, "top-level",
                "{name}: only a staged frame has a sender"
            );
            assert_eq!(
                record_tag(walker, &format!("{frame}.sender")),
                "frame",
                "{name}"
            );
            senders += 1;
        }
        if has("home") {
            assert_eq!(
                record_tag(walker, &format!("{frame}.home")),
                "frame",
                "{name}"
            );
        }
        if kind == "closure" {
            assert_eq!(
                record_tag(walker, &format!("{frame}.kind.closure")),
                "closure",
                "{name}: `{frame}` runs a closure"
            );
        }
    }
    senders
}

/// The row of the declaration named `declaration`, if the image has one.
fn row_named(walker: &Walker, declaration: &str) -> Option<String> {
    (0..walker.number("mixins.count"))
        .map(|row| format!("mixins.row[{row}]"))
        .find(|row| {
            text_at(
                walker,
                &format!("symbols.symbol[{}]", walker.number(&format!("{row}.name"))),
            ) == declaration
        })
}

/// The order of a row's three initializer bodies, held by `EVERYTHING`'s
/// known rows: `Marker` has a full body and an own part and no standalone
/// body, its full body the longer, since it runs the prologue before the
/// own part; `Both`, which has `<:` clauses and a body, has a standalone
/// body. Answers whether the image has both rows.
fn check_initializer_order(walker: &Walker, name: &str) -> bool {
    let (Some(marker), Some(both)) = (row_named(walker, "Marker"), row_named(walker, "Both"))
    else {
        return false;
    };
    let present = |row: &str, field: &str| walker.number(&format!("{row}.has_{field}")) != 0;
    assert!(
        present(&marker, "initializer")
            && present(&marker, "own_initializer")
            && !present(&marker, "standalone_initializer"),
        "{name}: `Marker` has a full body and an own part, and no standalone body"
    );
    assert!(
        present(&both, "standalone_initializer"),
        "{name}: `Both` has a standalone body"
    );
    let instructions = |field: &str| {
        walker.number(&format!(
            "code.body[{}].instruction_count",
            walker.number(&format!("{marker}.{field}"))
        ))
    };
    assert!(
        instructions("initializer") > instructions("own_initializer"),
        "{name}: `Marker`'s full body is longer than its own part"
    );
    true
}

// spec: image.structure
// spec: image.encoding
// spec: image.sections
// spec: image.symbols
// spec: image.code-table
// spec: image.texts
// spec: image.mixins
// spec: image.modules
// spec: image.shipped-names
// spec: image.graph
// spec: image.graph.record
// spec: image.graph.value
// spec: image.hashes
#[test]
fn every_image_is_laid_out_as_the_book_gives() {
    let grammar = Grammar::read();
    let body = BodyGrammar::read();
    let samples = [
        packaged_application(true),
        packaged_application(false),
        everything(),
        level_over_a_shipped_class(),
    ];
    let mut reached = BTreeSet::new();
    let mut floats = Vec::new();
    let mut senders = 0;
    let mut initializers_pinned = false;
    for sample in &samples {
        let walker = Walker::walk(&sample.bytes, &grammar, &body);
        let name = sample.name;
        assert_eq!(walker.number("header.version"), 1, "{name}: the version");
        assert_eq!(
            walker.number("header.flags"),
            u64::from(sample.flags),
            "{name}: the flags"
        );
        for (index, expected) in sample.modules.iter().enumerate() {
            let module = format!("modules.module[{index}]");
            let origin = match walker.by_path.get(&format!("{module}.origin.tag")) {
                Some(Field::Text(origin)) => origin.clone(),
                _ => panic!("{name}: `{module}` has an origin"),
            };
            let (expected_origin, field) = match expected.path.as_deref() {
                None => ("none", None),
                Some(path) => match path.strip_prefix("core:") {
                    Some(file) => ("core", Some(("file", file))),
                    None => ("project", Some(("path", path))),
                },
            };
            assert_eq!(origin, expected_origin, "{name}: `{module}`'s origin");
            for origin_field in ["file", "path"] {
                assert_eq!(
                    walker
                        .by_path
                        .get(&format!("{module}.origin.{origin_field}")),
                    field
                        .filter(|(named, _)| *named == origin_field)
                        .map(|(_, path)| Field::Text(path.to_string()))
                        .as_ref(),
                    "{name}: `{module}`'s path"
                );
            }
            assert_eq!(
                walker.by_path.get(&format!("{module}.id")),
                Some(&Field::Text(expected.id.as_str().to_string())),
                "{name}: `{module}`'s id"
            );
            assert_eq!(
                walker.by_path.get(&format!("{module}.version.version")),
                expected
                    .version
                    .as_ref()
                    .map(|version| Field::Text(version.to_string()))
                    .as_ref(),
                "{name}: `{module}`'s version"
            );
        }
        // The symbols are numbered in the order the code table and then
        // the mixin table first name them, the first the code table names
        // first: the encoder writes each where it first meets it.
        let dense: Vec<u64> = (0..walker.symbol_order.len() as u64).collect();
        assert_eq!(
            walker.symbol_order, dense,
            "{name}: the symbols in the order first named"
        );
        for (_, field) in &walker.fields {
            if let Field::Float(float) = field {
                floats.push(*float);
            }
        }
        check_sections(&walker, name);
        check_ranges_and_references(&walker, name);
        senders += check_frames(&walker, name);
        initializers_pinned |= check_initializer_order(&walker, name);
        // The images the book is held to are images: each loads.
        if let Err(error) = crate::image::load(&sample.bytes) {
            panic!("{name} does not load: {error}");
        }
        reached.extend(walker.reached);
    }
    assert!(
        senders > 0,
        "a sample's frame has a sender, which pins the sender's place beside the home"
    );
    assert!(
        initializers_pinned,
        "a sample pins the order of a row's initializer bodies"
    );
    assert!(
        floats.contains(&1.0e300) && floats.contains(&1.5),
        "a float is its binary64 bits: {floats:?}"
    );
    let missed: Vec<&String> = grammar.rows.difference(&reached).collect();
    assert!(missed.is_empty(), "no sample reaches the rows {missed:?}");
    let unread: Vec<&String> = grammar.optional_items.difference(&reached).collect();
    assert!(
        unread.is_empty(),
        "no sample reads the repeated or optional items {unread:?}"
    );
}
