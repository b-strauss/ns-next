# File Structure (Document Symbols) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The JetBrains Structure view, File Structure popup and breadcrumbs show a Newspeak module's classes, factories, sides, slots, methods and method temporaries, from the language server's `textDocument/documentSymbol`.

**Architecture:** A follow-up on the branch `feature/editor-folding`, after `docs/superpowers/plans/2026-10-03-2341-editor-folding.md`. `crates/ns-lsp` gains `document_symbols.rs`, which walks the AST that `parse_for_editor` answers (recovered while a file is typed) into a symbol tree in byte spans, and encodes it as LSP `DocumentSymbol`s; the server announces `documentSymbolProvider` and answers the request from the cached analysis. The platform registers `LspStructureViewFactory` and `LspFileBreadcrumbsCollector` for every language (`lang.psiStructureViewFactory language=""`, in 2026.1.4's `intellij.platform.lsp.impl.jar` and 2026.2.3's `intellij.platform.lsp.impl.structureView.jar`), so the plugin needs no code — checked by hand under `runIde`.

**Tech Stack:** Rust 1.98, `lsp-types` 0.97, `lsp-server` 0.10, `cargo-nextest`.

**Spec:** decided with the user in the session of 2026-10-04 (no spec file):
- Under each class, in this order: its **factory**, then an **instance side** node, then a **class side** node.
- Within a side, members in **source order**.
- A method's **temporaries** (its body's `| |` slots) are its children.
- A slot shows whether it is **lazy** and whether it has a **getter and a setter or a getter only**, with IntelliJ's own icons and labels, as Java's structure view shows properties: `AllIcons.Nodes.PropertyRead` for a slot with a getter only (declared with `=`), `AllIcons.Nodes.PropertyReadWrite` for one with a getter and a setter (`::=` or bare), and the grey detail text `lazy` beside a lazy slot.

## Global Constraints

- The tree, per class: `Class <name>` → [`Constructor <factory pattern>`, `Namespace instance side` → members, `Namespace class side` → methods]. The class side node exists only when the class has `: ( … )`. The instance side node always exists.
- A class without a factory pattern shows the factory `new` (Newspeak's default primary factory), its range the class name.
- Members of the instance side: header slots, lazy slots, nested classes and methods, ordered by span start. Kinds: a slot with a getter only (`SlotMutability::Immutable`) `CONSTANT`, a slot with a getter and a setter (`MutableInitialized`, `MutableBare`) `PROPERTY`; a lazy slot has the `detail` `lazy`, every other symbol none; nested class `CLASS` (recursively the same tree), method `METHOD`. A method's children: its body's temporaries, kind `VARIABLE`, in order. Nothing else is a child (no parameters, no block temporaries). LSP has no "read-only property" kind, and `CONSTANT`/`PROPERTY` are the nearest for any LSP client; the plugin maps the two to IntelliJ's property icons (Task 2).
- Names: a class its name; a method or factory its pattern as written in canonical form — `foo`, `+ other`, `at: index put: value`; a slot or temporary its name; the sides `instance side` and `class side`. A root class whose name is missing (recovery) shows `(unnamed)`.
- Ranges: `range` is the node's span, `selectionRange` the name's span, always inside `range`; a side's `range` runs from its opening `(` to its closing `)` (or the side's span end when recovery found none) and its `selectionRange` is the opening `(`.
- The server answers an empty list for a document it never opened or has closed.
- Names are written out (`CLAUDE.md`); comments explain decisions; inline Newspeak in tests is `indoc!`.
- Per-commit gates, every command in the foreground:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets
  cargo nextest run --workspace --exclude ns-lsp
  cargo test --workspace --exclude ns-lsp --doc
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  cargo nextest run -p ns-lsp
  ./editors/jetbrains/gradlew -p editors/jetbrains check
  ```
- Commit messages: conventional commits ending with the `Co-Authored-By:` line naming the committing model and `Claude-Session: https://claude.ai/code/session_014iiGvJnqAeLJtzhwKA6Poa`.

## Review Focus

1. A file with syntax errors (a half-typed method) must keep the symbols of the members after the error (test in Task 1).
2. Every `selectionRange` must lie inside its `range`, also for a recovered node and for the default `new` factory; clients reject a symbol otherwise (test in Task 1, over every module of the corpus and every truncation).
3. Positions on `\r\n` lines with non-ASCII text before a symbol are UTF-16 (test in Task 1).
4. An empty file answers a root `(unnamed)` class with an empty instance side, no panic (test in Task 1).
5. Lazy slots declared among the methods appear at their place in source order (test in Task 1).

---

### Task 1: Document symbols in the server

**Files:**
- Create: `crates/ns-lsp/src/document_symbols.rs`
- Modify: `crates/ns-lsp/src/lib.rs`, `crates/ns-lsp/src/analysis.rs`, `crates/ns-lsp/src/server.rs`
- Modify: `crates/ns-lsp/tests/protocol.rs`, `crates/ns-lsp/tests/corpus.rs`

**Interfaces:**
- Consumes: `ns_syntax::parse_for_editor`, the AST (`ClassDeclaration`, `ClassBody { delimiters, slots, methods, nested_classes, span }`, `MethodDeclaration { pattern, body, span }`, `SlotDeclaration { name, span, lazy }`, `CodeBody { temporaries }`), `crate::documents::LineIndex::position`.
- Produces:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum SymbolCategory { Class, Factory, Side, ReadOnlySlot, ReadWriteSlot, Method, Temporary }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Symbol { pub name: String, pub category: SymbolCategory, pub lazy: bool, pub span: Span, pub selection: Span, pub children: Vec<Symbol> }
  pub fn symbols(module: &ast::Module) -> Vec<Symbol>      // one root: the module's class
  pub fn encode(symbols: &[Symbol], text: &str, line_index: &LineIndex) -> Vec<lsp_types::DocumentSymbol>
  Analysis.symbols: Vec<Symbol>
  ```

- [ ] **Step 1: Write the failing tests**

In `document_symbols.rs`, tests first. A helper renders the tree as indented lines, `<category> <name>`, two spaces per level, ` (lazy)` after a lazy slot. The code samples below predate the slot categories: write `ReadOnlySlot` / `ReadWriteSlot` instead of `Slot` in every expected outline (`| first second |` and `lazy cache = 3.` are `ReadWriteSlot first`, `ReadWriteSlot second`, `ReadOnlySlot cache (lazy)`), set `lazy` in `variable` from `slot.lazy` (always `false` for a temporary), choose the slot category from `slot.mutability`, and in `encode` map `ReadOnlySlot` to `SymbolKind::CONSTANT`, `ReadWriteSlot` to `SymbolKind::PROPERTY`, and `detail` to `Some("lazy")` for a lazy slot. Add one test, `a_slot_shows_its_accessors_and_whether_it_is_lazy`, over `| a = 1. b ::= 2. c |` in the header and `lazy d = 4.` and `lazy e ::= 5.` among the methods, asserting the five categories, the `lazy` flags and, after `encode`, the kinds and details:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn outline(text: &str) -> String {
        fn render(symbol: &Symbol, depth: usize, lines: &mut Vec<String>) {
            lines.push(format!("{}{:?} {}", "  ".repeat(depth), symbol.category, symbol.name));
            for child in &symbol.children {
                render(child, depth + 1, lines);
            }
        }
        let parsed = ns_syntax::parse_for_editor(text);
        let mut lines = Vec::new();
        for symbol in symbols(&parsed.module) {
            render(&symbol, 0, &mut lines);
        }
        lines.join("\n")
    }

    #[test]
    fn a_class_shows_its_factory_then_its_instance_side_then_its_class_side() {
        let text = indoc! {"
            class Module usingPlatform: platform = (
                | first second |
            ) (
                class Inner = () (
                    run = ( ^1 )
                )
                at: index put: value = (
                    | old |
                    ^value
                )
                lazy cache = 3.
                + other = ( ^other )
            ) : (
                make = ( ^self usingPlatform: nil )
            )
        "};
        assert_eq!(
            outline(text),
            indoc! {"
                Class Module
                  Factory usingPlatform: platform
                  Side instance side
                    Slot first
                    Slot second
                    Class Inner
                      Factory new
                      Side instance side
                        Method run
                    Method at: index put: value
                      Temporary old
                    Slot cache
                    Method + other
                  Side class side
                    Method make"}
        );
    }

    #[test]
    fn a_half_typed_method_keeps_the_symbols_after_it() {
        let text = indoc! {"
            class Module = () (
                broken = (
                    ^1
                second = ( ^2 )
            ) : (
                make = ( ^self new )
            )
        "};
        assert_eq!(
            outline(text),
            indoc! {"
                Class Module
                  Factory new
                  Side instance side
                    Method broken
                    Method second
                  Side class side
                    Method make"}
        );
    }

    #[test]
    fn an_empty_file_answers_an_unnamed_class() {
        assert_eq!(
            outline(""),
            "Class (unnamed)\n  Factory new\n  Side instance side"
        );
    }

    /// Every selection lies inside its range; a client refuses a symbol
    /// otherwise.
    fn assert_selections_inside(symbols: &[Symbol]) {
        for symbol in symbols {
            assert!(
                symbol.span.start <= symbol.selection.start && symbol.selection.end <= symbol.span.end,
                "{:?} {}: {:?} outside {:?}",
                symbol.category,
                symbol.name,
                symbol.selection,
                symbol.span
            );
            assert_selections_inside(&symbol.children);
        }
    }

    #[test]
    fn selections_lie_inside_their_ranges() {
        let text = indoc! {"
            class Module = () (
                run = ( | first | ^first )
                broken = (
            ) : ()
        "};
        assert_selections_inside(&symbols(&ns_syntax::parse_for_editor(text).module));
    }

    #[test]
    fn a_symbol_is_encoded_in_utf16_on_crlf_lines() {
        let text = "class Module = () (\r\n    (* Grüße 😀 *) run = ( ^1 )\r\n)\r\n";
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = crate::documents::LineIndex::new(text);
        let encoded = encode(&symbols(&parsed.module), text, &line_index);
        let side = &encoded[0].children.as_ref().expect("the class's children")[1];
        let method = &side.children.as_ref().expect("the side's members")[0];
        let line_start = text.find("    (* Grüße").expect("line 1");
        let column = text[line_start..text.find("run").expect("the method")].encode_utf16().count() as u32;
        assert_eq!(method.name, "run");
        assert_eq!((method.selection_range.start.line, method.selection_range.start.character), (1, column));
        assert_eq!(method.kind, lsp_types::SymbolKind::METHOD);
    }
}
```

In `tests/corpus.rs`, beside `no_truncated_module_declares_an_empty_name`, using its `cut_points`:

```rust
/// Every symbol's selection lies inside its range, on every module and every
/// cut of one: a client refuses the whole answer otherwise.
#[test]
fn every_symbol_selection_lies_inside_its_range() {
    fn check(symbols: &[ns_lsp::document_symbols::Symbol], context: &str) {
        for symbol in symbols {
            assert!(
                symbol.span.start <= symbol.selection.start
                    && symbol.selection.end <= symbol.span.end,
                "{context}: {} {:?} outside {:?}",
                symbol.name,
                symbol.selection,
                symbol.span
            );
            check(&symbol.children, context);
        }
    }
    for path in modules() {
        let text = read(&path);
        for end in cut_points(&path, &text, false) {
            let module = ns_syntax::parse_for_editor(&text[..end]).module;
            let context = format!("{} cut at byte {end}", path.display());
            check(&ns_lsp::document_symbols::symbols(&module), &context);
        }
    }
}
```

In `tests/protocol.rs`'s `an_editor_session`: assert `capabilities["capabilities"]["documentSymbolProvider"] == true`; after the folding request on the `folded` document, request `textDocument/documentSymbol` and assert the root's name is `Module` and its children's names are `["new", "instance side", "class side"]` — read the actual factory of `folded` (`Object new` is its superclass clause; the class has no factory pattern, so `new`); renumber later request ids.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p ns-lsp`
Expected: compile errors — `symbols`, `encode`, `Symbol` not found.

- [ ] **Step 3: Build the symbol tree**

Above the tests in `document_symbols.rs`:

```rust
//! The symbols of one document, for an editor's structure view and
//! breadcrumbs: each class with its factory, its instance side and its
//! class side, each side's members in source order, each method's
//! temporaries. They come from the AST `parse_for_editor` answers, so they
//! survive while a file is typed. See `docs/design/editor-support.md`,
//! "Structure".

use lsp_types::{DocumentSymbol, SymbolKind};
use ns_syntax::ast;
use ns_syntax::Span;

use crate::documents::LineIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolCategory {
    Class,
    Factory,
    Side,
    Slot,
    Method,
    Temporary,
}

/// One symbol, in byte offsets: `span` encloses it, `selection` is what an
/// editor selects when it is picked, always inside `span`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub category: SymbolCategory,
    pub span: Span,
    pub selection: Span,
    pub children: Vec<Symbol>,
}

/// The module's class and everything in it.
pub fn symbols(module: &ast::Module) -> Vec<Symbol> {
    vec![class(&module.root_class)]
}

fn class(declaration: &ast::ClassDeclaration) -> Symbol {
    let name = if declaration.name.name.is_empty() {
        "(unnamed)".to_string()
    } else {
        declaration.name.name.clone()
    };
    let mut children = vec![factory(declaration)];
    children.push(side(
        "instance side",
        &declaration.instance_side,
        instance_members(&declaration.instance_side),
        declaration.name.span,
    ));
    if let Some(class_side) = &declaration.class_side {
        children.push(side(
            "class side",
            class_side,
            class_side.methods.iter().map(method).collect(),
            declaration.name.span,
        ));
    }
    Symbol {
        name,
        category: SymbolCategory::Class,
        span: declaration.span.merge(declaration.name.span),
        selection: declaration.name.span,
        children,
    }
}

/// The primary factory: the pattern as written, or `new`, which a class
/// without one answers (§6), placed at the class's name.
fn factory(declaration: &ast::ClassDeclaration) -> Symbol {
    match &declaration.factory {
        Some(pattern) => {
            let span = pattern_span(pattern);
            Symbol {
                name: pattern_text(pattern),
                category: SymbolCategory::Factory,
                span,
                selection: span,
                children: Vec::new(),
            }
        }
        None => Symbol {
            name: "new".to_string(),
            category: SymbolCategory::Factory,
            span: declaration.name.span,
            selection: declaration.name.span,
            children: Vec::new(),
        },
    }
}

/// A side from its `(` to its `)`, or to where it ends when recovery found
/// no `)`; selected at its `(`, or at the class's name when it has none.
fn side(name: &str, body: &ast::ClassBody, members: Vec<Symbol>, class_name: Span) -> Symbol {
    let (span, selection) = match body.delimiters {
        Some(delimiters) => {
            let end = delimiters.close.map_or(body.span.end, |close| close.end);
            (
                Span::new(delimiters.open.start, end.max(delimiters.open.end)),
                delimiters.open,
            )
        }
        None => (body.span.merge(class_name), class_name),
    };
    Symbol {
        name: name.to_string(),
        category: SymbolCategory::Side,
        span,
        selection,
        children: members,
    }
}

/// Header slots, lazy slots, nested classes and methods, in source order.
fn instance_members(body: &ast::ClassBody) -> Vec<Symbol> {
    let mut members: Vec<Symbol> = body.slots.iter().map(|slot| variable(slot, SymbolCategory::Slot)).collect();
    members.extend(body.nested_classes.iter().map(class));
    members.extend(body.methods.iter().map(method));
    members.sort_by_key(|member| member.span.start);
    members
}

fn method(declaration: &ast::MethodDeclaration) -> Symbol {
    let selection = pattern_span(&declaration.pattern);
    Symbol {
        name: pattern_text(&declaration.pattern),
        category: SymbolCategory::Method,
        span: declaration.span.merge(selection),
        selection,
        children: declaration
            .body
            .temporaries
            .iter()
            .map(|temporary| variable(temporary, SymbolCategory::Temporary))
            .collect(),
    }
}

fn variable(slot: &ast::SlotDeclaration, category: SymbolCategory) -> Symbol {
    Symbol {
        name: slot.name.name.clone(),
        category,
        span: slot.span.merge(slot.name.span),
        selection: slot.name.span,
        children: Vec::new(),
    }
}

/// `foo`, `+ other`, `at: index put: value`.
fn pattern_text(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ast::MessagePattern::Binary { selector, parameter } => {
            format!("{} {}", selector.name, parameter.name)
        }
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, parameter)| format!("{}: {}", keyword.name, parameter.name))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// From the pattern's first name to its last.
fn pattern_span(pattern: &ast::MessagePattern) -> Span {
    match pattern {
        ast::MessagePattern::Unary { selector } => selector.span,
        ast::MessagePattern::Binary { selector, parameter } => selector.span.merge(parameter.span),
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, parameter)| keyword.span.merge(parameter.span))
            .reduce(Span::merge)
            .unwrap_or(Span::new(0, 0)),
    }
}

/// The LSP form, positions in UTF-16 units.
pub fn encode(symbols: &[Symbol], text: &str, line_index: &LineIndex) -> Vec<DocumentSymbol> {
    symbols
        .iter()
        .map(|symbol| {
            let range = |span: Span| {
                lsp_types::Range::new(
                    line_index.position(text, span.start),
                    line_index.position(text, span.end),
                )
            };
            #[allow(deprecated)] // `deprecated` is a required field of the struct, superseded by `tags`
            DocumentSymbol {
                name: symbol.name.clone(),
                detail: None,
                kind: match symbol.category {
                    SymbolCategory::Class => SymbolKind::CLASS,
                    SymbolCategory::Factory => SymbolKind::CONSTRUCTOR,
                    SymbolCategory::Side => SymbolKind::NAMESPACE,
                    SymbolCategory::Slot => SymbolKind::FIELD,
                    SymbolCategory::Method => SymbolKind::METHOD,
                    SymbolCategory::Temporary => SymbolKind::VARIABLE,
                },
                tags: None,
                deprecated: None,
                range: range(symbol.span),
                selection_range: range(symbol.selection),
                children: (!symbol.children.is_empty())
                    .then(|| encode(&symbol.children, text, line_index)),
            }
        })
        .collect()
}
```

Notes for the implementer: `Span::merge` exists (`ns_syntax::span`); check whether a keyword `MessagePattern` can be empty after recovery (Task 6 leaves a pattern missing a name out whole, so `parts` is non-empty; keep the `unwrap_or` as the total fallback). In the expected outline of the first test, check `lazy cache = 3.` against the parser: a lazy slot in the side body lands in `ClassBody.slots` (after the header slots) and so sorts by its span into source order — if the parser stores it differently, follow the code and say so.

- [ ] **Step 4: Wire it into the analysis and the server**

`Analysis` gets `pub symbols: Vec<document_symbols::Symbol>`, computed in `analyze_unguarded` from the resolved module (`document_symbols::symbols(&resolution.module)`); `guarded`'s panic branch answers `symbols: Vec::new()`. `server.rs`: `document_symbol_provider: Some(OneOf::Left(true))` in `capabilities()`, and `answer` dispatches `DocumentSymbolRequest::METHOD` to a `document_symbols_of` that mirrors `folding_ranges_of` (params `DocumentSymbolParams`; a document never opened answers an empty list; the response is `DocumentSymbolResponse::Nested(…)`). `lib.rs`: `pub mod document_symbols;` and the module doc names document symbols.

- [ ] **Step 5: Run the tests and the gates, commit**

Run: `cargo nextest run -p ns-lsp` — PASS. Every gate. Commit: `feat: the language server answers document symbols — factory, sides, slots, methods and temporaries — for the structure view`.

---

### Task 2: The slot icons in the plugin, checked by hand, and the documentation

**Files:**
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lsp/NewspeakSymbolKinds.kt`
- Create: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/lsp/NewspeakSymbolKindsTest.kt`
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lsp/NewspeakLspClientDescriptor.kt`
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml`, `editors/jetbrains/README.md`
- Modify: `docs/design/editor-support.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`

- [ ] **Step 0: Map the two slot kinds to IntelliJ's property icons.** `LspCustomization` has `symbolKindCustomizer: LspSymbolKindCustomizer` (`getIcon(SymbolKind): Icon?`, `getSupportedKinds()`), whose default answers `AllIcons.Nodes.Constant` for `Constant` and `AllIcons.Nodes.Property` for `Property` (confirmed with `javap` on the 2026.2.3 jars; confirm the same on 2026.1.4's). `NewspeakSymbolKinds`, a singleton subclass, answers `AllIcons.Nodes.PropertyRead` for `SymbolKind.Constant` (a slot with a getter only) and `AllIcons.Nodes.PropertyReadWrite` for `SymbolKind.Property` (a getter and a setter) — the icons Java's structure view shows for a read-only and a read-write property — and `super.getIcon(kind)` for every other kind; its KDoc says why the two LSP kinds carry Newspeak's slot accessors. `NewspeakLspClientDescriptor`'s `lspCustomization` overrides `symbolKindCustomizer` with it. `NewspeakSymbolKindsTest`, plain JUnit like `NewspeakSemanticTokensTest`, asserts the two mapped icons and that `Method` still answers the default. Run `./editors/jetbrains/gradlew -p editors/jetbrains check`. Commit: `feat: the plugin shows a slot's accessors with IntelliJ's property icons`.

- [ ] **Step 1: The user checks under `runIde`** (the controller starts it): the Structure tool window and the File Structure popup (Ctrl+F12) show the tree of the Global Constraints for `Kernel.ns`, slots with the read-only / read-write property icons and `lazy` beside a lazy slot; picking a symbol jumps to its name; breadcrumbs show the class and method at the caret; a half-typed method keeps the members after it in the tree. If nothing shows, the platform needs the feature enabled in `NewspeakLspClientDescriptor`'s `lspCustomization` (`documentSymbolCustomizer`, an `LspDocumentSymbolSupport`), confirmed against the jars with `javap`.
- [ ] **Step 2: Documentation.** `plugin.xml` description gains a `<li>` for the structure view and breadcrumbs, the change notes name it; the plugin README's first paragraph too. `editor-support.md`: Scope gains **Structure**, the protocol surface `textDocument/documentSymbol`, a new subsection **Structure** (the tree, the names, kinds, ranges, `new` for a class without a factory, the platform's registration and the outcome of Step 1), `document_symbols.rs` in the module list, Testing; "Out of scope" and "Planned later" no longer name the structure view (navigation keeps go to definition and find usages). `OPEN-WORK.md`'s Navigation item drops the structure view. `PROGRESS.md`: the milestone names the structure view. `CLAUDE.md`'s ns-lsp bullet: only with the user's approval, "(diagnostics, semantic tokens, fold ranges, document symbols)".
- [ ] **Step 3: Gates (Markdown and `plugin.xml`: the plugin's `check`), commit:** `docs: the structure view in the design, the plugin's description and the open work`.

---

After Task 2, the branch's end-of-batch gates (`finishing-a-batch`) run once, for this plan and `2026-10-03-2341-editor-folding.md` together.
