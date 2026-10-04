# Editor support — the language server and the JetBrains plugin

> **Status:** **Built:** the language server (`crates/ns-lsp/`, and
> `resolve_for_editor` in `crates/ns-runtime/src/resolver/mod.rs`) and the
> JetBrains plugin (`editors/jetbrains/`) both exist: highlighting, errors
> as you type, code folding and a structure view with breadcrumbs, in every
> commercial JetBrains IDE from 2026.1.4, kept while a file has syntax errors.
> Formatting, go to definition, find usages and further platforms are planned
> ("Planned later").

Without a plugin, editing `.ns` files in a JetBrains IDE means editing plain
text. This document describes the editor support that replaces it: syntax
highlighting as you type, the errors `ns-next` would report before running a
module, shown in the editor at the place they occur, code folding, and a
structure view of each file's classes and members.

## Scope

**In scope.** Four capabilities, in every commercial JetBrains IDE:

1. **Highlighting** — keywords, strings, symbols, numbers, comments,
   selectors, and, once the server has analysed the file, what the resolver
   knows about each name: parameter, local variable, slot, nested class,
   method, `self` / `super` / `outer`.
2. **Errors as you type** — every lexer, parser and resolver diagnostic, with
   the message `ns-next` itself produces.
3. **Folding** — comments, class bodies, class sides, method bodies, closures
   and slot sections collapse and expand, from fold ranges the server
   computes from the AST ("Folding").
4. **Structure** — the Structure tool window, the File Structure popup, the
   breadcrumbs and the sticky lines show a file's classes and their members,
   each with its access, from document symbols the server computes from the
   AST ("Structure").

**Target IDEs.** Every commercial JetBrains IDE that carries JetBrains' own
LSP API: IntelliJ IDEA, WebStorm, PhpStorm, PyCharm, DataSpell, RubyMine,
CLion, DataGrip, GoLand, Rider and RustRover. The free editions and Android
Studio do not carry that API and are not a target. The plugin is written so it
can be published to the JetBrains Marketplace at any time, even while it has
one user: complete metadata, a verifier-clean build, third-party notices, and
no machine-specific paths.

**Out of scope, for now.** Navigation (go to definition, find usages),
completion and formatting. Each is described under
"Planned later", with what it would need.

## Architecture

```
crates/ns-lsp/          binary `ns-next-lsp`: diagnostics, semantic tokens, fold
                        ranges and document symbols over LSP, and `dump-tokens`
                        for the lexer comparison
crates/ns-syntax/       + an editor entry, `parse_for_editor`: a module for
                        every source, recovered, with its comments' spans
crates/ns-runtime/      + a resolver entry answering bindings and errors together
editors/jetbrains/      Gradle project: file type, Kotlin lexer, highlighter,
                        LSP client, structure view, settings
```

Colour comes in two layers:

1. **The Kotlin lexer** colours every keystroke synchronously, with no server
   involved. It is always present, and it also carries the editor features
   the IntelliJ Platform hangs on lexer tokens: brace matching, quote
   handling, word boundaries, TODO markers in comments, spell checking
   confined to comments and strings.
2. **Semantic tokens from the server** arrive shortly after each change and
   are laid over the lexer's colours by the IDE. Where the two agree nothing
   visibly changes; where the server knows more — a name is a parameter, a
   slot, a nested class — its colour wins.

Errors travel separately, pushed by the server as `publishDiagnostics` after
every change. The IDE asks for fold ranges and document symbols, as it asks
for semantic tokens.

### Why a Kotlin lexer, when the rest avoids a second implementation

The repository keeps one implementation of anything two consumers must agree
on — one dispatch, one lowering checked against a reference — and the design
avoids a second Newspeak *parser* for that reason: the errors the editor shows
come from `ns-syntax` and the resolver, the same code `ns-next run` uses.

A second *lexer* is the deliberate exception. The IntelliJ Platform's editor is
built on a synchronous, incremental lexer; highlighting on open and on every
keystroke, and the editor features listed above, all read its tokens. The two
alternatives weighed were worse:

- **Semantic tokens alone**, no Kotlin lexer: one lexer, but a file is
  uncoloured until the server answers, uncoloured entirely with no server,
  and the lexer-driven editor features are missing.
- **The Rust lexer compiled to WebAssembly** and run in the JVM (for example
  on Chicory): one lexer, synchronous and platform-independent, but a new
  dependency and build step on an approach with no established precedent in
  plugins.

What keeps the second lexer honest is the comparison gate (see "Plugin
testing and gates"): Newspeak's lexical grammar is small and fixed by
the specification, and every file the repository holds is lexed by both and
compared token by token.

### Why a language server over JetBrains' native LSP API

- **A language server** keeps the analysis in Rust, in the crates that already
  do it, so the editor's diagnostics cannot drift from `ns-next`'s. The same
  server serves any other LSP editor at no extra cost, and navigation later
  extends it rather than replacing anything.
- **The native API** (`com.intellij.modules.lsp`) rather than Red Hat's LSP4IJ:
  the target is the commercial IDEs, all of which carry it, so a third-party
  plugin dependency buys nothing. LSP4IJ would have been the choice had the
  free editions been a target.
- A plugin that ran the CLI from an `ExternalAnnotator` was rejected: it needs
  the same bundled binary, but gives nothing to other editors and would have
  to be replaced by a server for navigation.

## The language server, `crates/ns-lsp`

A crate of its own with a binary of its own, `ns-next-lsp`, so that its
dependencies never reach the `ns-next` binary: `lsp-server` (rust-analyzer's
transport), `lsp-types` and `serde_json`, on top of `ns-syntax` and
`ns-runtime`. It never starts the VM; from `ns-runtime` it uses only the
resolver.

Modules:

- `main.rs` — with no argument it speaks LSP on stdin and stdout; with
  `dump-tokens <files…>` it prints each file's tokens as JSON, for the lexer
  comparison. `dump-tokens` prints UTF-16 offsets, the unit the plugin's
  documents use, and refuses a file longer than a span can address
  (`ns_syntax::source_too_long`) on stderr with a failing exit status, as it
  does an unreadable one.
- `server.rs` — the message loop over `lsp-server`, synchronous on one thread.
  Lexing, parsing and resolving one file takes milliseconds, so neither a
  debounce nor a worker pool buys anything. Open documents are keyed by the
  URI's string form, not by `lsp_types::Uri` itself: `Uri` wraps a
  `fluent_uri::Uri<String>` that caches parsed components behind a `Cell`, so
  clippy's `mutable_key_type` refuses it as a map key even though its
  `Hash`/`Eq` only ever read the string. A `semanticTokens/full`,
  `foldingRange` or `documentSymbol` request for a URI the server never opened, or has since
  closed, answers an empty set rather than an error.
- `documents.rs` — the open text per URI with its version, and a line index
  converting a `Span`'s byte offsets to LSP positions in UTF-16 code units,
  the one encoding every LSP client must accept.
- `analysis.rs` — run once per document version and cached:
  `ns_syntax::parse_for_editor`, then the resolver entry below on its module
  whether or not it was recovered, then the classification over the editor
  parse's tokens and the folds over its module and comment spans. Its
  findings are every diagnostic together -- the lexer's, then the parser's,
  then the resolver's -- so a lexer error does not hide a parser error after
  it.
- `semantic_tokens.rs` — classification and the LSP encoding.
- `folding.rs` — the fold ranges and their LSP encoding ("Folding").
- `document_symbols.rs` — the document symbols and their LSP encoding
  ("Structure").

**Protocol surface.** `initialize`, `textDocument/didOpen`, `didChange` (full
text on every change), `didClose`, `publishDiagnostics` (push, supported by
the platform since 2023.2), `textDocument/semanticTokens/full`,
`textDocument/foldingRange` (the `foldingRangeProvider` capability,
announced as `true`), `textDocument/documentSymbol` (the
`documentSymbolProvider` capability, announced as `true`), `shutdown` and
`exit`. Nothing else. A request the
server does not implement is answered `MethodNotFound`. A document
notification whose params do not match the schema has no response to carry
an error, so the server writes it to stderr and sends the client a
`window/logMessage` of type `Error` -- the client's
log for the server, not a `window/showMessage` popup, since the defect is the
client's, not the user's text -- and keeps serving the documents already open.

### The resolver entry

`resolve` answers either every binding or `Err(errors)`, although it collects
both. The server needs both at once, so that a file with one resolver error
still gets semantic tokens everywhere else. `ns_runtime::resolve_for_editor`
answers both together as an `EditorResolution` (module, bindings, errors);
`resolve` remains the entry everything that compiles uses.
`resolve_for_editor` runs on a recovered module as on any other: recovery
declares nothing the source does not name ([frontend.md](frontend.md),
"Recovery"), so it causes no duplicate or clashing member, and an unbound
name is a send in Newspeak, not an error. `resolve_for_editor` never records
`Binding::MirrorSend` — it goes through the same walk as `resolve`, never
through `resolve_kernel` — so the VM mirror's capability invariant
(`CLAUDE.md`, "The VM mirror is a capability") is not touched, and
`Kernel.ns`, opened in the editor, shows its `vmMirror` sends as ordinary
sends.

`Kernel.ns` resolves without errors this way: the `kernel` flag decides only
whether a send to `vmMirror` is marked (`resolver/mod.rs`, `resolve_send`),
never whether anything is an error — pinned by
`the_editor_entry_resolves_the_kernel_without_the_mirror_mark`.

### Classification

Legend: token types `parameter`, `variable` (a local), `property` (a slot),
`class`, `method`, `keyword`; modifiers `declaration` and `readonly` (a slot or
local declared with `=`). The reserved words (§4.1: `self`, `super`, `outer`,
`true`, `false`, `nil`), `class` and the access modifiers are coloured by the
plugin's lexer, which knows them from their text, so the server adds nothing
for them. `keyword` is for the one word the lexer cannot know: §6.3.3's
`lazy`, which both lexers answer as an identifier and only the parser
recognizes, by its position before or after an access modifier. The server
finds it among the tokens between a lazy slot's start and its name, where it
is the only identifier; a slot or method that is merely named `lazy` stays a
slot or a send. Until the server answers, `lazy` shows the neutral identifier
colour, never a wrong one.
A keyword message's span starts at its receiver, so the receiver's span is
excluded when the selector's tokens are found.

- **Declarations** come from the AST: class names, slots, message patterns,
  parameters, temporaries. A lazy slot is a slot like any other, `readonly`
  when declared with `=`; its initializer is walked without the factory's
  parameters in scope, as the resolver resolves it (§6).
- **Uses** come from the bindings. `Local` and `OuterLocal` are a parameter or
  a local according to the declaration they bind to. The resolver records a
  slot read as a `LexicalSend { depth }`, as the specification makes it a send
  like any other; the server looks the selector up among the declarations of
  the class `depth` levels out — slot, nested class or method — and colours it
  accordingly. The resolver finds that depth over both sides of a class
  together; the server looks only at the side the send reaches there. At
  depth 0 that is the instance side from an instance-side body and the class
  side (with the primary factory) from a class-side method; further out it is
  always an instance side, since a class side holds methods only and every
  nested class is declared on an instance side. A name declared as a slot on
  one side and a method on the other is so coloured by the declaration the
  send reaches. A
  name that class only inherits cannot be placed statically and
  is coloured `method`, which is what it is: a send. A write, `name:: value`,
  is coloured over the whole `name::` token, the colons with the name.
- **Messages to an object.** A send with an explicit receiver has no binding;
  its selector -- unary, binary or keyword -- is coloured `method`, in an
  expression, in a cascade and in a superclass or `<:` clause alike: `a + b` is
  a send like any other. A binary selector may be any of the tokens the parser
  takes as one after a receiver, `=`, `<`, `>`, `|` and `||` included, and a
  unary one may be `class` or an access modifier, which §4.1 does not reserve
  (`x public`): the lexer colours those words as keywords, and the server's
  colour replaces it where one is a name. The `N`
  of `outer N` is left alone: it names an enclosing class, not a message.
- **Selector parts.** The AST's `Selector` carries no span, so the keyword
  parts of a send are found among the lexer's tokens inside the message's span,
  outside its arguments' spans. `ns-syntax` does not change.
- **A nested class's superclass and `<:` clause receiver.** Every prefix form
  is classified: `self X` and `outer Y X` resolve through the enclosing chain
  and record a `LexicalSend` at the receiver's span, the same as a bare
  receiver, so `X` is looked up and coloured exactly as one; `super X` records
  a `SuperSend` and is coloured `method` directly, the same way a `super`
  send's own selector is, since the clause is a message to the class the
  applying declaration's own binding names, never a lookup through the
  enclosing chain.

A file with syntax errors is analysed as recovered ([frontend.md](frontend.md),
"Recovery"), so its colours, its resolver errors and its folds survive the
errors a file passes through while it is typed. A construct recovery cannot
place -- an unfinished body whose next line is a valid expression, which is
read as that expression -- folds worse there, never wrongly.

### Folding

`folding.rs` computes a document's fold ranges once per version, with the
analysis, from the AST and the comment spans `parse_for_editor` answers
beside it; `textDocument/foldingRange` answers them from the cache. A code
fold runs from the opening token to the closing one inclusive, read off the
AST's `Delimiters` ([frontend.md](frontend.md), "AST"), so a fold names what
it folds:

| Range | From — to | Collapsed text | `kind` |
|---|---|---|---|
| Class body (instance side) | the header's `(` to the instance side's last `)`, header and members together | `(…)` | none |
| Class side | the `(` after `:` to its `)` | `(…)` | none |
| Method body | `(` to `)` | `(…)` | none |
| Closure | `[` to `]` | `[…]`, or `[:x :y \| …]` with its parameters | none |
| Slot section -- a class header's, or a body's temporaries | `\|` to `\|`, or `\|\|` to `\|\|` | `\|…\|` | none |
| Comment | `(*` to `*)`, an unterminated one to the end of the file | `(* `, the first non-blank line cut to 60 characters, ` …*)`; `(*…*)` for a blank one | `comment` |

- **Only a range over more than one line** is answered: a fold on one line
  hides nothing.
- **A construct recovery closed without its closing token** folds to where
  its node ends, the last token recovery gave it; a slot section without its
  closing bar, to its last slot.
- **A code fold carries no kind.** LSP's kinds are `comment`, `imports` and
  `region`, and `region` names `#region` markers, which Newspeak has not.
  The server collapses nothing: whether a range starts collapsed is the
  platform's decision, from the kind and the IDE's folding settings.
- A parenthesized expression and a tuple do not fold.
- Positions are a line and a UTF-16 character at each end, through the line
  index; the ranges are ordered by start, an enclosing one before those
  inside it.

**The plugin needs no code for it.** The platform registers its
`LspFoldingBuilder` for every language (`lang.foldingBuilder language=""` in
`intellij.platform.lsp.impl.xml`), and `LspCustomization`'s default
`foldingRangeCustomizer` is `LspFoldingRangeSupport`, in 2026.1.4 as in
2026.2.3, so the server's ranges reach a `.ns` file as they are. Checked by
hand under `runIde` (2026-10-04): fold markers on comments, class bodies,
class sides, methods, closures and slot sections; a folded method shows
`(…)`; and typing an unclosed `foo = (` inside a class keeps the folds of
the methods after it. Were the platform ever to need folding enabled for a
language with a parser definition of its own, as semantic tokens do
(`shouldAskServerForSemanticTokens`, below), it would go into
`NewspeakLspClientDescriptor`'s `lspCustomization`.

### Structure

`document_symbols.rs` computes a document's symbols once per version, with
the analysis, from the module the editor parse recovered;
`textDocument/documentSymbol` answers them from the cache, in the nested form
(`DocumentSymbol`), never as flat `SymbolInformation`. The answer is one root,
the module's class, and it is never empty for a document the server has
opened, however broken its text; a document it never opened, or has closed,
is answered `[]`.

**The tree.**

- A class has three kinds of child, in this order: its **factory**, its
  **instance side** and, when it declares one, its **class side**. A side is
  named `instance side` or `class side`.
- The instance side holds the header's slots, the lazy slots, the nested
  classes (each a class with the same children) and the methods, in source
  order. The class side holds its methods in source order.
- A method has its temporaries as children; nothing else but a class and a
  side has children.
- The factory is named by its pattern as written (`usingPlatform: platform`);
  a class without one is answered `new`, which §6 gives it, placed at the
  class's name. A pattern is written `foo`, `+ other` or
  `at: index put: value`.
- A nameless root class, which recovery keeps ([frontend.md](frontend.md),
  "Recovery"), is named `(unnamed)`. Recovery leaves any other declaration
  without a name out, so no other symbol has an empty one.

**Kinds.** LSP has no kind for a read-only property, so a slot with a getter
only (declared with `=`) is a `Constant` and a slot with a getter and a setter
(`::=`, or a bare one) a `Property`, the nearest two kinds any client knows.
A class is a `Class`, a factory a `Constructor`, an instance or class side a
`Namespace` (a named group of members, not a package), a method a `Method`
and a temporary a `Variable`.

**`detail`** is the effective access word, then ` lazy` for a lazy slot
(`protected lazy`); a side and a temporary have neither, so no `detail`.
The access is what the member declares, else protected (§6.3.1: "If no access
modifier is declared, the default is protected"); the root class is public
(§6.3.1: the access level of a top-level class is always public); a factory is
public whatever the default says, because the runtime enters every primary
factory as public (`ns_runtime::mixin::factory_entry`).

**Ranges.** A class's `range` runs from its declaration's start to its end,
and its `selectionRange` is its name. A method's range is its declaration;
its selection is its pattern, from the first name to the last. A slot or
temporary is selected at its name. A side runs from its `(` to its `)` (or to
where recovery ended it) and is selected at its `(`, or at the class's name
when recovery found no brackets. A factory's range and selection are its
pattern, or the class's name for `new`. Every selection lies inside its
range, which a client requires of the whole answer; positions are UTF-16
through the line index.

**Why the plugin has a structure view of its own.** The platform's structure
view for a language server (`LspStructureViewFactory`, registered for every
language) takes its icons per `SymbolKind` only, so it cannot show a member's
access. The plugin registers `NewspeakStructureViewFactory`
(`lang.psiStructureViewFactory` for `Newspeak`), built over the public LSP
API alone: it finds the client with
`LspClientManager.getInstance(project).getClients(NewspeakLspIntegrationProvider::class.java)`
and asks `textDocument/documentSymbol` through `sendRequestSync`. The
platform's own structure view is switched off for the client by an
`LspDocumentSymbolSupport` subclass with `structureViewSupport = false`
(`NewspeakDocumentSymbols`, in `NewspeakLspCustomization`); its breadcrumbs
stay on, and with them the sticky lines, which the platform draws from the
same symbols (`LspFileBreadcrumbsCollector.computeStickyLineInfos`).

**The rows.** A row's text is the symbol's name, with `lazy` beside a lazy
slot. Its icon is the kind's icon beside the access icon: `C_public`,
`C_protected` or `C_private` for the first word of `detail`, joined to the
kind's with `IconManager.createRowIcon` as Java's structure view joins them,
so the "show visibility icons" setting behaves as for Java. A `detail` whose
first word is no access word, and a side or temporary, shows the kind's icon
alone. `NewspeakSymbolKinds`, the client's `symbolKindCustomizer`, gives the
kinds their icons, for the breadcrumbs as for the tree: `Constant` is
`PropertyRead` and `Property` is `PropertyReadWrite` (a slot is always reached
through its accessors, which those icons say), `Namespace` is `CustomRegion`
(the icon the platform gives a group of members, not the package's), and every
other kind keeps the platform's. A pick moves the caret to the start of the
symbol's `selectionRange`, in the document's UTF-16 columns; the view selects
the innermost symbol whose range holds the caret.

**When it asks.** The model asks the server for the file's children, off the
EDT, and again only once the document's modification stamp has changed; asked
on the EDT it answers what it already has. A usable answer is a non-empty one:
the server always answers the root class for a document it has opened, so no
client, no answer in time or an empty answer means the file has not reached
the server yet (a file opened with the tool window already showing, or an
editor restored at IDE start). The platform refreshes its own view when a
client starts through a listener that is internal API, which the plugin may
not use, so while no usable answer exists the view asks again: every second
for 60 tries, then every ten seconds for as long as it lives, and fast again
after an answer; a new model (another file, the view reopened) starts fast. A
server that starts late, or is restarted, therefore fills the tree without an
edit.

**Known behaviours.** After a timeout following an edit the tree keeps its
last answer, and its positions are stale until the next answer. Two views of
one file asking at once (the tool window and the popup) each send a request.

**Checked by hand under `runIde` (2026-10-04)** on `Kernel.ns`: the Structure
tool window and the File Structure popup show the tree, with the factory, the
instance side, the class side, the members in source order and the
temporaries under their methods; Java-like access icons (a member without a
modifier shows protected), the read-only and read-write property icons and
`lazy`; picking a symbol jumps to its name; breadcrumbs show the class and
method at the caret, and IntelliJ's sticky lines follow; the tree survives a
half-typed method; and it fills on first open.

### Robustness

An editor sends invalid text all the time. `ns-syntax`'s contract is that
malformed source produces diagnostics, not a panic; the server nonetheless runs
each analysis under `catch_unwind`, turns a panic into one "internal error"
diagnostic at the start of the file, and keeps running. A panic there is an
ns-next defect and must be visible, but must not take the editor's server down
with it. The release profile unwinds (`Cargo.toml` deliberately does not set
`panic = "abort"`), which this relies on.

### Testing

`cargo nextest run -p ns-lsp`:

- positions — byte offset to UTF-16 with non-ASCII text, umlauts and emoji
  (surrogate pairs), at line ends and at the end of the file;
- diagnostics — one `indoc!` module each with a lexer, a parser and a resolver
  error, checking range and message; a lexer error's findings come first, the
  parser's after them, and a syntax error keeps the names around it
  classified;
- folding — a class body and a class side, a method with its temporaries and
  a closure, a comment folded to its first line, an unterminated comment to
  the end of the file, single-line constructs answering no range, the folds
  after a half-typed method, and a fold's UTF-16 encoding on CRLF lines;
- document symbols — the tree of a class with a factory, a nested class,
  slots, methods and a class side, in source order; a half-typed method
  keeping the symbols after it; an empty file answering an unnamed class;
  every selection inside its range, also over every module of the corpus and
  every cut of one (`every_symbol_selection_lies_inside_its_range`); a
  symbol's UTF-16 encoding on CRLF lines; a slot's kind and `lazy`; and a
  member's effective access;
- semantic tokens — a small module per category (parameter, local, slot with
  and without `readonly`, nested class, declared method, inherited send,
  a receiver inside a keyword message's span, a lazy slot in both
  modifier orders and both forms, a name a slot on one side of a class and a
  method on the other, read from either side, from a nested class and from a
  superclass clause, and the primary factory as a class-side member only),
  each expected category asserted
  directly rather than through an `insta` snapshot, so every expectation is
  stated and reviewed rather than accepted;
- the resolver entry — for every module in `corpus/modules`, `goldens/` and
  `Kernel.ns`, the same bindings and errors as `resolve`, so the two cannot
  drift;
- the editor parse — over that same set, every module `parse` accepts gets
  the same module from `parse_for_editor`, and no diagnostic, so the editor
  never reads a clean file differently from `ns-next run`;
- robustness — no analysis over that same set produces an "internal error",
  through the parse, the resolver, the classification and the folding alike,
  nor over truncations of each file, which is what a file looks like while it
  is being typed: at every token boundary of every golden and of the kernel
  and in the middle of each of their tokens -- a string, a comment or a
  keyword half typed -- and at the end and in the middle of 64 evenly spaced
  tokens of each corpus module, since every prefix of every corpus module is
  quadratic in its size;
- no empty name — on the token-boundary truncations, no declaration in the
  recovered module has an empty name but the root class's, which recovery
  keeps ([frontend.md](frontend.md), "Recovery");
- protocol — one end-to-end test over an in-memory `lsp-server` connection:
  `initialize` and its announced capabilities, `foldingRangeProvider`
  and `documentSymbolProvider` included, `didOpen`, the expected `publishDiagnostics`,
  `semanticTokens/full`, `textDocument/foldingRange`, `textDocument/documentSymbol` (the class, its
  three children, and `[]` for a document never opened), `shutdown`; a malformed
  notification, reported as a `window/logMessage` error while the next
  request is still answered; an unknown request, answered `MethodNotFound`.

## The plugin, `editors/jetbrains/`

**Identity.** Plugin ID `io.github.bstrauss.newspeak` — permanent once
published — name `Newspeak (ns-next)`, vendor Benjamin Strauß, MIT licence like
the repository. The name says whose parser and messages these are rather than
suggesting the Newspeak project's own plugin.

**Platform version.** `sinceBuild` `261.26222` (IntelliJ IDEA 2026.1.4 is
build 261.26222.65), `untilBuild` open. JetBrains
reworked the LSP API in 2026.1.4 — module `com.intellij.modules.lsp`, extension
point `com.intellij.platform.lsp.integrationProvider`, classes
`LspIntegrationProvider` (with its nested `LspIntegrationProvider.LspClientStarter`,
handed to `fileOpened` to start the client) and `ProjectWideLspClientDescriptor`,
replacing `modules.ultimate`, `serverSupportProvider` and `LspServerSupportProvider`.
The client descriptor's `lspCustomization` (`LspCustomization.semanticTokensCustomizer`)
is where the server's semantic tokens are wired in, over `LspSemanticTokensSupport`,
whose `shouldAskServerForSemanticTokens` the plugin overrides to answer `true`: the
platform's default asks only for plain-text and TextMate files, so a `.ns` file,
which has a language of its own, would never be asked for at all;
restarting the server, after the settings path changes or by hand, goes through
`LspClientManager.getInstance(project).stopAndRestartClientsIfNeeded(...)`. Every
one of these names was confirmed against the actual 2026.2.3 platform jars while
building this plugin (Task 5) — the platform's own documentation and samples lag
a release behind the API itself. Targeting the new API alone keeps one code path
with no deprecated calls for the verifier to report; older IDE versions are the
price.

Components:

- **Language and file type** — `NewspeakLanguage`, `NewspeakFileType` for
  `.ns`, an SVG icon with a light and a dark variant.
- **Lexer** — hand-written Kotlin, a `LexerBase` that mirrors the Rust lexer
  function by function, so the two can be reviewed side by side and no
  generator step exists (JFlex was considered and rejected for that reason).
  Every comment — nested `(* … *)` included — and every string is a single
  token, so the lexer restarts in state 0 at any token boundary, which the
  platform's incremental lexing needs. Unlike the Rust lexer it also emits
  whitespace and comment tokens, since the platform requires every character
  to belong to a token.
- **A flat `ParserDefinition`** — every token under a single file node, no
  grammar. It is not a second parser; it exists so the platform knows which
  tokens are comments and strings, for TODO markers and spell checking.
- **Highlighting** — a `SyntaxHighlighter` and `TextAttributesKey`s derived
  from `DefaultLanguageHighlighterColors`, so every colour scheme works as is,
  and a `ColorSettingsPage` with sample code covering the semantic-token
  categories too: the server's token types are mapped onto the plugin's own
  keys, so each one can be configured. A send and a keyword selector derive
  from `INSTANCE_METHOD`, not `FUNCTION_CALL`: the platform's schemes leave a
  function call in the plain text colour but give an instance method its own
  blue, which is what a message to an object is. A setter token, `name::`,
  has a key of its own over `IDENTIFIER`: the lexer cannot tell a slot's or a
  local's write from a setter send, so it leaves the token neutral for the
  semantic colour to replace instead of showing it as a send first. A binary
  selector token derives from `INSTANCE_METHOD` too, since it is always a send
  or a method's own selector; `<` and `>`, which also delimit type annotations
  and pattern literals, have a neutral key of their own and are coloured by the
  server where they are sends. A parameter derives from
  `PARAMETER` and a slot from `INSTANCE_FIELD`, as in TypeScript.
- **Editor features** — a `BraceMatcher` for `()`, `[]` and `{}`, a
  `Commenter` for `(* *)`, a `QuoteHandler`. Not for `<>`: `<` and `>` are
  also binary selectors, and pairing a comparison with a type's closing
  bracket would be worse than pairing nothing. TODO markers need only a
  `NewspeakIndexPatternBuilder` (the `indexPatternBuilder` extension point,
  pointed at the lexer's own comment tokens) — no dedicated `todoIndexer` was
  needed. Spell checking is wired the same shallow way, but as an *optional*
  dependency: `plugin.xml` declares
  `<depends optional="true" config-file="newspeak-spellchecker.xml">com.intellij.modules.spellchecker</depends>`,
  and `newspeak-spellchecker.xml` registers a `spellchecker.support` for
  `Newspeak` there, in a file the platform loads only when that module is
  present, rather than in the main descriptor — so an IDE built without the
  spellchecker module still loads this plugin, with no spell checking in
  Newspeak comments and strings rather than a failure to start. The build
  still needs the module's classes to compile and test against, which
  `bundledModule("intellij.spellchecker")` supplies, since the module has no
  plugin descriptor of its own to depend on at compile time.
- **LSP client** — an `LspIntegrationProvider` for `.ns` files, one server per
  project. Folding needs nothing of its own: the platform's `LspFoldingBuilder`
  asks the server ("Folding").
- **Structure view** — `NewspeakStructureViewFactory`, its model and
  `NewspeakStructureElement`, over the server's document symbols, with
  `NewspeakSymbolKinds` and `NewspeakDocumentSymbols` in the client
  descriptor's customization ("Structure").
- **Settings** — *Languages & Frameworks → Newspeak*: a server path, empty by
  default, typed or chosen with a file picker, and a "Restart server" action.
  Stored at application level: the path is specific to a machine, so it does
  not belong in `.idea/`.

### Which binary runs

1. The path from the settings, when set — for working on `ns-syntax` or the
   resolver and seeing the change in the editor at once, pointed at
   `target/debug/ns-next-lsp.exe`.
2. Otherwise `<plugin directory>/bin/<os>-<arch>/ns-next-lsp[.exe]`, beside the
   plugin's jars rather than inside one, so it can be started without
   extracting it first. On Unix the executable bit is set on first start,
   because the zip format does not carry it.
3. With neither, one balloon notification linking to the setting. Highlighting
   and the editor features work regardless.

The plugin bundles the binary, built from the same commit as the Kotlin lexer,
so the two always match. The directory under `bin/` is decided by one mapping,
`platform-directories.properties` beside `ServerBinaryLocator`: the system by
what `os.name` starts with, the architecture by `os.arch`. The locator finds
the server through it at run time, and the build reads the same file to place
the server it builds for its own host. Only that host's server is bundled so
far, so `plugin.xml`'s description names no platform; the layout is per
platform so that the other five of `windows-x64`, `windows-arm64`,
`macos-x64`, `macos-arm64`, `linux-x64` and `linux-arm64` are added later by a
CI build placing their binaries in the same tree, with no change to the
plugin's code.

## Build and packaging

- **Gradle project** under `editors/jetbrains/`, with the wrapper committed,
  Kotlin, and the IntelliJ Platform Gradle Plugin 2.x, built against IntelliJ
  IDEA 2026.2.x. Bytecode targets Java 21, the floor this plugin supports
  (2026.1.x's bundled JBR) rather than 2026.2.x's JBR 25 — see
  "`verifyPluginProjectConfiguration`" below for why that is the right target
  even while compiling against the newer platform. Gradle 9.8 runs on any JDK
  from 17 to 26; the JDK 21 the build compiles with is a toolchain the foojay
  resolver provisions, so the build does not depend on which JDK happens to
  run Gradle.
- **Rust from Gradle.** `buildLanguageServerDebug` and `buildLanguageServerRelease`
  each run `cargo build -p ns-lsp` at the repository root, debug and `--release`
  respectively; `tasks.test` depends on the debug build and points the tests at
  it. Every sandbox an IDE starts from depends on the release build and gets
  the binary in `bin/<os>-<arch>/` for the host it runs on, the directory
  `platform-directories.properties` names for it (a host the file does not
  name fails the sandbox task rather than bundling a server the plugin would
  not look for), beside `THIRD-PARTY-NOTICES.txt` and
  `LICENSE`: `prepareSandbox`, which `buildPlugin` packages, and
  `prepareSandbox_runIde` with its split-mode siblings, which `runIde` starts
  from — a separate sandbox (`plugins_runIde/`), so configuring
  `prepareSandbox` alone leaves `runIde` without a server. The test sandboxes
  (`prepareTest…`) are left out: only these build `--release`, because the
  release profile's fat LTO and single codegen unit make that build slow and
  the tests use the debug build.
- `runIde` starts a sandbox IDE with the plugin, for trying it by hand. It has
  been tried end to end: the server starts from `plugins_runIde/`, the
  semantic colours land over the lexer's, a resolver error
  (`^outer Missing foo`) is underlined with ns-next's message, and the folds
  are there, through a syntax error too ("Folding").
- `editors/jetbrains/build/` and `.gradle/` are ignored.

**Marketplace readiness from the first commit.** `plugin.xml` carries the ID,
name, vendor, description, change notes, `sinceBuild` and the dependency on
`com.intellij.modules.lsp`. The bundled binary contains third-party crates —
those `cargo tree -p ns-lsp -e normal,no-proc-macro` shows compiled into
`ns-next-lsp` itself, excluding build-time-only proc macros — whose licence
texts go into a hand-maintained `THIRD-PARTY-NOTICES` in the plugin; the list
is short enough that a generator such as `cargo-about` would be one more tool
for no gain. `verifyPlugin` checks the lowest supported and the current
release of four commercial IDEs — IntelliJ IDEA 2026.1.4 and 2026.2.3,
RustRover 2026.1.4 and 2026.2.3, PyCharm 2026.1.5, CLion 2026.2.3 (the exact
versions `printProductsReleases` printed for each product, constrained to
build 261.26222 or later) — all `Compatible`, with no deprecated or internal
API usage. Reaching that took one fix: `NewspeakLspIntegrationProvider`
originally read its own installation directory with
`PluginManagerCore.getPlugin(PluginId.getId(PLUGIN_ID))?.pluginPath`, and
every overload of that lookup — on `PluginManagerCore`, on `PluginManager`
and on `PluginAwareClassLoader.getPluginDescriptor` — is `@ApiStatus.Internal`
in this platform version; `verifyPlugin` fails on `INTERNAL_API_USAGES` by
default, not just warns. The fix implements `com.intellij.openapi.extensions.PluginAware`
instead: `XmlExtensionAdapter` calls `setPluginDescriptor` right after
constructing an extension instance that implements it, with no lookup on the
plugin's own part. `signPlugin` and `publishPlugin` are configured but
have no certificate or token; those come from environment variables when
publishing, never from the repository.

**`verifyPluginProjectConfiguration`** is not part of `check` on this
platform version and is run on its own; it reports two findings, both the
same decision seen from different angles, and both stand as configured:

- *since-build's major version (261) is lower than the target platform's
  (262).* Deliberate: `sinceBuild` `261.26222` is IntelliJ IDEA 2026.1.4, the
  floor this plugin supports, chosen because that release carries the LSP API
  this plugin depends on exclusively; the project compiles and tests against
  the current 2026.2.x release (`platformVersion`) because that is what a
  developer working on the plugin today has installed, not because 2026.1.x
  support is an oversight. Raising `sinceBuild` to silence this would narrow
  the supported range for no gain.
- *Java sourceCompatibility (21) is lower than the target platform's JBR
  (25).* Also deliberate, and the reason is the same decision from the other
  side: IntelliJ IDEA 2026.1.x still bundles JBR 21, so bytecode compiled for
  Java 25 would not load on the lowest IDE this plugin supports at all — an
  older JVM cannot run newer class files, where a newer JVM runs older ones
  without issue. `verifyPlugin`'s own report is the evidence that Java 21
  bytecode is fine on every targeted IDE, 2026.2.x's JBR 25 included: all six
  verifications above, the 2026.1.x ones among them, came back `Compatible`.
  Raising `sourceCompatibility` to match the tool's advice would break the
  IDEs this plugin's `sinceBuild` promises to support.

## Plugin testing and gates

The server's own tests are described under its section, above ("Testing").

**Kotlin, `./gradlew -p editors/jetbrains check`:**

- the lexer comparison (`LexerComparisonTest`) — `ns-next-lsp dump-tokens`
  against the Kotlin lexer over `corpus/modules`, `goldens/` and `Kernel.ns`:
  kind and span of every token must agree, and the Kotlin lexer's whitespace
  and comment tokens must fill exactly the gaps between the Rust tokens.
  `dump-tokens` runs in batches of 100 files, which keeps each command line
  under Windows' length limit over the repository's close to 400 `.ns` files.
  The test task declares the debug server, `corpus/modules`, `goldens/` and
  `Kernel.ns` as inputs, so a change to any of them — the Rust lexer reaches
  it through the rebuilt server — reruns the comparison rather than leaving
  `check` up to date;
- restart (`LexerRestartTest`) — lexing from any token boundary answers the
  same tokens, the platform test framework's standard check;
- plugin tests on `BasePlatformTestCase` (`NewspeakEditorTest`) — file type,
  brace matching, commenting; `NewspeakColorSettingsPageTest` is plain JUnit,
  over the colour settings page directly, with no platform test case needed
  either — like `ServerBinaryLocatorTest` and `NewspeakSemanticTokensTest`,
  which cover the binary lookup order and the semantic-token classification;
- the structure view, plain JUnit: `NewspeakSymbolKindsTest` (the icon per
  kind), `NewspeakStructureElementTest` (a row's name, `lazy`, kind icon and
  access icon from a `DocumentSymbol`), `NewspeakStructureViewModelTest` (no
  answer and an empty answer make the model ask again, an answer with symbols
  is shown) and `NewspeakStructureViewFactoryTest`.

**Gates.** The runtime's per-commit suite is `cargo nextest run --workspace
--exclude ns-lsp`, so runtime work does not pay for the editor's corpus-wide
tests, with `cargo nextest run -p ns-lsp` its own per-commit gate for a change that
touches `crates/ns-lsp/` or `crates/ns-syntax/`; `cargo clippy --workspace
--all-targets` still compiles `ns-lsp` on every commit, so a resolver or
`ns-syntax` change that breaks its API fails at the commit even when its tests
only run at the end of the batch (`CLAUDE.md`, "Working in this repository").
The stress-mode runs exclude `ns-lsp` too: it never starts the VM. The
plugin's own gate joins this:

- **Before every commit** that touches `editors/jetbrains/`, `crates/ns-lsp/`
  or `crates/ns-syntax/` — `./gradlew -p editors/jetbrains check`, besides the
  usual `fmt` and `clippy`. A change to `crates/ns-syntax/src/lexer.rs` must be
  mirrored in `NewspeakLexer.kt` in the same commit, or the comparison fails.
- **Once at the end of a batch**, whether runtime or editor work, both test
  groups; and, for a batch that touched `editors/jetbrains/`,
  `./gradlew -p editors/jetbrains verifyPlugin buildPlugin` — which
  downloads several IDEs, and so is not a per-commit gate. A resolver
  change that shifts the semantic tokens is then noticed within one batch.

## Known gaps

Reviewed and deliberately left for later; none affects a module that parses
and resolves cleanly in a JetBrains IDE today.

- **The IDE path has no automated test.** It was checked by hand under
  `runIde` (see "Build and packaging"); nothing but that check covers the
  platform handing `NewspeakLspIntegrationProvider` its descriptor
  (`PluginAware`), the sandbox layout `runIde` starts from, or the
  platform's `LspFoldingBuilder` reaching a `.ns` file. The sandbox
  also disables the ultimate marker plugin for `runIde`, not only for tests.
- **Untested paths:** `dump-tokens` on an unreadable file (exit status 1); a
  block comment over a whole-line selection; TODO markers through the
  project-wide TODO index rather than a single open file.

## Planned later

- **Formatting.** The native LSP API supports `textDocument/formatting` since
  2023.3; once the server announces it, the IDE's Reformat Code uses it with no
  change to the plugin. A platform-native formatter (`FormattingModelBuilder`)
  would need a PSI tree, which is the second parser this design avoids. The
  work is in Rust. A formatter must keep each comment where it was; the
  comments' spans exist (`parse_for_editor`'s `comments`, beside the AST),
  and what remains is attaching each to the nodes around it, and the
  formatter itself: one module that an `ns-next fmt` command shares, so the
  editor, the command line and any later formatting gate use one formatter.
  It would not touch a file with a syntax error.
- **Navigation.** Go to definition and find usages need an AST for a file
  that does not parse, while it is typed; the fault-tolerant parse exists
  ([frontend.md](frontend.md), "Recovery"). What remains is `definition` and
  `references` in the server.
- **More platforms.** A CI build producing the other five binaries into
  `bin/<os>-<arch>/`. The repository has no CI yet.
