# Editor support — the language server and the JetBrains plugin

> **Status:** **Built:** the language server (`crates/ns-analyzer/`: `analysis.rs`,
> `semantic_tokens.rs`, `hover.rs`, `folding.rs`, `document_symbols.rs`, `hints/`,
> `code_actions.rs`, the formatting answer in `server.rs`, the breakable
> lines in `breakable.rs`; on
> `ns_syntax::parse_for_editor` and `resolve_for_editor` in
> `crates/ns-runtime/src/resolver/mod.rs`) and the JetBrains plugin
> (`editors/jetbrains/`, its structure view in the `structure` package and
> `lsp/NewspeakSymbolKinds.kt`, the document symbols the structure view and
> the run markers share in `lsp/DocumentSymbolCache.kt`, the editing aids in
> the `editor` package, every text in `messages/NewspeakBundle.properties`)
> both exist: highlighting, a block of `///` lines in the doc-comment
> colour and rendered in place as Markdown, its tags -- `@param`,
> `@answers`, `@signals`, `@see`, `@factory` -- as Java's sections, a hover
> over a name showing its declaration's signature and doc comment, a
> warning for a tag that is wrong, errors as you type, code folding with one-line
> methods shown on one line, hints with their quick fixes, a structure view
> with breadcrumbs, Reformat Code, and typing in the formatter's
> indentation and width, in every commercial JetBrains IDE from 2026.1.4, kept while a file has
> syntax errors (Reformat Code leaves such a file alone). For a project's
> `newspeak.json`, completion and validation from the project file's schema, and play buttons that run or benchmark the
> project with the `ns-next` the settings name; for a module `ns-next` can
> run from its source, a play button beside its `main:args:` that runs or
> benchmarks the module ("The project file and running projects"). Debug
> on a Newspeak run configuration debugs the program in the IDE's
> debugger over a DAP client of the plugin's own (the `debugger`
> package), with line breakpoints on the lines the server's
> `newspeak/breakableLines` answers, their block variants and pass
> counts, and method and exception breakpoints ("Breakable lines";
> [debugger.md](debugger.md), "The JetBrains plugin").
> Range formatting, formatting on type, go to definition, find usages and
> further platforms are planned ("Planned later").

Without a plugin, editing `.ns` files in a JetBrains IDE means editing plain
text. This document describes the editor support that replaces it: syntax
highlighting as you type, the errors `ns-next` would report before running a
module, shown in the editor at the place they occur, code folding, hints
with their quick fixes, a structure view of each file's classes and members,
and Reformat Code.
Beside the `.ns` files, it knows a project's `newspeak.json`: its schema, and how to run it; and it
runs a single module from its `main:args:`.

## Scope

**In scope.** Seven capabilities, in every commercial JetBrains IDE:

1. **Highlighting** — keywords, strings, symbols, numbers, comments,
   selectors, and, once the server has analysed the file, what the resolver
   knows about each name: parameter, local variable, slot, nested class,
   method, `self` / `super` / `outer`.
2. **Errors as you type** — every lexer, parser and resolver diagnostic, with
   the message `ns-next` itself produces.
3. **Folding** — comments, classes, instance sides, class sides, method bodies, closures
   and slot sections collapse and expand, from fold ranges the server
   computes from the AST; a method whose body is one line shows on one line,
   as Java's one-line methods do ("Folding").
4. **Hints** — a local whose first value a statement assigns, and a local
   nothing assigns again, marked as weak warnings, each with a quick fix
   and one fix for the whole file ("Hints").
5. **Structure** — the Structure tool window, the File Structure popup, the
   breadcrumbs and the sticky lines show a file's classes and their members,
   each with its access, from document symbols the server computes from the
   AST ("Structure").
6. **Formatting** — Reformat Code rewrites the file in the formatter's one
   layout ("Formatting"; [formatting.md](formatting.md)).
7. **Documentation** — a block of `///` lines is a doc comment, coloured as
   Java's and rendered in place as Markdown, its tags as Java's sections; a
   hover over a name the server places at a declaration of the file shows
   that declaration's signature and doc comment; the server warns about a
   tag that is wrong, and the structure view shows a member's summary
   ("Hover", "Doc comments").

Beside them, for a project's `newspeak.json`: completion and validation from
the project file's schema, and running and benchmarking the project from the
IDE; running and benchmarking a single module from its `main:args:`
("The project file and running projects"); and debugging what a run
configuration runs in the IDE's debugger, its line breakpoints placed by
the server's breakable lines ("Breakable lines"; [debugger.md](debugger.md),
"The JetBrains plugin").

**Target IDEs.** Every commercial JetBrains IDE that carries JetBrains' own
LSP API: IntelliJ IDEA, WebStorm, PhpStorm, PyCharm, DataSpell, RubyMine,
CLion, DataGrip, GoLand, Rider and RustRover. The free editions and Android
Studio do not carry that API and are not a target. The plugin is written so it
can be published to the JetBrains Marketplace at any time, even while it has
one user: complete metadata, a verifier-clean build, third-party notices, and
no machine-specific paths.

**Out of scope, for now.** Navigation (go to definition, find usages),
completion, and formatting a range or as you type. Each is described under
"Planned later", with what it would need.

## Architecture

```
crates/ns-analyzer/     binary `ns-next-analyzer`: its `lsp` command serves
                        diagnostics, semantic tokens, fold ranges, document
                        symbols, hints with their quick fixes and formatting
                        over LSP; `dump-tokens` is for
                        the lexer comparison; `parse`, `parse-corpus`, `disasm`
                        and `inspect` look at a module or an image from the
                        shell; `fmt` formats sources
crates/ns-syntax/       + an editor entry, `parse_for_editor`: a module for
                        every source, recovered, with its comments; the
                        printer, `format`
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
every change, and the hints with them. The IDE asks for fold ranges and
document symbols, as it asks for semantic tokens, for the code actions at
the caret, and for the formatting edit when the user runs Reformat Code.

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
which prints one compiled body ([compiler.md](compiler.md)); `inspect`,
which prints an image's structure ([image-format.md](image-format.md),
"Reading an artifact"); and `fmt`, the one command that writes, which
rewrites `.ns` files in the formatter's layout
([formatting.md](formatting.md), "Entry points"). It never runs a program:
`disasm` compiles a module or decodes an image, `inspect` reads an image without loading it, and neither
runs the program's code -- the only Newspeak that runs is the shipped core
library's instantiation in the interpreter `disasm` bootstraps, and in the
one the language server bootstraps for the breakable lines ("Breakable
lines"), and no other command starts an interpreter. Running is the `ns-next` binary's. Being a
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
  parse's tokens and the folds over its module and comments, and the hints
  of a source that parses without error ("Hints"). Its
  findings are every diagnostic together -- the lexer's, then the parser's,
  then the resolver's -- so a lexer error does not hide a parser error after
  it.
- `semantic_tokens.rs` — classification and the LSP encoding.
- `folding.rs` — the fold ranges and their LSP encoding ("Folding").
- `hints/` — the hints and their fixes as byte-offset text edits, and
  `apply_all`, which applies every hint of a source; no LSP type
  ("Hints").
- `code_actions.rs` — the hints as diagnostics and code actions, and the
  whole-file edit ("Hints").
- `document_symbols.rs` — the document symbols and their LSP encoding
  ("Structure").
- `breakable.rs` — the lines a line breakpoint can stand on and their
  block variants, and the interpreter they are compiled in ("Breakable
  lines").

**Protocol surface.** `initialize`, `textDocument/didOpen`, `didChange` (full
text on every change), `didClose`, `publishDiagnostics` (push, supported by
the platform since 2023.2), `textDocument/semanticTokens/full`,
`textDocument/foldingRange` (the `foldingRangeProvider` capability,
announced as `true`), `textDocument/documentSymbol` (the
`documentSymbolProvider` capability, announced as `true`; answered also for a
`file` URI never opened, read from disk), `textDocument/formatting` (the
`documentFormattingProvider` capability, announced as `true`; "Formatting"),
`textDocument/codeAction` (the `codeActionProvider` capability, announced
as `true`; "Hints"), the custom request `newspeak/breakableLines`
("Breakable lines"), `shutdown` and `exit`. Nothing
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
  parameters in scope, as the resolver resolves it (§6). A class header's
  factory pattern declares the class side's primary factory, as a method's
  pattern declares the method: each of its keywords (or its unary or binary
  selector) is `method` with `declaration`, and names the factory's own
  declaration, keyed by the pattern's span from its selector through its
  last parameter's type (`factory_span`; for the default `new`, which the
  source does not spell, the empty span at the class's name), so a hover
  there shows the factory ("Hover").
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
analysis, from the AST and the comments `parse_for_editor` answers
beside it; `textDocument/foldingRange` answers them from the cache. A code
fold runs from the opening token to the closing one inclusive, read off the
AST's `Delimiters` ([frontend.md](frontend.md), "AST"), so a fold names what
it folds:

| Range | From — to | Collapsed text | `kind` |
|---|---|---|---|
| Class | the header's `(` to the class's last `)`, the class side's when it has one | `(...)` | none |
| Instance side | the members' `(` to their `)`; to the end of the line before when that `)` stands first on a line the class side opens on, as in `) : (`, and the class side folds; none after a header of one line | `(...)`, or `(...` when it ends with the line before | none |
| Class side | the `(` after `:` to its `)` | `(...)` | none |
| Method body | the spaces before the `(` when the `(` follows the header on its line, else the `(`, to `)` | ` (...)` with those spaces, else `(...)` | none |
| Closure | `[` to `]` | `[...]`, or `[:x :y \| ...]` with its parameters | none |
| Slot section -- a class header's, or a body's temporaries | `\|` to `\|`, or `\|\|` to `\|\|` | `\|...\|` | none |
| Comment | `(*` to `*)`, an unterminated one to the end of the file; a block of `///` lines, first to last | `(* `, the first non-blank line cut to 60 characters, ` ...*)`, or `/// `, that line, ` ...`; `(*...*)` or `///...` for a blank one | `comment` |

- **Three ASCII dots** stand for what a fold hides, never the single
  character `…`, which the editor draws in a fallback font beside the
  editor's own.
- **Only a range over more than one line** is answered: a fold on one line
  hides nothing.
- **One fold marker a line.** The IDE's gutter shows a single marker on
  each line, the smaller fold's when one ends where another starts
  (`FoldingAnchorsOverlayStrategy` in 2026.2.3). A class's three folds are
  laid out so that each has its top marker: the class's on the header's
  line, the instance side's on `) (`, the class side's on `) : (`, which
  the instance side therefore leaves before. The whole class always keeps
  its marker: after a header of one line the instance side would start on
  the header's line and, the smaller fold, take it, so it has no fold
  there; the class side keeps its own.
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
` (...)`, in the editor's own font; and typing an unclosed `foo = (` inside a class keeps the folds of
the methods after it. That each of a class's folds keeps its marker is
checked against the platform's own folding pass and gutter in a throwaway
test over `Kernel.ns` (2026-10-14): every class line, `) (` and `) : (`
shows a marker, and no range of the server's is dropped as conflicting. Were the platform ever to need folding enabled for a
language with a parser definition of its own, as semantic tokens do
(`shouldAskServerForSemanticTokens`, below), it would go into
`NewspeakLspClientDescriptor`'s `lspCustomization`.

**One-line methods.** A method whose body is one line shows on its
header's line, as IntelliJ's "One-line methods" does for Java: the
formatter writes every body on lines of its own ([formatting.md](formatting.md),
rule 10), and `public size = (`, `^size_.`, `)` reads as
`public size = ( ^size_. )`. This is the plugin's, not the server's
(`editor/NewspeakOneLineMethodFolding.kt`, a `lang.foldingBuilder` for
Newspeak beside the platform's): the platform's `LspFoldingBuilder` starts
a range collapsed only for the `imports` and `region` kinds, so no fold
range of the server's can open folded. Two folds per method in one
`FoldingGroup`, so they open and close together, laid out as Java's
one-liners are: the first from the spaces before the `(` (none when the
`(` follows the `=` directly or starts its line) to the statement, shown
as ` ( ` (`( ` without them), the second from the statement to the `)`
included, shown as ` )`; each is as wide as the text it stands for, and a
click on either bracket or the space beside it opens the method. The
platform refuses a fold that partly overlaps one already in the editor
(`FoldRegionsTree.checkIfValidToCreate`), whichever came first: the
server's restored from the last session before any builder runs, or one
that outlived an edit. The server's method fold therefore starts where the
first of these does (the table above), so it contains both and every fold
is created in either order; pinned in `NewspeakOneLineMethodFoldingTest`.
They exist, collapsed, only while *Settings | Editor | General | Code
Folding | Newspeak | One-line methods* is on
(`NewspeakCodeFoldingOptionsProvider`, the `codeFoldingOptionsProvider`
Java's options use; the value in `NewspeakSettings`, application-level);
with it off the method has only the server's fold, as in Java.
The file is never changed. The plugin has no Newspeak parser, so the method
is found in the tokens: `= (` ending a line, one line of body, and a `)`
alone on the next. A class header has the same shape but its `)` is followed
on its line by the instance side's `(`, and a parenthesised slot initializer
by its `.`; a `)` that does not end its line is therefore no method's. A
body with a `///` (it would hide the `)` shown after it), a body that is
only a comment, one whose brackets do not balance, and one longer folded
than the formatter's 100 columns stay open.

### Hover

`textDocument/hover` over a name answers the declaration it names, in
Markdown: the signature in a ` ```newspeak ` block, then the declaration's
doc comment -- its description, then its tags as a table of sections
("Doc comments", below) -- then the class it is a member of
(`*In* `Counter``, or `` `Counter`'s class side``). A client that reads the block as the
definition shows it as one, highlighted as Newspeak -- the JetBrains
platform does for a hover that starts with a fenced block
(`createLspDocumentationData`, 2026.2.3), which is how its Quick
Documentation shows the signature above the text as Java's does; the plugin
needs no code for it, the platform's `LspHoverSupport` being the default.

**Which name names what** is the classifier's decision, the one that colours
it (`semantic_tokens.rs`; "Classification" above), so a hover and a colour never
disagree: beside each name's colour it records the declaration's node the
name names (`Reference`), and each declaration's parts (`Declared`). A
declaration's own name names it; a parameter's or a local's use, the
innermost declaration of that name, as the resolver bound it; a send the
resolver binds lexically (`LexicalSend`, a slot's setter included), the
member of the class at its depth, the primary factory of a class side
the factory's declaration, as its keywords in the class's header do;
an inherited member, which the file does not declare, a send to an object,
`super`'s and `outer`'s, none. The server sees one file, so a name another
module declares has no hover.

| Declaration | Signature, its whitespace runs one space |
|---|---|
| Class | the header up to the instance side's `(`: `class Shapes usingPlatform: platform = Object new` |
| Method | the pattern up to the body's `(`, its `=` left out: `public grow: amount <Integer> by: step` |
| Factory | the header's factory pattern: `usingPlatform: platform`, or `new` for a class whose header names none |
| Slot, local | the declaration, its `.` left out; past 80 characters its initializer is `...`: `private count ::= 0` |
| Parameter | the name and its type: `amount <Integer>` |

**The doc comment** is the plugin's (`NewspeakDocComments`): a block of two
or more `///` lines, attached before the declaration (`Anchor::Before`,
`Placement::OwnLine`) and ending on the line right above it. A single `///`
line, a `(* *)` comment and one a blank line away are no doc comment; the
user decided so, so that what shows green and renders in the editor is what
a hover shows. Its text is `Comment::text`'s, read as a doc comment
(`ns_syntax::doc_comment`, "Doc comments" below): the description goes
into the hover as Markdown, an inline `{@summary …}` replaced by its text,
and the tags as the section table.

**The factory** has no comment of its own: the class's doc comment
describes it, as KDoc's `@constructor` does a primary constructor. Over
the header's factory keywords, and over a send the resolver binds to the
factory, the hover shows the factory's signature, the class comment's
`@factory` text as its description, a "Params:" row of the class
comment's `@param`s, and an "Answers:" row the server writes itself,
``a new instance of `Shapes`.``, which stands with a class comment or without
one; its container is `` `Shapes`'s class side``. Over the class's name the
hover shows the class comment whole: its description, then its sections,
"Factory:" and "Params:" among them.

### Doc comments

A doc comment carries Java's block tags, under Newspeak's names, and an
inline summary. One grammar, stated twice: `ns_syntax::doc_comment`
(`DocComment::parse`, for the server's hover and warnings and for the core
library's documentation test) and the plugin's `editor/NewspeakDocTags.kt`
(for the rendered view and the structure view's hover), each test of one
named as its case in the other, so a rule changed in one is changed in
both. It reads a doc comment's lines without their `///` and the one space
after it:

- **The description** is every line before the first tag line.
- **A tag line**, outside a fenced code block (a line whose trimmed text
  starts with three backticks opens or closes one, so an example's `@`
  stays code), is one whose trimmed text starts with `@`, one or more ASCII
  letters -- the tag's name -- and then whitespace or the line's end. An
  `@` inside a line is text.
- **A tag's text** is the rest of its line after the name, then every line
  up to the next tag line, joined with `\n`; trailing blank lines dropped,
  the first line's leading whitespace too.
- **The known tags.** `@param name text`, a parameter; `@signals Class
  text`, an Error class the member signals and when, one tag per class --
  both take their text's first word as their argument; `@answers text`,
  what a method answers; `@see text`, a related member; and `@factory
  text`, in a class's comment, what the class's factory does, the class's
  `@param`s being the factory's parameters (KDoc's `@constructor`). Any
  other name is an unknown tag, kept with its text in order, and shown,
  verbatim, as a paragraph of the description after the description's own
  text, so a misspelt tag stays visible rather than vanishing.
- **The inline summary**, `{@summary text}` in the description, ends at
  the `}` matching its `{`, braces inside backtick code spans not counted;
  the rendered description shows its text in its place.
- **The summary** is the inline summary's text if there is one, else the
  first sentence of the description's first paragraph: up to and including
  the first `.` outside backtick code spans that whitespace follows or that
  ends the paragraph, so `` `a.b` `` does not end it, else the whole
  paragraph; whitespace runs collapsed to one space; none for an empty
  description. The structure view's hover shows it ("Structure").

There is no `@deprecated`, no `@since`, `@author` or `@version`, and no
`[Name]` link: a reference stays in backticks until go to definition
exists (`OPEN-WORK.md`, "Planned: editor support and the analyzer").

**The sections.** The hover and the rendered view show the known tags after
the description as the platform's section table, the one Java's rendered
doc comments use (`DocumentationMarkup`'s `sections` classes): rows in one
order whatever the comment's -- "Factory:", "Params:", "Answers:",
"Signals:", "See Also:" -- each only when it has a tag, its items in the
comment's order; a `@param`'s and a `@signals`'s item is its argument in
code, a dash and its text (`` `stop` – a number. ``), the others' their text.
The server writes the table into its Markdown as HTML, `<table
class='sections'>`, a row `<tr><td valign='top' class='section'><p>Params:</td><td valign='top'>`,
each cell's items between blank lines, which end the HTML block, so that
the platform's converter keeps the classes and renders each item as
Markdown, its code as the description's (`hover::section_table`; pinned by
the plugin's `NewspeakHoverSectionsTest`). Another client sees a plain
table. The plugin builds the same rows with `DocumentationMarkup`'s
constants, its labels from `NewspeakBundle` (`doc.section.*`), each item
through `DocMarkdownToHtmlConverter`.

**The warnings.** `doc_tags.rs` checks every `///` comment of two or more
lines against the declaration it documents -- by the hover's rule
(`hover::doc_comment`, `hover::directly_above`), so a comment warns about
exactly the declaration it renders over -- and publishes a warning with
the other diagnostics, on the tag's `@name` or its argument:

| Fault | Message |
|---|---|
| an unknown tag | ``unknown doc comment tag `@name`; the tags are @param, @answers, @signals, @see and @factory`` |
| `@param` without a name | ``` `@param` needs the name of a parameter ``` |
| `@signals` without a class | ``` `@signals` needs the class of the Error signalled ``` |
| `@param` naming no parameter of a method or factory | ``` `name` is no parameter of `selector` ``` |
| `@param` above a slot or a class without a factory | ``` `name` is no parameter: the comment documents a slot ``` (`a class without a factory`) |
| `@factory` outside a class's comment | ``` `@factory` describes a class's factory: the comment documents a method ``` (`a slot`, `nothing`) |

A `@param` in a comment that documents nothing has no parameters to be
checked against and no warning. Whether a member has a `@param` for each
parameter, an `@answers` or a `@factory` is no warning: the core library's
documentation test holds its own modules to that
(`crates/ns-runtime/tests/core_library_documentation.rs`; `CLAUDE.md`,
"Conventions"), and other code is free to say less.

### Hints

The server points at two things in any `.ns` file, as a hint the editor
shows as a weak warning, each with a quick fix and one fix for the whole
file:

- **A, initialize in the declaration** (`initialize-in-declaration`): a
  local declared bare whose first value a statement assigns can take that
  value in its declaration -- `| count | count:: 1.` becomes
  `| count ::= 1. |`.
- **B, read-only** (`read-only`): a local declared with `::=` that nothing
  ever assigns again can be declared with `=`.

Every fix preserves behaviour; where that cannot be proved from the source
alone, no hint is given. The hints are syntactic: everything a rule needs is
in the tree, so they need neither the resolver nor a running program, and
only a module that parses without error gets them. The formatter stays as it
is: it changes layout only, and these hints change code.

**Sections.** A *section* is a code body, a method's or a closure's, with
its temporaries and statements; the hints are given for the locals of
sections only. A class's header is no section: its slots get no hint of
either kind, private ones included, but the closures in its expressions are
sections like any other. A local is reachable from its own code body alone,
so what the body says about it is all there is; a header slot is not. A
`::=` slot has a setter, which the class's clients and subclasses can send
unless the slot is private; a header's `x:: e` is a setter send a subclass
can override, which an initializer does not send; and a private slot's B
fix could lift a value object's refusal of a mutable slot
([core-library.md](core-library.md)), a change of behaviour, while whether a
class's instances are values is not in its declaration. A section with a
simultaneous slot clause (`|| ||`) gets no hint.

**Hint A.** A local `x` is a candidate when it is declared bare (`x`, no
initializer) and a statement `x:: e.` of its own section assigns it. The
statement qualifies when:

- no comment stands in it, in the text a fix deletes or on the rest of its
  line, and no comment is attached to it -- an attached comment describes
  the statement it stands above, and deleting the statement would leave it
  describing the next;
- it is not parenthesized and is not a chain (`x:: y:: e`), and `e` does
  not mention `x`;
- it is not the last statement of a closure body: a closure answers its
  last statement's value, which an initializer does not have. A method
  answers `self` without `^`, so a method's last statement may move;

and one of these holds:

1. **At the start.** It is the section's first statement. Whatever `e` is,
   moving it keeps the order of evaluation as long as every existing
   initializer of the section still runs first, then `e`. The local is
   reordered for that: its declaration moves after the last initialized
   one. A comment in what the move removes, between the local and the next
   one, or after the last initialized one on its line would be deleted or
   copied by the move, or left behind the moved declaration; when a comment
   stands there, no hint is given. Each fix stands alone: once the first
   statement has moved, the next one is first, which is how "apply all"
   moves a whole run of them.
2. **After other statements.** `e` is a literal (`nil`, `true` and `false`
   among them), `self`, or an identifier whose innermost declaration in
   scope is a parameter (of a method, a closure or a factory) or a local
   declared with `=`; no statement before it and no initializer of the
   section mentions the name `x` anywhere -- read, assigned or sent as `x`
   or `x:` -- nested closures included (a closure an initializer makes
   could read `x` before the statement runs); and, as in 1, every existing
   initializer of the section still runs before it, the local's declaration
   moving after the last initialized one when needed. Such an `e` can
   neither fail nor observe anything those statements change, and neither
   they nor any initializer can observe `x` before it, so evaluating `e`
   earlier is unobservable.

**Hint B.** A local `x` declared with `::=` is a candidate when nothing can
assign it: no `x::` anywhere in its section, its initializers and nested
closures included. Only `x::` writes a local: an implicit-receiver send
`x: v` is resolved through the classes, never the locals
(`ns-runtime`'s `resolver/mod.rs`). The test is by name and conservative:
an `x::` that writes something else, a shadowing closure local, still
counts, which costs a hint at worst, never correctness. Nor may a closure
read `x` before its initializer has run: no initializer of `x` itself or of
a local declared before it in the section may mention `x`, nested closures
included. The optimized lowering copies a read-only local into a closure
when the closure is made ([compiler.md](compiler.md)), so a closure made
before the initializer ran -- `| f = [x]. x ::= 1. |`, or the
self-referential `fib ::= [:n | ... fib value: n - 1 ...]` -- would read nil
where the `::=` local it shares reads the value. Any mention counts, a
closure's or not, so the rule stays conservative.

**The edits are minimal**, so a file keeps its layout and a file never
formatted is not reformatted by a quick fix (`hints/edits.rs`):

- A appends ` ::= <value>.` to the declaration (` ::= <value>` before the
  `.` a bare local may be written with, §6.3.2), the value copied verbatim
  from the source, its continuation lines shifted by the difference in
  indentation between the statement and the declaration -- for a reordered
  local, the column its declaration moves to. A line that starts inside a
  string or a symbol literal is part of the literal's value and is copied
  as it is; the lexer finds the literals, so a quote in a comment opens
  none. A deletes the statement and its period, and its line when that
  leaves the line blank; and, when the local is reordered, moves its
  declaration, its own `.` removed with it.
- B replaces `::=` by `=`. The operator and a bare local's `.` are found by
  the lexer, not by searching the text: a comment beside them may spell
  either.
- Every fix's result parses: the rules promise it, `apply_all` checks it
  with a `debug_assert!`, and the behaviour oracle holds every golden to it.
  Should a round of `apply_all` still not parse -- a defect in a rule --
  a release build answers the last round's text that did, so the editor
  is never offered a text that no longer parses.
  Parsing each fix's result while hints are computed would cost a parse per
  hint on every keystroke, so it is not done there.

**Apply all** (`hints::apply_all`) works in rounds until no hint is left:
each round applies at most one hint per section and none whose edits
overlap another's -- a moved statement may hold a closure with hints of its
own, whose edits the move would carry along -- then finds the hints of the
result. Every fix is valid on its own, so every round preserves behaviour,
and each removes an assignment or a `::=`, so the rounds end.

**The server.** `hints/` computes the hints from the AST: for each its kind,
the local's name and the span of it, its section, and its edits in byte
offsets; it holds no LSP type. They are computed once per document version
with the rest of the analysis (`analysis.rs`) and cached.
`publishDiagnostics` carries them beside the errors: severity `HINT`, `code`
`initialize-in-declaration` or `read-only`, `source` `ns-next`, at the
local's name, with the message "`x` can be initialized in its declaration"
or "`x` is never assigned again and can be read-only". The server announces
`codeActionProvider`; `textDocument/codeAction` answers, for each hint the
requested range touches, a `quickfix` action ("Initialize `x` in its
declaration", "Make `x` read-only") naming its diagnostic and carrying its
edit, and beside them one action, "Apply all hints in this file", whose one
edit replaces the smallest range covering every change. A range that
touches no hint, and a document the server has not opened, answer no
action. The edits are plain `changes`: with full synchronisation the server
always holds the text the client asks about. The whole-file edit runs every
round of `apply_all`, each parsing the text afresh, so it is computed only
when a code-action request first needs it and then kept with the document
version (`code_actions::whole_file_edit`, in `server.rs`'s `Document`). Its
later rounds parse and edit texts the analysis never saw, so it runs under
`catch_unwind`, as the analysis does ("Robustness"): a panic there answers
no whole-file action and is reported on the server's standard error, and
the quick fixes are still offered.

**The plugin needs no code for it.** The platform shows a diagnostic of
severity `HINT` as a weak warning (`LspDiagnosticsSupport.getHighlightSeverity`)
and offers its quick fixes and the whole-file action on Alt+Enter
(`LspCodeActionsSupport`). Checked by hand under `runIde` (2026-10-14): a
method local `count` declared bare and assigned by the first statement
shows a weak warning; Alt+Enter offers "Initialize `count` in its
declaration" and "Apply all hints in this file", and both do what their
titles say; after the first, the editor offers "Make `count` read-only".

**Tests.**

- The unit tests in `hints/mod.rs`, on `indoc!` modules: each rule above
  with its positive and negative cases -- a closure's own `x::`, a comment
  spelling `::=`, header slots getting no hint of either kind, a local its
  own or an earlier initializer captures (a self-referential closure among
  them) and one a later initializer captures, a guard before the
  statement, a value that may fail or change, a guard or an initializer's
  closure mentioning the local, the reordering and the comments that
  prevent it, a comment in, beside or attached to the statement, a chain,
  a parenthesized assignment, the last statement of a closure and of a
  method, a bare local written with its `.` -- each fix
  applied, parsed and compared with the expected text, CRLF lines and
  continued values included, a multi-line string or symbol moved right and
  left and a reordered value following its declaration's new column;
  "apply all" over a run of assignments, into a moved closure, past a local
  an earlier initializer captures, and a second time changing nothing.
  `code_actions.rs`'s unit tests cover the smallest changed range on
  character boundaries and a panic while applying every hint answering no
  whole-file edit.
- **The behaviour oracle**, `crates/ns-analyzer/tests/hinted_goldens.rs`:
  "apply all" over every golden program's sources (for a project, every
  source its `newspeak.json` names, outside its directory too, and every
  one inside it), each changed program run from a scratch copy under both
  lowerings and held to the `status`, `stdout` and `stderr` its `.toml`
  records; each result parses, and a second "apply all" changes nothing.
  The goldens themselves are not rewritten. `goldens/language/slot/local/`
  holds goldens written for the oracle, with the shapes a defect in a fix
  would show in: a closure made before its local is initialized or
  assigned, a closure in its local's own initializer, a multi-line string
  a moved value carries. The programs run on `ns-next`, which
  `ns-analyzer` does not depend on and cargo does not hand a test of
  another package, so the oracle builds it itself through cargo on first
  use; it shares `ns-cli`'s golden helpers by `#[path]`. It runs every
  golden twice more, so it runs only in its own nextest profile, `hints`,
  for every commit that touches `crates/ns-analyzer`: `cargo nextest run -p
  ns-analyzer --profile hints` (`.config/nextest.toml`; the default profile
  leaves it out). Its trials over the benchmark programs, which take up to a
  minute each, run in the profile `benchmarks-verify` instead.
- **The protocol test** (`tests/protocol.rs`): a hint published with its
  severity, code, source and range; its quick fix and the fix for the file
  answered, the latter's one edit giving what `apply_all` gives; no action
  away from a hint.
- **The core-library gate**, `crates/ns-analyzer/tests/hint_gate.rs`, beside
  the formatting gate and in the default profile: no shipped core-library
  module (`crates/ns-runtime/core-library/`) has a hint. Its failure lists
  each hint and names the ignored test that applies every hint to the core
  library in the formatter's layout (`apply_every_hint_to_the_core_library`,
  run deliberately; the image fixture is regenerated after it, its header
  carrying the sources' fingerprint). Only the core library is held to it:
  the goldens pin a program's form along with its behaviour, the benchmarks
  stay close to the programs they adapt, and `corpus/` stays upstream's
  verbatim copy.

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
API alone: its model asks through `DocumentSymbolCache` (below), which finds
the client with
`LspClientManager.getInstance(project).getClients(NewspeakLspIntegrationProvider::class.java)`
and asks `textDocument/documentSymbol` through `sendRequestSync`. With no
client it starts one: the platform starts the server only through the
provider's `fileOpened`, for a file opened in an editor, so with no `.ns`
file opened in the session a file selected in the Project view would find no
server. `LspClientManager.startClientsIfNeeded` does not help, since it calls
`fileOpened` for the files open in editors only; the cache takes the
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

A member with a doc comment shows its summary ("Doc comments") on a line of
its own under the hover in the table, the two escaped and joined as HTML
(`Public method`, then `Answers the size of the receiver.`), in the
tooltip and the icon's alike, so the view tells what a member does without
opening it. The plugin reads it from the file's current text
(`NewspeakDocComments.summaryAbove`: the doc comment ending on the line
above the symbol's range), never from the server: a document symbol's
`detail` carries words the plugin reads (the access, `class`, `value`),
and the comment stands right above the symbol anyway. The summaries are
computed once per change of the file, by the 0-based line each comment
ends on, since the view asks for every row and a walk of the file per row
would make a large module's tree quadratic.

`NewspeakSymbolKinds`, the client's `symbolKindCustomizer`, gives each kind
the same base icon for the breadcrumbs and sticky lines as for the tree,
every kind named rather than left to the platform's defaults; it sees the
kind alone, so there a root class and a value class show the class icon,
and a temporary declared with `=` the constant icon either way. A pick
moves the caret to the start of the symbol's `selectionRange`, in the
document's UTF-16 columns; the view selects the innermost symbol whose
range holds the caret.

**When it asks.** The model asks the server for the file's children, off the
EDT, through `DocumentSymbolCache.fetch`, and again only once the document's
modification stamp has changed; asked on the EDT it answers what it already
has. A usable answer is a non-empty one: the server always answers the root class for a document it has opened or read
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
The last usable answer is therefore kept by `DocumentSymbolCache`, a project
service in the `lsp` package, on the `VirtualFile` (user data) with the
document stamp it was given for, and every model of the file answers an
unchanged document from it at once, on the EDT too, without a request; the
server is asked again only once the stamp changes. The stamp is read before
the question is sent, so an answer to older text is never kept under a newer
stamp, and `current` answers the kept symbols only while their stamp is the
document's (the open document's, else the file's), so no reader places
anything by the positions of older text. The same cache serves the module's
play button and "Run" in its context menu ("The project file and running
projects"), so an answer one of them has asked for serves the others while
the document is unchanged. The user data holds
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

### Formatting

`textDocument/formatting` formats the open document's text with
`ns_syntax::format` ([formatting.md](formatting.md)) and answers one
`TextEdit` that replaces the whole document, from its start to its end in
UTF-16 positions, with the formatted text. It answers no edit when the text
is formatted already, when it does not parse -- a document with a syntax
error is left as the user wrote it -- and for a document the client has not
opened: like folds and tokens, only an open editor buffer is formatted,
never a file read from disk. The request's options (`tabSize`,
`insertSpaces`) are ignored, since the formatter has one style. The
platform's native LSP API hands Reformat Code to the server, but by default
only for a file the server has registered formatting for dynamically
(`client/registerCapability`); the static `documentFormattingProvider`
the server announces does not count, and without more Reformat Code never
reaches the server and reports the file as formatted already. The plugin's
`NewspeakFormatting` (`NewspeakLspClientDescriptor.kt`, a
`LspFormattingSupport`) therefore sends every file the client supports to
the server, which is the only formatter Newspeak has. A platform-native
formatter (`FormattingModelBuilder`) would need a PSI tree, the second
parser this design avoids. The format runs on the
server's one thread under `catch_unwind`, as the analysis does
("Robustness"): a panic in the printer answers no edit and leaves the
document as it is.

### Breakable lines

The custom request `newspeak/breakableLines` answers the lines of an open
document a line breakpoint can stand on, from which the plugin decides
`canPutAt` and a line breakpoint's variants before any debug session runs
([debugger.md](debugger.md), "A breakpoint names a source position").
Its params are `{ "textDocument": { "uri" } }`; its result is
`{ "version", "lines" }`: `version` the document's version the server
holds, so that the plugin keeps the answer per version, and `lines` one
entry `{ "line", "variants" }` per line a breakable statement starts on,
in order, `variants` listing, for each statement starting on the line
that is the first breakable statement of a block literal, the literal's
`range` and the statement's start `position`, in order -- the block may
begin on a line above. Lines and positions are 0-based LSP positions in
UTF-16 code units. A document whose text does not parse (`ns_syntax::parse`,
not the editor's recovering parse) or does not resolve (`resolve`) answers
`"lines": null`, every line of which the plugin allows; a document the
client never opened, or has closed, is an `InvalidParams` error rather
than an empty set, which would forbid every line.

The plugin's `BreakableLinesCache` (`debugger/`) keeps each file's answer
with the document stamp it was asked for, as `DocumentSymbolCache` keeps
the symbols, and drops one whose `version` is not the one the platform's
client holds for the document -- an edit the server has not been told of
yet. `NewspeakLineBreakpointType.canPutAt` answers from it and never
waits: while there is no answer for the current text -- the server not
started, the document not opened with it, an error -- it allows every line
and asks in the background, again every second for about a minute, so
that a click is never refused for want of an answer. A file that is not
on disk -- a module's text the debugger shows by reference -- is allowed
every line without asking. A line's variants are the line and each of
its blocks (`computeVariants`), but a block whose first statement is the
line's first, where the two stop at the same statement; a block
variant's breakpoint keeps the column of that statement and is sent with
it ([debugger.md](debugger.md), "The JetBrains plugin").

The answer is `Interpreter::breakable_statements`' over the resolved
module, which compiles every level of it under the optimized lowering
without loading or running anything, grouped by line. It is
compiled on a worker thread of its own, which the server spawns at the
first request and asks synchronously, handing it the document's text: the
worker parses, resolves and compiles, and answers spans only, since an
interpreter and the symbol interner are bound to the thread that made
them. The worker bootstraps one interpreter, at its first module rather
than at the server's start, so that an editor that never asks pays
nothing, and keeps it: the bootstrap instantiates the shipped core
library, the only Newspeak that runs. The answer is computed once per
document version, by the first request for it; on `Kernel.ns`, 8,355
lines and 1,399 breakable statements, `breakable_statements` takes about
9 ms in a debug build, its parse and resolve about 8 ms more. The compile
runs under `catch_unwind`, as the analysis does ("Robustness"): a panic
answers `"lines": null` and drops the interpreter, which a new one
replaces at the next request.

**The chain guard.** The compiler recurses along a chain of messages,
which the parser does not bound ([frontend.md](frontend.md), "Recovery",
rule 7), and a stack overflow aborts the process, past `catch_unwind`,
taking the editor's support for every file with it. So the worker
counts, without recursion, the most sends any path through the module
nests -- a chain one per message, `breakable::nested_sends` -- and above
`MAX_NESTED_SENDS`, 20,000, answers `"lines": null` without compiling.
The cap sits above what `ns-next run`'s 8 MiB thread compiles today -- a
body of one chain overflows it past about 14,900 messages in a release
build and 10,000 in a debug one, measured by bisecting the chain's length
with `ns-next run` -- so every module `ns-next` can compile gets its
exact answer; and below what the worker's stack holds: it reserves
64 MiB, address space rather than memory, on which a debug build
compiles a chain of 80,000 and overflows below 100,000. Pinned by
`a_chain_past_the_cap_answers_null_lines_and_the_server_serves_on`. The
guard goes when the compiler takes a chain link by link
([OPEN-WORK.md](../OPEN-WORK.md), "A valid method whose body is one long
chain of messages aborts `ns-next run`").

### Robustness

An editor sends invalid text all the time. `ns-syntax`'s contract is that
malformed source produces diagnostics, not a panic; the server nonetheless runs
each analysis under `catch_unwind`, turns a panic into one "internal error"
diagnostic at the start of the file, and keeps running. A panic there is an
ns-next defect and must be visible, but must not take the editor's server down
with it. The whole-file hint edit runs under `catch_unwind` too ("Hints").
The release profile unwinds (`Cargo.toml` deliberately does not set
`panic = "abort"`), which this relies on.

### Testing

`cargo nextest run -p ns-analyzer`, and for the hints' behaviour oracle
`cargo nextest run -p ns-analyzer --profile hints` ("Hints"). Besides the
language server's tests, the
crate's suite holds the command-line tests (`command_line.rs`: the usage, exit
status 2 for no command or an unknown one, `lsp` taking no argument) and the
tests of the commands `parse`, `parse-corpus`, `disasm` (with its snapshots) and
`inspect`:

- positions — byte offset to UTF-16 with non-ASCII text, umlauts and emoji
  (surrogate pairs), at line ends and at the end of the file, and back: every
  character's offset read back from its position, a position past its line
  or inside a surrogate pair clamped, a line the text lacks refused;
- hover (`hover.rs`) — a method over each keyword of its pattern and of a
  send bound to it, with its description, its tags as the section table in
  the sections' order whatever the comment's, and its class; a comment
  without tags with no table; an inline summary read as its text; the
  factory over its header keyword and a send bound to it, with its
  signature, its `@factory` text, its parameters and its answer, and the
  default `new`; a class over its declaration and a use, with its
  "Factory:" and "Params:" rows; a slot over its name, its setter and a
  read, and a long one's initializer left out; a parameter with its type
  and a local; a single `///` line and a comment a blank line away showing
  none; a class-side method; a send to an object and an inherited member
  answering no hover; and over the protocol, the capability and a hover's
  Markdown and range (`protocol.rs`);
- doc comments — the grammar (`ns_syntax::doc_comment`'s tests): a
  comment without tags all description, `@factory` without an argument,
  the tags splitting the description off with a tag's text over several
  lines, an unknown tag kept in order and shown in the description, a
  `@param` without a name, an `@` inside a line and a tag line inside a
  code fence as text, the first sentence as the summary, not ended inside
  code, an inline summary winning and read as its text, ending at its
  closing brace outside code, every span holding on CRLF lines, and a
  `(* *)` comment an empty doc comment; the warnings (`doc_tags.rs`): an
  unknown tag, `@factory` outside a class, a tag without its argument, a
  `@param` naming no parameter of a factory, of a method and of a class
  without a factory, one above a slot, a single `///` line not checked, a warning placed on CRLF lines,
  and a correct comment with none;
- diagnostics — one `indoc!` module each with a lexer, a parser and a resolver
  error, checking range and message; a lexer error's findings come first, the
  parser's after them, and a syntax error keeps the names around it
  classified;
- folding — a class, its instance side and its class side, no instance
  side fold after a header of one line, the instance side's fold before a
  class side on its `)`'s line, a method with its temporaries and a closure, a comment folded to its first line, an unterminated comment to
  the end of the file, single-line constructs answering no range, the folds
  after a half-typed method, and a fold's UTF-16 encoding on CRLF lines;
- hints — the unit tests of `hints/` and `code_actions.rs`, the behaviour
  oracle `hinted_goldens.rs` in its own profile, and the core-library gate
  `hint_gate.rs` ("Hints", "Tests");
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
  temporary's kind, `Constant` when declared with `=`; a
  member's effective access; and what the module's play button reads -- a
  root's `new`, written or omitted, and its `main:args:` named by its
  pattern, `public` on the instance side, `public class` on the class side,
  `protected` without a modifier;
- semantic tokens — a small module per category (parameter, local, slot with
  and without `readonly`, nested class, declared method, inherited send,
  a receiver inside a keyword message's span, a lazy slot in both
  modifier orders and both forms, a name a slot on one side of a class and a
  method on the other, read from either side, from a nested class and from a
  superclass clause, the primary factory as a class-side member only, and
  the header's factory keyword as a declared method),
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
  `initialize` and its announced capabilities, `foldingRangeProvider`,
  `documentSymbolProvider`, `documentFormattingProvider` and
  `codeActionProvider` included,
  `didOpen`, the expected `publishDiagnostics`, `semanticTokens/full`, `textDocument/foldingRange`,
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
  without blocking); `textDocument/formatting` answering one edit over the
  whole document whatever options it is sent, and no edit for a formatted
  document, a document with a syntax error and one never opened; a hint
  published with its quick fix and the fix for the file, and no action away
  from a hint;
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

**Texts.** Every text the plugin shows -- settings labels and comments, the
colour settings page's attribute names, notifications, the run configuration
type and its editor, tooltips, the structure view's hover texts, the action's
name -- is a key of `src/main/resources/messages/NewspeakBundle.properties`,
read through `NewspeakBundle.message` (`lazyMessage` where a text is read
later, as the colour settings page's attribute names are); `plugin.xml`
names the bundle with `<resource-bundle>` and refers to it by `key` (the
settings page, the notification group) and by `action.<id>.text` and
`action.<id>.description`. Ids stay in code -- the language and file type
names, the configuration type and notification group ids -- and so do the
plugin's `<name>`, `<description>` and `<change-notes>`, which are Marketplace
metadata. A message with parameters is a `MessageFormat` pattern, so a `'` in
it is written `''`. There is no translation: the IDE's language comes from
JetBrains' language packs, and a `NewspeakBundle_xx.properties` beside the
bundle is what one would add.

Components:

- **Language and file type** — `NewspeakLanguage`, `NewspeakFileType` for
  `.ns`, an SVG icon with a light and a dark variant.
- **Lexer** — hand-written Kotlin, a `LexerBase` that mirrors the Rust lexer
  function by function, so the two can be reviewed side by side and no
  generator step exists (JFlex was considered and rejected for that reason).
  Every `(* … *)` comment — nested ones included — and every string is a
  single token, so the lexer restarts in state 0 at any token boundary, which
  the platform's incremental lexing needs. A `///` comment is one token per
  line, of its own type `LINE_COMMENT`: this lexer -- the parser's, the
  PSI's, the TODO search's -- mirrors the Rust lexer token for token, which
  the lexer comparison holds, so it merges nothing and leaves blocks to the
  Rust side ([frontend.md](frontend.md), "Comments"); only the highlighting
  lexer, below, merges a doc comment. Unlike the Rust lexer it also emits
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
  `PARAMETER` and a slot from `INSTANCE_FIELD`, as in TypeScript. A block of
  two or more `///` lines documents what follows it, as a Java doc comment
  does, and takes a key of its own over `DOC_COMMENT` (Java's green in the
  platform's schemes); a single `///` line, and a `///` after code, keep the
  comment key. The block is the Rust lexer's (`continues_line_block`: lines
  that each start their line, one line break and only whitespace between
  them), the rule `editor/NewspeakDocComments.kt` states over the PSI for
  the rendered view. A line's colour depends on the lines around it, and
  the editor paints the highlighter's colours before any daemon pass, so
  the colour comes from the highlighting lexer, `NewspeakHighlightingLexer`,
  which the `SyntaxHighlighter` alone answers: the parsing lexer's tokens,
  but a block as one `DOC_COMMENT` token from its first line's `///` to the
  end of its last line's text, the whitespace between included. An
  annotator runs only after the file is shown, so every doc comment would
  show grey first. The lexer reads whether a `///` starts its line from
  the buffer, so it restarts at any token; its state says whether a token
  starts its line (0) or not (1), because the editor relexes an edit from
  two tokens before it, walked back to a token in state 0, and stops only
  at one in state 0 that matches the old tokens: a `///` typed on the line
  under a single one relexes from that line, which starts its line, not
  from the whitespace after it, and a relex never stops inside a line,
  whose start decides whether a `///` further on is a remark. Every reader
  of the highlighter's tokens takes the merged token: the indentation, for
  which each of its lines is a `///` line of its own; Enter in a `///`
  comment, which continues the caret's line of a block; and the TODO
  index, which reads the highlighter's tokens against the parser
  definition's comment set, `DOC_COMMENT` included.
- **Rendered doc comments** — every doc comment, and only those, renders in
  place as Java's do: `NewspeakInlineDocumentationProvider`, the platform's
  `inlineDocumentationProvider` extension point (every language's, so it
  answers nothing for another file), hands the platform each block's range
  and its text without the `///` and the space after it, read as a doc
  comment (`NewspeakDocTags`, "Doc comments"): the description as Markdown
  by the platform's own `DocMarkdownToHtmlConverter`, code fences
  highlighted as Newspeak, an inline `{@summary …}` as its text and an
  unknown tag as a paragraph of it, then the known tags as the platform's
  section table, as Java's rendered tags, each item through the same
  converter; no Markdown library of the plugin's own. The platform draws it,
  puts the toggle between the rendered and the raw text in the gutter, and
  follows *Settings | Editor | Appearance | Render documentation comments*
  and the reader mode. What a comment documents is the server's to say: its
  owner is the document symbol (`DocumentSymbolCache`) that starts on the
  line after it, the first in pre-order, so the outermost; before the server
  has answered for the current text a comment has no owner and renders all
  the same, and the daemon runs again once the answer is kept
  (`DocumentSymbolCache.currentForDaemon`, which the module's play button
  reads the same way). LSP has no rendered comments of its own: the server's
  only part is the symbols.
- **Editor features** — a `BraceMatcher` for `()`, `[]` and `{}`, a
  `Commenter` for `///` lines and `(* *)` blocks, a `QuoteHandler`. Not for `<>`: `<` and `>` are
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
  asks the server ("Folding"). The server's own request,
  `newspeak/breakableLines`, is declared on `NewspeakLanguageServer`, an
  lsp4j interface the client descriptor names as its `lsp4jServerClass`,
  which is how the platform's client sends a request the protocol does
  not define ("Breakable lines").
- **Structure view** — `NewspeakStructureViewFactory`, its model and
  `NewspeakStructureElement`, over the server's document symbols, which
  `DocumentSymbolCache` keeps per file and stamp for the structure view and
  the module's play button alike, with `NewspeakSymbolKinds` and
  `NewspeakDocumentSymbols` in the client descriptor's customization; a
  row's hover adds the summary of the member's doc comment
  ("Structure").
- **Settings** — *Languages & Frameworks → Newspeak*: an "ns-next-analyzer
  path", the server binary, and an "ns-next path", the runtime run
  configurations start ("The project file and running projects"); both empty
  by default, typed or chosen with a file picker. "Restart Newspeak Language
  Server", in the Tools menu, restarts the server by hand; applying the
  settings restarts it too. Stored at application level: the paths are
  specific to a machine, so they do not belong in `.idea/`.
- **The project file and run configurations** — the `project` package
  (`NewspeakJsonSchemaProviderFactory`) and the `run` package: the "Newspeak"
  run configuration, the play buttons of `newspeak.json` and its context
  menu's producer, and the module's play button beside `main:args:`
  (`NewspeakModuleRunLineMarkerContributor`) and its context menu's producer
  (`NewspeakModuleRunConfigurationProducer`) ("The project file and running
  projects").
- **The debugger** -- the `debugger` package: the DAP client in `dap/`
  (`DapConnection`, `DapClient`, `DapTypes`), Debug on a Newspeak run
  configuration (`NewspeakDebugRunner`, which starts `ns-next run
  --debug-connect` against a socket the plugin listens on), the
  `XDebugProcess` (`NewspeakDebugProcess`) with its suspend context,
  stack, frames, values and sources, and the three breakpoint types --
  line, method and exception -- with their handlers and the breakable
  lines' cache ("Breakable lines"; [debugger.md](debugger.md), "The
  JetBrains plugin").

### Indentation and width

The formatter has one style and no options: two spaces per level,
continuation lines at +4, never a tab, 100 columns
([formatting.md](formatting.md), rules 1 and 12). The editor therefore
takes these values for every `.ns` file rather than offering settings
Reformat Code would ignore: there is no Code Style page for Newspeak, and
neither the user's code style, the detected indents nor an `.editorconfig`
reaches a Newspeak file. `editor/NewspeakCodeStyle.kt` holds the values and
their hooks:

- `NewspeakIndentOptionsProvider`, a `FileIndentOptionsProvider` ordered
  first, answers indent 2, tab size 2, continuation 4 and spaces only. Tab,
  smart Backspace and the editor's tab width read it. Its status-bar entry
  shows "2 spaces" with the hint that the formatter fixes it, and offers no
  action -- not "Configure Indents for Newspeak", which would open settings
  that change nothing.
- `NewspeakCodeStyleModifier`, a `CodeStyleSettingsModifier` ordered first,
  claims every Newspeak file. Only the first modifier that claims a file is
  asked, and an `.editorconfig`'s modifier ignores the indent options
  providers, so without it a `[*]` section's `indent_size` and
  `max_line_length` would reach `.ns` files. It leaves the indentation to
  the provider (`acceptsFileIndentOptionsProviders`) and sets the file's
  right margin to 100, which the editor reads for its margin guide and soft
  wraps. The platform takes a language's own right margin otherwise only
  from a `LanguageCodeStyleSettingsProvider`, which brings a Code Style page
  with it. The interface is `@ApiStatus.Experimental`, the one experimental
  API the plugin uses.

Enter, smart Backspace and a typed closer place a line as the formatter
would, from the editor's tokens alone -- the plugin has no Newspeak parser,
and the formatter's own answer comes only from Reformat Code
(`editor/NewspeakIndentation.kt`):

1. After a line whose last code token, comments after it aside, is an
   opening `(`, `[` or `{`, or the `|` closing a block's parameters
   (`[:each |`, `[:key :value |`, a parameter's type included), the new
   line stands at the opener line's indentation + 2. A `|` of temporaries
   or slots is no block's parameters.
2. Enter between an opener and its closer with nothing between them on the
   line -- `= (<caret>)`, `[<caret>]`, `{<caret>}`, and `[:each |<caret>]`
   -- gives three lines: the opener's, the caret's at +2, and the closer's
   at the opener line's indentation.
3. A closer typed as the first character of a line moves to the
   indentation of the line holding its matching opener; brackets of every
   kind are counted, as the formatter's tree nests them.
4. Any other line keeps the indentation of the line before it, blank lines
   skipped. No continuation indentation is guessed, since telling a broken
   message from a statement needs the grammar.

A string or comment spanning lines is never re-indented: inside one, Enter
does what the platform does everywhere (the line break copies the current
line's indentation), and a line that continues one is measured from the
line it starts on. "The opener line's indentation" is that line's leading
whitespace; where the opener stands on a continuation line, the formatter
measures a block's body from the statement instead (rule 13), and Reformat
Code corrects it.

The hooks: `NewspeakLineIndentProvider`, a `LineIndentProvider`, is what
`EnterHandler` asks for the new line and smart Backspace for the line it
unindents, in place of a formatter -- Newspeak has no
`FormattingModelBuilder` (see "Formatting"). `NewspeakEnterBetweenBracketsDelegate`,
an `EnterBetweenBracesDelegate` for the language, makes the platform's
`EnterBetweenBracesFinalHandler` open the pair: the platform's default takes
only `()` and `{}` and places the closer through a formatter; this one takes
`[]` and `|]` too, places the closer through the line indent provider and
reads the editor's highlighter instead of committing the document.
`NewspeakClosingBracketTypedHandler`, a `TypedHandlerDelegate`, moves a
typed closer: the platform does it for `)` and `}` through a formatter only,
and for `]` not at all. All of them read the editor's highlighter, which is
up to date while typing, without a document commit.

**Enter in a `///` comment continues it** (`editor/NewspeakLineCommentEnter.kt`,
an `EnterHandlerDelegate` asked before the platform's): the new line starts
with `///` and the spacing after this line's `///` (one space after a bare
`///`), at the end of the line as well as in its middle, the text after the
caret carried on and the spaces around the split dropped. `///` is the
comment form ns-next's sources are written in, and its comments run over
several lines, as Rust's `///` do, where Enter continues them too; the
platform's own handler continues a line comment only when text follows the
caret, Java's choice for `//`. A `///` after code on its line continues
only when Enter splits its text, which would otherwise fall out of the
comment into code; at its end Enter starts a plain line. Enter before the
slashes, and in a `(* … *)`, is a plain Enter.

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
gets its schema in the IDE, and can be run and benchmarked from it; so can a
single module ns-next can run from its source, from its `main:args:`.

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
schema provider, the play buttons of `newspeak.json` and its context-menu
producer, all of which read JSON PSI, so an IDE without the JSON plugin
still loads this one, with neither the schema nor those play buttons. The
module's play button and its producer read no JSON PSI and are registered
in `plugin.xml`, so they are there either way. The build compiles against it
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
configurations. A configuration names its **program** (the option
`program`, stored system-independently): a project's `newspeak.json`, whose
directory ns-next is given, or a `.ns` module, whose file ns-next is given
(`ProgramKind`; a file named `newspeak.json` is a project, a name ending in
`.ns` a module). No older option name is read: the plugin is unpublished,
so only local configurations ever held one. It runs `ns-next run` or `ns-next bench` ("Run" or
"Benchmark" in its form) on it, in the program file's directory for both
kinds, with an optional script, extra arguments, and for `bench` the
iterations (ten by default, at least one): `ns-next <run|bench>
[--script=<name>] [--iterations <n>] <project directory | module file> [--
<arguments>]` (`newspeakCommandLine`). The form's "Program" field chooses a
`newspeak.json` or a `.ns` file. The script is one token, so a name starting
with `-` is not read as an option; an empty script field means no script, as
a script's name is never empty nor surrounded by whitespace
([image-format.md](image-format.md), "Scripts"). A script belongs to a
project, as the CLI refuses `--script` with a source file: the form's Script
row is disabled while the program names a `.ns` module and applying the form
drops the script, and a configuration that names a module and a script --
stored by hand -- is refused by the check ("A script belongs to a project's
newspeak.json: ns-next refuses --script with a .ns module"). A relative
program file is the IDE project's, resolved against its base directory, both
to run it and to recognise the configuration from the file. The arguments
are one field, split as the platform splits program parameters
(`ParametersListUtil`), and go after `--`. A program that is missing, names
a file of neither kind, or is a text that is no valid path is refused before
anything starts ("Choose the program: a project's newspeak.json or a .ns
module"), and so are fewer than one iteration.
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

**The module's play button.** `NewspeakModuleRunLineMarkerContributor`, a
`RunLineMarkerContributor` for the language `Newspeak`, puts the same play
button, with the same "Run" and "Benchmark" (`NewspeakRunAction`), beside
`main:args:` of a module ns-next can run from its source, and nowhere else.
`ns-next run <file>.ns` builds the root class with `new` and sends the
instance `main:args:` as a public send (`crates/ns-cli/src/program.rs`,
`instantiate_application`), so a protected or private `main:args:` -- and
one without a modifier is protected (§6.3.1) -- answers
`doesNotUnderstand:`, and a class-side one is never sent. Whether a module
is runnable is read from the server's document symbols (`runnableMain`), not
from a parser of the plugin's own: the root class, the answer's top-level
symbol, has a `Constructor` named `new`, which the server names a written
and an omitted `new` alike, and a `Method` whose `detail` is exactly
`public` -- `public class` is class-side -- and whose name, the full pattern
the server writes (`main: platform args: arguments`), has the selector
`main:args:`. The analyzer test
`a_runnable_root_shows_its_factory_new_and_a_public_instance_side_main`
pins those names. A project's main module, built with `packageUsing:`, has
no button: it runs from its `newspeak.json`. The button stands on the
keyword `main:` where `main:args:`'s `selectionRange` starts, the symbol's
UTF-16 position converted through the document, and its tooltip is `Run
module '<root class>'`. A daemon pass runs in a read action, which every
write action, typing included, waits behind, so the contributor never waits
for the server: it reads `DocumentSymbolCache.current`, and with no answer
for the current stamp it calls `request`, which asks on a pooled thread --
again every second, for about a minute, while the server has no answer --
and, once an answer is kept, restarts the daemon for that file
(`DaemonCodeAnalyzer.restart`, skipped for a file no longer valid); the next
pass places the button. One request per file and stamp is in flight; a
second for the same stamp is not called back, since every caller restarts
the same daemon. After an edit the button is therefore gone until the
server has answered the new text; keeping the stale answer was rejected,
since after an edit above `main:args:` it would put the button on the wrong
line. A configuration created from the button is named `<file>.ns`, with
` (benchmark)` appended for `bench`, and is temporary, reused while equal
(the same program file, no script, the same command) and made unique as for
a project. "Run" in the context menu of a runnable module, in the project
view and anywhere in its editor (`NewspeakModuleRunConfigurationProducer`,
registered in `plugin.xml`), creates the same "Run" configuration. It runs
once per gesture, so off the EDT, with no current answer, it asks the
server itself (`DocumentSymbolCache.fetch`), as the structure view does; on
the EDT only a kept answer counts. Recognising its configuration needs no
symbols: one is the module's when it names the file, with no script and
`run`.

**Checked under `runIde` (2026-10-05)** with a module printing through
`platform transcript`: the button beside `main:` with Run and Benchmark,
Run printing its output and Benchmark running its ten iterations; after a
line typed above `main:args:` the button back on the moved line with no
further edit; no button for a `main:args:` without `public` nor for a root
whose factory is `usingPlatform: platform`; "Run" in the context menu of
the file in the Project view and in its editor; the Script field disabled in
the form for a `.ns` program; and a project's `newspeak.json` running as
before.

Decided with the user on 2026-10-05: the plugin runs a project with the
`ns-next` the user chooses, and bundles none; a run configuration type
"Newspeak", with the plugin's file icon, groups the configurations; a play
button beside `main` and each script opens Run and Benchmark. Chosen beside
them: the platform's run marker as the gutter icon; temporary configurations
from the gutter, named as above; the extra arguments as one field split as
program parameters; the JSON plugin as an optional dependency. Decided with
the user on the same day: a play button beside `main:args:` of a module
ns-next can run from its source, and "Run" in its context menu, read from
the document symbols the structure view shares through
`DocumentSymbolCache`; the run configuration's program a `newspeak.json` or
a `.ns` module.

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
  respectively; the lexer comparison depends on the debug build and points
  its test at it, and `buildRuntimeDebug` builds `ns-next` (debug) for the
  DAP session test and the debug process test. Every sandbox an IDE starts from depends on the release build and gets
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
  against the Kotlin lexer over every `.ns` in `corpus/modules`, `goldens/`,
  the core library (`crates/ns-runtime/core-library/`) and `benchmarks/`:
  kind and span of every token must agree, and the Kotlin lexer's whitespace
  and comment tokens must fill exactly the gaps between the Rust tokens.
  `dump-tokens` runs in batches of 100 files, which keeps each command line
  under Windows' length limit over the repository's more than 600 `.ns` files.
  It is a test task of its own, `lexerComparisonTest`, which `check`
  depends on beside `test`: its inputs are those `.ns` files -- not a
  golden's `.toml`, which the comparison does not read -- and what
  `dump-tokens` answers derive from, `ns-syntax`'s sources, the analyzer's
  `main`, its dispatch and `token_dump.rs`, the processor check and the
  reach record `main` runs first, and the workspace's build settings (the
  toolchain, the compiler flags, `Cargo.lock` and every manifest), so a
  change to the Rust lexer reruns it rather than leaving `check` up to
  date. The debug server is built before it, but its bytes are no input:
  it is relinked whenever `ns-runtime` changes, and the gates' own builds
  relink it at every run, which would rerun the comparison on every runtime
  commit though nothing it reads changed. A later test that starts a
  workspace binary takes a task of its own the same way, declaring the
  sources its answers run through and `binaryBuildSettings`
  (`editors/jetbrains/build.gradle.kts`);
- the DAP client against the real agent (`DapSessionTest`) --
  `ns-next run --debug-connect` connecting to a socket the test listens
  on, then `initialize`, `attach`, `setBreakpoints`, `configurationDone`,
  the `stopped` event, the top frame of `stackTrace` and `continue` to the
  `exited` event's 0, every wait bounded; the debug build of `ns-next`
  (`buildRuntimeDebug`, `cargo build -p ns-cli`) is named by the system
  property `nsNext.runtime`. A task of its own, `dapSessionTest`, which
  `check` depends on: its answers run through the whole runtime, so its
  inputs are the sources of `ns-cli`, `ns-runtime` (the core library
  included), `ns-tooling`, `ns-syntax`, `ns-memory` and `ns-assembler`
  and `binaryBuildSettings`: every change to the runtime reruns it, while
  a change to the analyzer alone leaves it up to date; its framing and bookkeeping, piped to a scripted
  peer instead, are `DapConnectionTest`'s, in plain JUnit under `test`
  (UTF-8 byte counts, header names in any case beside other headers, a
  body arriving in pieces, a failed response failing its future, events
  by name, a close or the stream's end failing every pending request, a
  peer closing inside a body, an oversized `Content-Length` and malformed
  JSON ending the connection, an `Error` the message handler throws
  ending it, an `Error` an event listener throws and a failing close
  listener leaving the other listeners called, and a stream that throws
  something other than an `IOException` failing the send and its
  request);
- the debug process in the platform's debugger
  (`NewspeakDebugProcessTest`, on `BasePlatformTestCase` run off the event
  dispatch thread, so that it can wait while the platform updates the
  session there) -- Debug on a Newspeak run configuration through the
  plugin's runner, against the same debug build of `ns-next`: a line
  breakpoint toggled through `XDebuggerUtil` stops the program, the
  session's position is its line and the top frame's locals and an
  `Array`'s elements read through the value tree; Step Over moves to the
  next line; the first Stop of a program inside one long leaf terminates
  through the agent and leaves the process running, the second kills it;
  and, against a fake agent over the same harness (a session over an
  undebugged `ns-next` process of the test's own): an agent reporting
  another process's id, and one sending `initialized` and `stopped`
  without any `process` event, ends the session with the error and kills
  the process before any configuration is sent; `stop()` keeps the
  connection until `terminate` is answered; a module shown by reference
  is one read-only `LightVirtualFile` across two stops, asked for once;
  a hidden frame is drawn greyed and the program's frame not; and an
  `Array` of 150 elements shows 100, offers 50 more, and fetches them
  with `start` 100; an agent's `terminated` event or a close before any
  `process` event ends the session with the error and kills the process,
  and Run to Cursor before the runtime connected says the session has not
  started.
  Every listener is registered as the platform creates the debug process
  (`XDebuggerManager.TOPIC`), before it connects, and every wait is
  bounded. A task of its own, `debugProcessTest`, with
  `dapSessionTest`'s inputs (`debugsWithTheRuntime` in
  `build.gradle.kts`), which `check` depends on;
- the breakpoint types (`NewspeakBreakpointTypesTest`, as
  `NewspeakDebugProcessTest` off the event dispatch thread) -- with the
  breakable lines' fetcher replaced, `canPutAt` refuses a comment's line,
  allows a statement's, allows every line for a `null` answer and while
  no answer has come; a line's variants are the line and its block, the
  block's highlighted range and column the server's; and, through each
  handler over a scripted peer and a session that records what it is
  told: a pass count is sent as the `hitCondition` and a block variant's
  column as the `column`, a breakpoint the agent moves says the line it
  stops at and one it refuses its reason, a method breakpoint is sent by
  its name and pass count, and the exception breakpoints as the filters
  and the classes' options, "Any exception" for uncaught ones taking
  every class; against the real runtime, a temporary breakpoint in a loop
  stops once and is gone. A task of its own, `breakpointTypesTest`, with
  `debugProcessTest`'s inputs, which `check` depends on;
- restart (`LexerRestartTest`) — lexing from any token boundary answers the
  same tokens, the platform test framework's standard check;
- the highlighting lexer (`NewspeakHighlightingLexerTest`) — which `///`
  lines merge into a doc comment (a block of two and of three, at the
  text's start and end, and not a single line, a block split by a blank
  line or code, a trailing remark, or a line beside a `(* *)` comment), the
  state of each token, restarts over the goldens, and, over every golden,
  benchmark and core-library module, the parsing lexer's tokens wherever it
  merges nothing and a block's lines where it does;
- plugin tests on `BasePlatformTestCase` (`NewspeakEditorTest`) — file type,
  brace matching, commenting; `NewspeakOneLineMethodFoldingTest`, the
  one-line methods' folds (which bodies fold and which stay open, the
  ranges and texts of Java's layout, the width, the setting, and the
  server's fold of the body created beside them in either order); `NewspeakIndentationTest`, the editor's indentation and
  width ("Indentation and width"): Enter after each opener, after a block's
  parameters with and without a type, after an opener with a comment behind
  it, but not after a temporaries' bar or an opener inside a string or
  comment; Enter between each pair and inside `[:each |]`; each closer typed
  first on its line moved, and one after code or inside a string left; a
  line kept at the one before it, past a blank line and a string spanning
  lines; Enter inside a string not re-indented; Tab inserting two spaces,
  also under settings of eight and tabs; smart Backspace in the
  indentation, also of a doc comment's line, and Enter after a doc comment
  at its last line's indentation; and the editor's right margin at 100 where the project's is
  120; `NewspeakLineCommentEnterTest`, Enter in a `///` comment (at its end,
  in its middle, after a bare `///`, with wider spacing, in a body, after
  code at its end and in its middle, before the slashes, and in a block
  comment) and in a doc comment (at the end of its first and of its last
  line, in the middle of a line, in a body, and before a later line's
  slashes); `NewspeakSyntaxHighlighterTest`, which ranges the editor's
  highlighter colours as a doc comment (the cases of the lexer's test), and
  the highlighter kept equal to a fresh lex while typing: a line typed
  under a single one and above it, by Enter and slash by slash, two lines
  joined across a blank line, the code before a line deleted, a line typed
  under a remark that came to start its line, and a block broken by a line
  deleted, by its slashes broken and by code typed before either line;
  `NewspeakEditorTest` also finds a TODO in a doc comment through the TODO
  index; `NewspeakDocCommentAgreementTest`, the PSI's blocks
  (`NewspeakDocComments`) and the highlighter's doc-comment tokens the same
  ranges over every golden, benchmark and core-library module and at the
  rule's edges, since the rule is stated twice; `NewspeakInlineDocumentationProviderTest`,
  the rendered doc comments (every block and no single line, the Markdown
  rendered, the tags as the section table with their code rendered and
  each item a paragraph of its own, the owner from the server's symbols
  and none without them, an item found by its range, nothing for another
  language's file); `NewspeakHoverSectionsTest`, the server's section table
  through the platform's converter, its classes kept and its cells'
  Markdown rendered;
  `NewspeakColorSettingsPageTest` is plain JUnit,
  over the colour settings page directly, with no platform test case needed
  either — like `ServerBinaryLocatorTest` and `NewspeakSemanticTokensTest`,
  which cover the binary lookup order and the semantic-token classification,
  and `NewspeakDocTagsTest`, the doc comment's grammar case by case, each
  test named as the Rust test of the same case (the Rust spans aside), with
  the summary's Unicode whitespace and the sections in their order and none
  without a known tag;
- the structure view, in plain JUnit: `NewspeakSymbolKindsTest` (the icon
  per kind, every kind the server sends named),
  `NewspeakStructureElementTest` (a row's name, `lazy`, access icon, the
  base icon and hover of each element in "The rows", root, root value and
  nested value classes included, and `class` read together with the access
  word and `lazy`; beside it, on `BasePlatformTestCase`,
  `NewspeakStructureElementSummaryTest`: a hover with the summary of the
  member's doc comment, the summary following an edit of its comment, and
  a member without one keeping the kind's hover) and
  `NewspeakStructureViewModelTest` (no answer and an
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
  project file's absolute directory, a module given as its file and run in
  its directory, run and benchmarked, the form giving back what it was shown,
  storing a system-independent path and dropping a module's script, the
  errors for no runtime, a missing runtime file, a missing or invalid
  program, a program of neither kind, a module with a script and too few
  iterations); `DocumentSymbolCacheTest` (an answer current until the
  document changes, an empty answer not kept, a request called back once
  its answer is kept, asking again while there is none, and two requests
  for one stamp asking once);
  `NewspeakRunLineMarkerContributorTest` (a play button beside `main` and each
  script and nowhere else, none in another JSON file, a menu of "Run" and
  "Benchmark", a configuration created without starting it, reused on a
  second click and when stored with backslashes, and the context menu's
  "Run" for the script under the caret or the project);
  `NewspeakModuleRunLineMarkerContributorTest`, over a cache filled by a
  stand-in for the server (`runnableMain` for `new` and for
  `packageUsing:`, a private, a class-side and a missing `main:args:`, a
  renamed parameter, another selector and an empty answer; Run and Benchmark
  beside `main:` of a runnable module, also past a character outside the
  BMP, and none for a protected `main:args:`, another factory or before the
  server has answered; the context menu's "Run" from the file and from inside a
  method, named `<file>.ns` and recognised from the gutter's
  configuration, none for a module that cannot run, and off the EDT asking
  the server itself). No test starts `ns-next`.

**Gates.** There is one suite: `cargo nextest run --workspace` runs
`ns-analyzer`'s tests, the hints' behaviour oracle aside ("Hints"), before
every commit and under every stress mode, like every other crate's (`CLAUDE.md`, "Working in this repository"). Besides the
language server's, they are the tests of `disasm` and `inspect`, which guard
the compiler's output (the disassembly snapshots) and the image format (an
image decoded and listed, its bodies matching the source's); under the wide
stress mode the analyzer's subprocess compiles every instruction wide, so they
also check that disassembly and decoding do not depend on the form. Which
gate runs when is the table "The gates" in `CLAUDE.md`: for the editor, the
plugin's `check` on every commit (Gradle reruns its tests only when the
plugin, a source they read or, for the lexer comparison, the Rust sources
`dump-tokens` runs or the build settings changed),
the hints' behaviour oracle for a commit touching `crates/ns-analyzer/`
("Hints"), and `verifyPlugin buildPlugin`, which downloads several IDEs, at
the end of a batch that touched `editors/jetbrains/`. A change to
`crates/ns-syntax/src/lexer.rs` must be mirrored in `NewspeakLexer.kt` in the
same commit, or the comparison fails.

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
  ("Structure"), and Reformat Code reaching the server's formatting and
  applying its edit ("Formatting"). The sandbox
  also disables the ultimate marker plugin for `runIde`, not only for tests.
- **The hints' path into the IDE** -- a hint shown as a weak warning, its
  quick fixes and the whole-file action on Alt+Enter -- is covered by the
  check by hand under `runIde` ("Hints") and by no automated test.
- **Untested paths:** `dump-tokens` on an unreadable file (exit status 1); a
  block comment over a whole-line selection; TODO markers through the
  project-wide TODO index rather than a single open file; a Newspeak run
  configuration starting `ns-next` and showing its output, which the tests
  stop short of (they check the command line and the process's charset);
  the module's play button against a running server, the daemon restarted
  by the answer (the tests fill the cache through a stand-in).
- **The debugger's paths into the platform** without a test: the real
  `newspeak/breakableLines` round trip through the platform's LSP client
  (the breakpoint types' test replaces the fetcher), a breakpoint in a
  shipped module's read-only file, and the turning off and on that
  carries an edited pass count or exception box to a running session
  ([debugger.md](debugger.md), "The JetBrains plugin"); and the plugin
  starts no image, so `ns-next load --debug-connect` has no run
  configuration of the plugin's.

## Planned later

- **Range formatting and formatting on type.** `textDocument/rangeFormatting`
  and `textDocument/onTypeFormatting`, for the selection or the line just
  typed. The printer lays out a whole module from its tree, so a range
  needs the enclosing members printed and only their edit answered, and a
  half-typed line does not parse at all, which the printer refuses
  ([formatting.md](formatting.md), "What is deliberately not done").
- **Navigation.** Go to definition and find usages need an AST for a file
  that does not parse, while it is typed; the fault-tolerant parse exists
  ([frontend.md](frontend.md), "Recovery"). What remains is `definition` and
  `references` in the server.
- **More platforms.** A CI build producing the other five binaries into
  `bin/<os>-<arch>/`. The repository has no CI yet.
