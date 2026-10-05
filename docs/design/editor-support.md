# Editor support — the language server and the JetBrains plugin

> **Status:** **Built:** the language server (`crates/ns-analyzer/`: `analysis.rs`,
> `semantic_tokens.rs`, `folding.rs`, `document_symbols.rs`; on
> `ns_syntax::parse_for_editor` and `resolve_for_editor` in
> `crates/ns-runtime/src/resolver/mod.rs`) and the JetBrains plugin
> (`editors/jetbrains/`, its structure view in the `structure` package and
> `lsp/NewspeakSymbolKinds.kt`) both exist: highlighting, errors as you
> type, code folding and a structure view with breadcrumbs, in every
> commercial JetBrains IDE from 2026.1.4, kept while a file has syntax
> errors. For a project's `newspeak.json`, completion and validation from
> the project file's schema, and play buttons that run or benchmark the
> project with the `ns-next` the settings name ("The project file and
> running projects"). Formatting, go to definition, find usages and further
> platforms are planned ("Planned later").

Without a plugin, editing `.ns` files in a JetBrains IDE means editing plain
text. This document describes the editor support that replaces it: syntax
highlighting as you type, the errors `ns-next` would report before running a
module, shown in the editor at the place they occur, code folding, and a
structure view of each file's classes and members. Beside the `.ns` files,
it knows a project's `newspeak.json`: its schema, and how to run it.

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

Beside them, for a project's `newspeak.json`: completion and validation from
the project file's schema, and running and benchmarking the project from the
IDE ("The project file and running projects").

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
crates/ns-analyzer/     binary `ns-next-analyzer`: its `lsp` command serves
                        diagnostics, semantic tokens, fold ranges and document
                        symbols over LSP; `dump-tokens` is for the lexer
                        comparison; `parse`, `parse-corpus`, `disasm` and
                        `inspect` look at a module or an image from the shell
crates/ns-syntax/       + an editor entry, `parse_for_editor`: a module for
                        every source, recovered, with its comments' spans
crates/ns-runtime/      + a resolver entry answering bindings and errors together
editors/jetbrains/      Gradle project: file type, Kotlin lexer, highlighter,
                        LSP client, structure view, settings, the project
                        file's schema, run configurations
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

## The editor tooling, `crates/ns-analyzer`

The editor tooling is a crate of its own with a binary of its own,
`ns-next-analyzer`: everything that looks at Newspeak without running it. Its
commands are `lsp`, the language server; `dump-tokens`, for the lexer
comparison; `parse` and `parse-corpus` ([frontend.md](frontend.md)); `disasm`,
which prints one compiled body ([compiler.md](compiler.md)); and `inspect`,
which prints an image's structure ([image-format.md](image-format.md),
"Reading an artifact"). It never runs a program: `disasm` compiles a module or
decodes an image, `inspect` reads an image without loading it, and neither
runs the program's code -- the only Newspeak that runs is the shipped core
library's instantiation in the interpreter `disasm` bootstraps, and no other
command starts an interpreter. Running is the `ns-next` binary's. Being a
binary of its own keeps the server's dependencies out of `ns-next` and out of
`ns-runtime`: `lsp-server` (rust-analyzer's transport), `lsp-types` and
`serde_json`, on top of `ns-syntax`, `ns-runtime` and `ns-tooling`, the crate
both binaries share (loading and resolving a source, collecting `.ns` files,
printing diagnostics).

Modules:

- `main.rs` — the command dispatch onto `commands/`, one module per
  command. With no command, an unknown one or arguments after `lsp` it prints
  its usage on stderr and exits with status 2; it never starts the server
  without `lsp`. `lsp` speaks LSP on stdin and stdout. `dump-tokens <files…>`
  prints each file's tokens as JSON (`token_dump.rs`), for the lexer
  comparison: UTF-16 offsets, the unit the plugin's documents use. It refuses
  a file longer than a span can address (`ns_syntax::source_too_long`) on
  stderr with a failing exit status, as it does an unreadable one.
- `server.rs` — the message loop over `lsp-server`, synchronous on one thread.
  Lexing, parsing and resolving one file takes milliseconds, so neither a
  debounce nor a worker pool buys anything. Open documents are keyed by the
  URI's string form, not by `lsp_types::Uri` itself: `Uri` wraps a
  `fluent_uri::Uri<String>` that caches parsed components behind a `Cell`, so
  clippy's `mutable_key_type` refuses it as a map key even though its
  `Hash`/`Eq` only ever read the string. A `semanticTokens/full` or
  `foldingRange` request for a URI the server never opened, or has since
  closed, answers an empty set rather than an error; a `documentSymbol`
  request for one reads the file from disk ("Structure").
- `file_uri.rs` — the file a `file` URI names: the scheme checked, the path
  percent-decoded as a whole (so `file:///c%3A/…` is a drive), and on Windows
  a drive letter (`file:///C:/…` → `C:\…`), a host (`file://server/share/…`
  → `\\server\share\…`, the host percent-decoded) or the four-slash form
  Java's `File.toURI()` writes (`file:////server/share/…`) made a Windows
  path; an empty host or `localhost` is this machine. A query, a fragment, a
  user or port, an IP literal in brackets or a separator as the host, a UNC
  server of `.` or `?` (Win32's device namespaces, `\\.\pipe\…` and `\\?\…`,
  whose pipes and ports could pass for a file and block the server's one
  thread on reading), a path that is not UTF-8 once decoded or holds a NUL, a
  Windows path without a drive and a host on POSIX name no file. `lsp-types` 0.97's `Uri` wraps
  `fluent_uri::Uri`, which parses and decodes but knows no file paths, so the
  conversion is here; `fluent-uri` is a direct dependency only to tell a
  host's kind apart (`HostData`), on the version `lsp-types` uses.
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
`documentSymbolProvider` capability, announced as `true`; answered also for a
`file` URI never opened, read from disk), `shutdown` and `exit`. Nothing
else. A request the server does not implement is answered
`MethodNotFound`. A document notification whose params do not match the
schema has no response to carry an error, so the server writes it to stderr
and sends the client a `window/logMessage` of type `Error` -- the client's
log for the server, not a `window/showMessage` popup, since the defect is
the client's, not the user's text -- and keeps serving the documents already
open.

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
| Class body (instance side) | the header's `(` to the instance side's last `)`, header and members together | `(...)` | none |
| Class side | the `(` after `:` to its `)` | `(...)` | none |
| Method body | `(` to `)` | `(...)` | none |
| Closure | `[` to `]` | `[...]`, or `[:x :y \| ...]` with its parameters | none |
| Slot section -- a class header's, or a body's temporaries | `\|` to `\|`, or `\|\|` to `\|\|` | `\|...\|` | none |
| Comment | `(*` to `*)`, an unterminated one to the end of the file | `(* `, the first non-blank line cut to 60 characters, ` ...*)`; `(*...*)` for a blank one | `comment` |

- **Three ASCII dots** stand for what a fold hides, never the single
  character `…`, which the editor draws in a fallback font beside the
  editor's own.
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
`(...)`, in the editor's own font; and typing an unclosed `foo = (` inside a class keeps the folds of
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
opened, however broken its text.

**A file not open in an editor.** The platform sends `didOpen` only for a file
open in an editor, but the plugin's structure view also asks for a file merely
selected in the Project view. For a URI it has not opened (or has closed), the
server reads the file the URI names (`file_uri.rs`) and analyses it as
`didOpen` would -- `analysis::analyze` and the line index; the text decoded
as UTF-8, as the project's sources are, invalid bytes replaced rather than
the file refused; a leading byte-order mark dropped as the editor's document
drops it -- answers its symbols and keeps nothing, since without `didOpen`
it hears of no change; each request reads the file again. Only a regular
file no longer than a source may be (`ns_syntax::source_too_long`) is read,
asked of its metadata before it is opened: a FIFO would block the opening
and a device such as `/dev/zero` the reading, stalling the one thread every
document is served on.
An open document always wins over the file on disk: it may hold text not yet
saved. A URI that names no file, or a file that cannot be read, is answered
`[]`. Fold ranges and semantic tokens are not read from disk: the platform
asks for them only of a file open in an editor, which it has opened with the
server first.

**The tree.**

- A class's children are flat, in this order: its **factory**, then its
  instance-side members -- the header's slots, the lazy slots, the nested
  classes (each a class with the same children) and the methods, in source
  order -- then its class-side methods, in source order. No node stands for
  a side; a class-side method is told apart by its `detail` (below) and, in
  the plugin's view, by its icon ("The rows").
- A method has its temporaries as children; nothing else but a class has
  children.
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
A class is a `Class`, a factory a `Constructor` and a method on either side a
`Method`. A temporary declared with `=` is a `Constant` and any other a
`Variable`; having no `detail` is what tells an immutable temporary from a
slot with a getter only.

**`detail`** is the effective access word, then ` class` for a class-side
method (`public class`, `protected class`), ` value` for a value class
(`public value`) and ` lazy` for a lazy slot (`protected lazy`); a temporary
has none of them, so no `detail`. LSP has no field or kind for a static
member or a value class, and `detail` is the one a client already reads for
the access, so `class` and `value` go there.
The access is what the member declares, else protected (§6.3.1: "If no access
modifier is declared, the default is protected"); the root class is public
(§6.3.1: the access level of a top-level class is always public); a factory is
public whatever the default says, because the runtime enters every primary
factory as public (`ns_runtime::mixin::factory_entry`).

A value class is one whose "class inherits from class `Value`" (§3.1.1).
For the **root class** the answer is exact: the root rule leaves a root two
ways to reach `Value`, built over it (`Value new`) or with it applied over
`Object` (`Object <: Value`), and `ns_runtime::root_class_is_value` -- read
from the same clauses `resolver::root_clause_defect` checks and
`interpreter::instantiate` builds the root from -- answers it; a root with a
defect is never a value class. For a **nested class** it is an
approximation: its clauses are expressions evaluated in the enclosing
object at run time (§6.1; `interpreter/value_class.rs`), which the editor
cannot run. The server follows the resolver's binding of each clause's
receiver (`LexicalSend { depth }`, as the semantic tokens do): a receiver
bound to a nested class declared in this module makes the class a value
class when that class is one -- found before or after it, in any enclosing
class -- or when that class is named `Value`, as `Kernel.ns` declares the
class itself. Any other receiver -- a slot, a method, a name only
inherited, a `super` clause -- counts when it is named `Value`: the one
case decided by the name alone. So a class over a value class of another
module, or over a slot named `Value` holding something else, is misread.
The class `Value` itself, as `Kernel.ns` declares it (`Object new`), shows
as a plain class: what makes a class a value class is inheriting from it.
The server reads each class's clauses once and spreads value-ness back
from the classes that reach `Value` directly, so no class is decided twice and the
cost is polynomial in the classes and clauses, and the answer is the same whichever class is asked
first, a cycle of clauses (`A = B`, `B = A <: Value`) included.

**Ranges.** A class's `range` runs from its declaration's start to its end,
and its `selectionRange` is its name. A method's range is its declaration;
its selection is its pattern, from the first name to the last. A slot or
temporary is selected at its name. A factory's range and selection are its
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
and asks `textDocument/documentSymbol` through `sendRequestSync`. With no
client it starts one: the platform starts the server only through the
provider's `fileOpened`, for a file opened in an editor, so with no `.ns`
file opened in the session a file selected in the Project view would find no
server. `LspClientManager.startClientsIfNeeded` does not help, since it calls
`fileOpened` for the files open in editors only; the model takes the
registered provider (`LspIntegrationProvider.EP_NAME.findExtension`, the one
instance that holds the plugin's directory) and its `startClient` hands the
same descriptor `fileOpened` would build to
`LspClientManager.ensureClientStarted(NewspeakLspIntegrationProvider::class.java, descriptor)`,
which does nothing for a client already running; a missing binary raises the
same one-time notification. `getClients`, `ensureClientStarted` and
`EP_NAME` are public, not internal, in 2026.1.4 and 2026.2.3. The
platform's own structure view is switched off for the client by
`NewspeakDocumentSymbols`, an `LspDocumentSymbolSupport` with
`structureViewSupport = false`, declared beside the client descriptor in
`NewspeakLspClientDescriptor.kt` and handed to the platform as
`NewspeakLspCustomization`'s `documentSymbolCustomizer`; its breadcrumbs stay
on, and with them the sticky lines, which the platform draws from the same
symbols (`LspFileBreadcrumbsCollector.computeStickyLineInfos`). With no node
for a side, both go from a class straight to its member (`Module › make`),
and no sticky line stands for a side's bracket. They show no static mark on a
class-side method: their icons come from the symbol's kind alone, through
the `symbolKindCustomizer`, which never sees `detail`.

**The rows.** A row's text is the symbol's name, with `lazy` beside a lazy
slot. Its icon is a base icon beside the access icon, and hovering over it
shows what the element is:

| Element | Server sends | Base icon | Hover |
|---|---|---|---|
| root class (the answer's top-level symbol) | `Class` | `NewspeakIcons.ROOT_CLASS` | `Root class`, or `Root value class` with `value` in `detail` |
| nested class | `Class` | `AllIcons.Nodes.Class` | `<Access> class` (`Public class`) |
| nested value class | `Class`, `value` in `detail` | `NewspeakIcons.VALUE_CLASS` | `<Access> value class` |
| factory | `Constructor` | `AllIcons.Nodes.Function` | `Factory method` |
| slot with a getter only (`=`) | `Constant` with an access | `AllIcons.Nodes.Constant` | `<Access> constant` |
| slot with a getter and a setter (`::=`, bare) | `Property` | `AllIcons.Nodes.Variable` | `<Access> variable` |
| temporary declared with `=` | `Constant` without `detail` | `AllIcons.Nodes.Constant` | `Local constant` |
| any other temporary | `Variable` | `AllIcons.Nodes.Variable` | `Local variable` |
| method | `Method` | `AllIcons.Nodes.Method` | `<Access> method` |
| class-side method | `Method`, `class` in `detail` | `AllIcons.Nodes.Method` with the static mark | `<Access> class method` |

The icons say what the program can do with an element rather than LSP's
names for the kinds: a slot is reached only through its accessors, so one
with a getter alone is a constant to every client of it and one with a
setter too is a variable, as a temporary declared with `=` or without one
is. The root class is the module's one top-level class and has an icon of
its own, a blue `N` in a rounded square, and a nested value class an orange
`V` in one; both are drawn as the platform's new-UI node icons are (the
shape filled with a light tint of the colour and stroked with it, the letter
in it), in the colours of its class icon and its constant icon, with
`_dark` variants (`icons/nodes/`, loaded by `NewspeakIcons`). Any other
class is a class.

The access icon is `C_public`, `C_protected` or `C_private` for the first
word of `detail` -- the root class shows `C_public`, the access §6.3.1 gives
it -- joined to the base icon with `IconManager.createRowIcon` as Java's
structure view joins them, so the "show visibility icons" setting behaves as
for Java. A `detail` whose first word is no access word, and a temporary,
shows the base icon alone. A class-side method -- `class` among `detail`'s
words -- has IntelliJ's static mark (`AllIcons.Nodes.StaticMark`) laid over
its base icon, which is how Java shows a static member: the structure view
draws no separator between groups of rows, so the mark is what tells the two
sides apart without a node for each. Java registers the mark as an icon
layer and lays it over the base icon; the plugin builds the same layered
icon with `IconManager.createLayered(baseIcon, AllIcons.Nodes.StaticMark)`,
public API on 2026.1 and 2026.2, without claiming one of the platform's
layer flags, and joins it with the access icon as above.

The hover is a `PresentationData`'s tooltip: an `ItemPresentation` has
none, and the structure view's tree copies a `PresentationData`'s tooltip
into its node (`PresentationData.updateFrom`), which `NodeRenderer` shows.
That holds for the Structure tool window on 2026.1 and 2026.2 and for the
File Structure popup on 2026.1. 2026.2 also carries a File Structure popup
in a split frontend, which receives a presentation as `PresentationDataDto`;
that carries no tooltip, so where the IDE uses that popup it shows no hover.
Over the icon the tree shows the icon's own tooltip instead
(`SimpleColoredComponent.getIconToolTipText`), which a row icon composes
from its parts -- `Public` from the access icon. So the row icon is wrapped
in an `IconWrapperWithToolTip` carrying the same hover text, public API on
2026.1 and 2026.2; it is a `RetrievableIcon` and keeps its tooltip when the
tree replaces the icon inside it for a dark or selected variant.

`NewspeakSymbolKinds`, the client's `symbolKindCustomizer`, gives each kind
the same base icon for the breadcrumbs and sticky lines as for the tree,
every kind named rather than left to the platform's defaults; it sees the
kind alone, so there a root class and a value class show the class icon,
and a temporary declared with `=` the constant icon either way. A pick
moves the caret to the start of the symbol's `selectionRange`, in the
document's UTF-16 columns; the view selects the innermost symbol whose
range holds the caret.

**When it asks.** The model asks the server for the file's children, off the
EDT, and again only once the document's modification stamp has changed; asked
on the EDT it answers what it already has. A usable answer is a non-empty one:
the server always answers the root class for a document it has opened or read
from disk, so no client, no answer in time or an empty answer means the server
is not up yet (it starts the client itself when there is none, above), the
file has not reached it yet (a file opened with the tool window already
showing, or an editor restored at IDE start), or the file cannot be read. A
file selected in the Project view and not open in an editor is answered from
disk, so its tree shows the file as last saved; one whose document holds
unsaved changes while no editor shows it is shown as saved. The platform
refreshes its own view when a client starts through a listener that is
internal API, which the plugin may not use, so while no usable answer exists
the view asks again: every second for 60 tries, then every ten seconds for as
long as it lives, and fast again after an answer; a new model (another file,
the view reopened) starts fast. A server that starts late, or is restarted,
therefore fills the tree without an edit.

**One answer per document stamp.** The Structure tool window builds a new
model each time it shows a file -- selected anew in the Project view, the
window reopened -- and a model that starts empty would show an empty tree
until the server has answered, then the tree: a flicker on every rebuild.
The last usable answer is therefore kept on the `VirtualFile` (user data)
with the document stamp it was given for, and every model of the file answers
an unchanged document from it at once, on the EDT too, without a request; the
server is asked again only once the stamp changes. The user data holds
platform types only -- the platform's `Pair` of lsp4j's symbols and the stamp
-- since a plugin class kept there for the session would hold the plugin's
class loader and stop the plugin from unloading without a restart. A file
with no open document that changes on disk keeps its old structure until the
IDE's VFS notices the change (on regaining focus, or through the file
watcher); its stamp moves then and the server is asked again. A row is equal
to another for an equal `DocumentSymbol` in the same place
(`NewspeakStructureElement.equals`), so a refresh bringing the same answer
keeps the tree's nodes, their expansion and selection.

**Known behaviours.** After a timeout following an edit the tree keeps its
last answer, and its positions are stale until the next answer. Two views of
one file asking at once (the tool window and the popup) before either has an
answer each send a request.
A question on the EDT that finds no current answer schedules a retry too, and
counts against the sixty fast ones.

**Checked under `runIde` (2026-10-04)** on `Kernel.ns`: the Structure tool
window and the File Structure popup show the flat tree, the factory first,
the members in source order, the class-side methods last with the static
mark and the temporaries under their methods; the icons of "The rows" in
light and dark themes, with Java-like access icons (a member without a
modifier shows protected) and `lazy`; the hover on a row's text and the hover
on its icon, which are the same; picking a symbol jumps to its name;
breadcrumbs show the class and method at the caret, and IntelliJ's sticky
lines follow; the tree survives a half-typed method; and it fills on first
open.

### Robustness

An editor sends invalid text all the time. `ns-syntax`'s contract is that
malformed source produces diagnostics, not a panic; the server nonetheless runs
each analysis under `catch_unwind`, turns a panic into one "internal error"
diagnostic at the start of the file, and keeps running. A panic there is an
ns-next defect and must be visible, but must not take the editor's server down
with it. The release profile unwinds (`Cargo.toml` deliberately does not set
`panic = "abort"`), which this relies on.

### Testing

`cargo nextest run -p ns-analyzer`. Besides the language server's tests, the
crate's suite holds the command-line tests (`command_line.rs`: the usage, exit
status 2 for no command or an unknown one, `lsp` taking no argument) and the
tests of the commands `parse`, `parse-corpus`, `disasm` (with its snapshots) and
`inspect`:

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
- document symbols — the flat tree of a class with a factory, a nested
  class, slots, methods and class-side methods, the instance side's members
  before the class side's, each in source order; a class-side method's
  `class` in `detail`; a half-typed method
  keeping the symbols after it; an empty file answering an unnamed class;
  every selection inside its range, also over every module of the corpus and
  every cut of one (`every_symbol_selection_lies_inside_its_range`); a
  symbol's UTF-16 encoding on CRLF lines; a slot's kind and `lazy`; a root
  class's `value` exactly as the root rule builds it, and a nested class's
  through the class its clause binds to -- a later sibling, an enclosing
  class's member, a class named `Value`, a `Value` slot, a cycle -- and a
  class reaching `Value` nowhere; a
  temporary's kind, `Constant` when declared with `=`; and a
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
  quadratic in its size; and a chain of messages as long as the source,
  valid or a run of unclosed methods, analysed on a 1 MiB stack
  (`a_chain_as_long_as_the_source_is_analysed_on_a_small_stack`);
- no empty name — on the token-boundary truncations, no declaration in the
  recovered module has an empty name but the root class's, which recovery
  keeps ([frontend.md](frontend.md), "Recovery");
- protocol — one end-to-end test over an in-memory `lsp-server` connection:
  `initialize` and its announced capabilities, `foldingRangeProvider` and
  `documentSymbolProvider` included, `didOpen`, the expected
  `publishDiagnostics`, `semanticTokens/full`, `textDocument/foldingRange`,
  `textDocument/documentSymbol` (the class, its three children, and `[]` for
  a document never opened and no file on disk), `shutdown`; a malformed
  notification, reported as a `window/logMessage` error while the next
  request is still answered; an unknown request, answered `MethodNotFound`;
  `documentSymbol` for a file
  written to a temporary directory and never opened (its name with a space
  and a non-ASCII letter, so it arrives percent-encoded; and one saved with a
  byte-order mark, its root class still selected at character 6), for an open
  document with other text than the file on disk (the open text wins, the
  file again once closed), and for a missing file, a directory, an
  `untitled:` and an `https:` URI (`[]`), and on POSIX for a FIFO (`[]`,
  without blocking);
- `server.rs`'s unit test — only a regular file no longer than a source may
  be is read, tested on the predicate the metadata feeds, since no test
  writes a four-gigabyte file;
- `file_uri.rs`'s unit tests — the conversion in both path styles on every
  host: drive letters (`file:///C:/…`, `file:///c%3A/…`,
  `file://localhost/D:/…`), percent-decoding, POSIX paths, a host as a UNC
  share on Windows (IPv4 included, a percent-encoded name decoded) and no
  file on POSIX, the four-slash UNC form, an IP literal, a separator and the
  device namespaces `.` and `?` (as host, encoded, or in the four-slash form)
  refused, and the URIs that name no file.

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
- **Settings** — *Languages & Frameworks → Newspeak*: an "ns-next-analyzer
  path", the server binary, and an "ns-next path", the runtime run
  configurations start ("The project file and running projects"); both empty
  by default, typed or chosen with a file picker. "Restart Newspeak Language
  Server", in the Tools menu, restarts the server by hand; applying the
  settings restarts it too. Stored at application level: the paths are
  specific to a machine, so they do not belong in `.idea/`.
- **The project file and run configurations** — the `project` package
  (`NewspeakJsonSchemaProviderFactory`) and the `run` package
  ("The project file and running projects").

### Which binary runs

1. The path from the settings, when set — for working on `ns-syntax` or the
   resolver and seeing the change in the editor at once, pointed at
   `target/debug/ns-next-analyzer.exe`.
2. Otherwise `<plugin directory>/bin/<os>-<arch>/ns-next-analyzer[.exe]`, beside the
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

### The project file and running projects

A project's `newspeak.json` ([image-format.md](image-format.md), "Packaging")
gets its schema in the IDE, and can be run and benchmarked from it.

**The schema.** The plugin's build copies `schemas/newspeak.schema.json`,
the one copy that `ns-next`'s reader is held to, into its resources
(`processResources`, under `schemas/`), and
`NewspeakJsonSchemaProviderFactory` gives it to every file named
`newspeak.json` -- the name `ns-next` recognises a project file by -- and to
no other. The IDE's JSON support does the rest: completion of the keys,
their descriptions, and the schema's rules as warnings (a missing `main`, a
path not ending in `.ns`, a module listed twice, a script that is no
string). A script's description states how `ns-next` splits it into
arguments; whether it splits, with no quote left open, only `ns-next`
checks, when it reads the file ([image-format.md](image-format.md),
"Scripts"). A change to the schema thus reaches the plugin, and runs the
plugin's `check`. The JSON plugin
(`com.intellij.modules.json`) is an optional dependency, declared like the
spellchecker's: `plugin.xml` names `newspeak-json.xml`, which registers the
schema provider, the play buttons and the context-menu producer, all of
which read JSON PSI, so an IDE without the JSON plugin still loads this one,
with neither the schema nor the play buttons. The build compiles against it
with `bundledPlugin("com.intellij.modules.json")`, a module of the IDE it
targets, not a dependency fetched from anywhere.

**The ns-next path.** The plugin runs a project with the `ns-next` the user
chooses, the "ns-next path" in the settings, beside the analyzer's.
`ns-next` is not bundled (decided with the user on 2026-10-05): the setting
is the only source, and the field's comment says to build it with `cargo
build -p ns-cli --release`. Every run reads the setting afresh, so a changed
path needs no restart. With the path empty, or naming no file, a
configuration refuses to start, with an error that says which ("The ns-next
path is not set: …", "No file at the ns-next path …") and offers to open
the settings as its quick fix (`RuntimeLocation`,
`NewspeakRunConfiguration.checkConfiguration`).

**The "Newspeak" run configuration type**, with the plugin's file icon
(`icons/newspeak.svg`, the 16-pixel icon `.ns` files carry), groups the
configurations. A configuration names a project's `newspeak.json`, stored
system-independently, and runs `ns-next run` or `ns-next bench` ("Run" or
"Benchmark" in its form) on that file's directory, in that directory, with
an optional script, extra arguments, and for `bench` the iterations (ten by
default, at least one): `ns-next <run|bench> [--script=<name>] [--iterations
<n>] <project directory> [-- <arguments>]` (`newspeakCommandLine`). The
script is one token, so a name starting with `-` is not read as an option;
an empty script field means no script, as a script's name is never empty nor
surrounded by whitespace ([image-format.md](image-format.md), "Scripts"). A
relative project file is the IDE project's, resolved against its base
directory, both to run it and to recognise the configuration from the file.
The arguments are one field, split as the platform splits program parameters
(`ParametersListUtil`), and go after `--`. A missing project file, a text
that is no valid path among them, is refused before anything starts ("Choose
the project's newspeak.json"), and so are fewer than one iteration.
ns-next's output is decoded as UTF-8, which it writes whatever the platform;
the default, Windows' code page, would garble anything past ASCII.

**The play buttons.** `NewspeakRunLineMarkerContributor` puts a play button
beside the top-level `main` and beside each key of the top-level `scripts`
object of a `newspeak.json`, and nowhere else. Its icon is the platform's
run marker, `AllIcons.RunConfigurations.TestState.Run`, the one every
language's play button uses through `RunLineMarkerContributor`, and clicking
it opens a menu of two entries, "Run" (`AllIcons.Actions.Execute`) and
"Benchmark" (`AllIcons.Gutter.RunInQueue`). `main`'s runs without a script, a
script's with `--script=<name>`. Each entry creates the configuration, or
reuses an equal one -- the same project file, compared system-independently
and with the file system's case sensitivity, the same script and the same
command -- selects it and starts it. A configuration created there is
temporary, as the platform's are: it is reused while equal, can be kept with
"Save Configuration", and is named `<project directory>` or
`<project directory> [<script>]`, with ` (benchmark)` appended for `bench`,
and made unique as the platform makes names unique when another
configuration has it already, so projects in directories of the same name
stay apart.
An invalid one, with no ns-next path say, takes the platform's own path: its
error, and the dialog to edit the configuration. Two scripts written on one
line get one button, holding both scripts' entries: the platform merges the
markers of a line. "Run" in the context menu of a `newspeak.json`, in the
project view or the editor, does the same for the project, or for the script
whose key the caret is on (`NewspeakRunConfigurationProducer`).

Decided with the user on 2026-10-05: the plugin runs a project with the
`ns-next` the user chooses, and bundles none; a run configuration type
"Newspeak", with the plugin's file icon, groups the configurations; a play
button beside `main` and each script opens Run and Benchmark. Chosen beside
them: the platform's run marker as the gutter icon; temporary configurations
from the gutter, named as above; the extra arguments as one field split as
program parameters; the JSON plugin as an optional dependency.

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
  each run `cargo build -p ns-analyzer` at the repository root, debug and `--release`
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
  (`^outer Missing foo`) is underlined with ns-next's message, the folds
  are there, through a syntax error too, with their `(...)` placeholder in the
  editor's font ("Folding"), and the structure view's icons show in light and
  dark themes, with the same hover on a row's text and on its icon
  ("Structure").
- `editors/jetbrains/build/` and `.gradle/` are ignored.

**Marketplace readiness from the first commit.** `plugin.xml` carries the ID,
name, vendor, description, change notes, `sinceBuild` and the dependency on
`com.intellij.modules.lsp`. The bundled binary contains third-party crates —
those `cargo tree -p ns-analyzer -e normal,no-proc-macro` shows compiled into
`ns-next-analyzer` itself, excluding build-time-only proc macros — whose licence
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

- the lexer comparison (`LexerComparisonTest`) — `ns-next-analyzer dump-tokens`
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
- the structure view, in plain JUnit: `NewspeakSymbolKindsTest` (the icon
  per kind, every kind the server sends named),
  `NewspeakStructureElementTest` (a row's name, `lazy`, access icon, the
  base icon and hover of each element in "The rows", root, root value and
  nested value classes included, and `class` read together with the access
  word and `lazy`) and `NewspeakStructureViewModelTest` (no answer and an
  empty answer make the model ask again, an answer with symbols is shown);
  on `BasePlatformTestCase`, `NewspeakStructureViewRefreshTest` (over a file
  never opened in an editor and a stand-in for the server: a new model of an
  unchanged file answers at once, on the EDT too, with no second request and
  no refresh scheduled; a changed file is asked again; rows of equal symbols
  are equal) and `NewspeakLspIntegrationProviderTest` (the
  structure view finds the registered provider it starts the client
  through; starting a client needs the server binary and a running IDE, so
  no test starts one); and on `BasePlatformTestCase`,
  `NewspeakStructureViewFactoryTest`: a
  Newspeak file gets the plugin's view rather than the platform's, the caret
  selects the innermost symbol around it, a member with an access shows the
  kind's and the access icon in one row, a class-side method shows its
  kind's icon layered with the static mark, then its access icon, in one
  row, the plugin's two node icons load at 16 by 16, the root class's row
  holds the root-class and public icons and carries its hover both as the
  `PresentationData`'s tooltip and as the icon's, and only the first
  top-level symbol is the root -- all under the test framework's
  `IconManager`, which answers a row of icons where the IDE's answers a
  `LayeredIcon`, both a `CompositeIcon` of the same icons;
- the project file and running projects:
  `NewspeakJsonSchemaProviderFactoryTest` (a `newspeak.json` gets the
  bundled schema, another JSON file does not, a missing `main` is reported);
  `RuntimeLocationTest`, plain JUnit (the ns-next path empty, naming no file,
  naming one); `NewspeakRunConfigurationTest` (the command line with a script
  and arguments and for `bench` with its iterations, UTF-8 output, a relative
  project file's absolute directory, the form giving back what it was shown
  and storing a system-independent path, the errors for no runtime, a missing
  runtime file, a missing or invalid project file and too few iterations);
  `NewspeakRunLineMarkerContributorTest` (a play button beside `main` and each
  script and nowhere else, none in another JSON file, a menu of "Run" and
  "Benchmark", a configuration created without starting it, reused on a
  second click and when stored with backslashes, and the context menu's
  "Run" for the script under the caret or the project). No test starts
  `ns-next`.

**Gates.** There is one suite: `cargo nextest run --workspace` runs
`ns-analyzer`'s tests before every commit and under every stress mode, like
every other crate's (`CLAUDE.md`, "Working in this repository"). Besides the
language server's, they are the tests of `disasm` and `inspect`, which guard
the compiler's output (the disassembly snapshots) and the image format (an
image decoded and listed, its bodies matching the source's); under the wide
stress mode the analyzer's subprocess compiles every instruction wide, so they
also check that disassembly and decoding do not depend on the form. The
plugin's own gate joins this:

- **Before every commit** that touches `editors/jetbrains/`, `crates/ns-analyzer/`
  or `crates/ns-syntax/` — `./gradlew -p editors/jetbrains check`, besides the
  usual `fmt` and `clippy`. A change to `crates/ns-syntax/src/lexer.rs` must be
  mirrored in `NewspeakLexer.kt` in the same commit, or the comparison fails.
- **Once at the end of a batch**, whether runtime or editor work, the full
  suite under the five stress modes; and, for a batch that touched
  `editors/jetbrains/`, `./gradlew -p editors/jetbrains verifyPlugin buildPlugin`
  — which downloads several IDEs, and so is not a per-commit gate. A resolver
  change that shifts the semantic tokens is then noticed within one batch.

## Known gaps

Reviewed and deliberately left for later; none affects a module that parses
and resolves cleanly in a JetBrains IDE today.

- **The IDE path has no automated test.** It was checked by hand under
  `runIde` (see "Build and packaging"); nothing but that check covers the
  platform handing `NewspeakLspIntegrationProvider` its descriptor
  (`PluginAware`), the sandbox layout `runIde` starts from, the
  platform's `LspFoldingBuilder` reaching a `.ns` file, or the structure
  view against a running server: its asking again until the server has
  opened the file, navigation, the breadcrumbs and the sticky lines
  ("Structure"). The sandbox
  also disables the ultimate marker plugin for `runIde`, not only for tests.
- **Untested paths:** `dump-tokens` on an unreadable file (exit status 1); a
  block comment over a whole-line selection; TODO markers through the
  project-wide TODO index rather than a single open file; a Newspeak run
  configuration starting `ns-next` and showing its output, which the tests
  stop short of (they check the command line and the process's charset).

## Planned later

- **Formatting.** The native LSP API supports `textDocument/formatting` since
  2023.3; once the server announces it, the IDE's Reformat Code uses it with no
  change to the plugin. A platform-native formatter (`FormattingModelBuilder`)
  would need a PSI tree, which is the second parser this design avoids. The
  work is in Rust. A formatter must keep each comment where it was; the
  comments' spans exist (`parse_for_editor`'s `comments`, beside the AST),
  and what remains is attaching each to the nodes around it, and the
  formatter itself: one module that an `ns-next-analyzer fmt` command shares,
  so the editor, the command line and any later formatting gate use one
  formatter.
  It would not touch a file with a syntax error.
- **Navigation.** Go to definition and find usages need an AST for a file
  that does not parse, while it is typed; the fault-tolerant parse exists
  ([frontend.md](frontend.md), "Recovery"). What remains is `definition` and
  `references` in the server.
- **Running a module's `main:args:`** (decided with the user on 2026-10-05):
  a Run/Benchmark play button beside `main:args:` of a `.ns` module's root
  class whose primary factory is the default `new`, found from the
  analyzer's document symbols rather than a second parser in the plugin, and
  the "Newspeak" run configuration taking a single `.ns` file as well
  ([OPEN-WORK.md](../OPEN-WORK.md), "Planned: editor support and the
  analyzer"; "The project file and running projects" above).
- **More platforms.** A CI build producing the other five binaries into
  `bin/<os>-<arch>/`. The repository has no CI yet.
