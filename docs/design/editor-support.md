# Editor support — the language server and the JetBrains plugin

> **Status:** **Built:** the language server (`crates/ns-lsp/`, and
> `resolve_for_editor` in `crates/ns-runtime/src/resolver/mod.rs`) and the
> JetBrains plugin (`editors/jetbrains/`) both exist: highlighting and errors
> as you type, in every commercial JetBrains IDE from 2026.1.4. Formatting,
> navigation and further platforms are planned ("Planned later").

Editing `.ns` files in a JetBrains IDE today means editing plain text. This
document describes the editor support that replaces it: syntax highlighting as
you type, and the errors `ns-next` would report before running a module, shown
in the editor at the place they occur.

## Scope

**In scope.** Two capabilities, in every commercial JetBrains IDE:

1. **Highlighting** — keywords, strings, symbols, numbers, comments,
   selectors, and, once the server has analysed the file, what the resolver
   knows about each name: parameter, local variable, slot, nested class,
   method, `self` / `super` / `outer`.
2. **Errors as you type** — every lexer, parser and resolver diagnostic, with
   the message `ns-next` itself produces.

**Target IDEs.** Every commercial JetBrains IDE that carries JetBrains' own
LSP API: IntelliJ IDEA, WebStorm, PhpStorm, PyCharm, DataSpell, RubyMine,
CLion, DataGrip, GoLand, Rider and RustRover. The free editions and Android
Studio do not carry that API and are not a target. The plugin is written so it
can be published to the JetBrains Marketplace at any time, even while it has
one user: complete metadata, a verifier-clean build, third-party notices, and
no machine-specific paths.

**Out of scope, for now.** Navigation (go to definition, find usages, the
structure view), completion and formatting. Each is described under
"Planned later", with what it would need.

## Architecture

```
crates/ns-lsp/          binary `ns-next-lsp`: diagnostics and semantic tokens
                        over LSP, and `dump-tokens` for the lexer comparison
crates/ns-runtime/      + a resolver entry answering bindings and errors together
editors/jetbrains/      Gradle project: file type, Kotlin lexer, highlighter,
                        LSP client, settings
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
every change.

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
  documents use.
- `server.rs` — the message loop over `lsp-server`, synchronous on one thread.
  Lexing, parsing and resolving one file takes milliseconds, so neither a
  debounce nor a worker pool buys anything. Open documents are keyed by the
  URI's string form, not by `lsp_types::Uri` itself: `Uri` wraps a
  `fluent_uri::Uri<String>` that caches parsed components behind a `Cell`, so
  clippy's `mutable_key_type` refuses it as a map key even though its
  `Hash`/`Eq` only ever read the string. A `semanticTokens/full` request for a
  URI the server never opened, or has since closed, answers an empty token
  set rather than an error.
- `documents.rs` — the open text per URI with its version, and a line index
  converting a `Span`'s byte offsets to LSP positions in UTF-16 code units,
  the one encoding every LSP client must accept.
- `analysis.rs` — `lex`, `parse` and the resolver entry below, run once per
  document version and cached.
- `semantic_tokens.rs` — classification and the LSP encoding.

**Protocol surface.** `initialize`, `textDocument/didOpen`, `didChange` (full
text on every change), `didClose`, `publishDiagnostics` (push, supported by
the platform since 2023.2), `textDocument/semanticTokens/full`, `shutdown`
and `exit`. Nothing else.

### The resolver entry

`resolve` answers either every binding or `Err(errors)`, although it collects
both. The server needs both at once, so that a file with one resolver error
still gets semantic tokens everywhere else. `ns_runtime::resolve_for_editor`
answers both together as an `EditorResolution` (module, bindings, errors);
`resolve` remains the entry everything that compiles uses. `resolve_for_editor`
never records `Binding::MirrorSend` — it goes through the same walk as
`resolve`, never through `resolve_kernel` — so the VM mirror's capability
invariant (`CLAUDE.md`, "The VM mirror is a capability") is not touched, and
`Kernel.ns`, opened in the editor, shows its `vmMirror` sends as ordinary
sends.

`Kernel.ns` resolves without errors this way: the `kernel` flag decides only
whether a send to `vmMirror` is marked (`resolver/mod.rs`, `resolve_send`),
never whether anything is an error — pinned by
`the_editor_entry_resolves_the_kernel_without_the_mirror_mark`.

### Classification

Legend: token types `parameter`, `variable` (a local), `property` (a slot),
`class`, `method`; modifiers `declaration` and `readonly` (a slot or local
declared with `=`). There is no `keyword` type: the reserved words (§4.1:
`self`, `super`, `outer`, `true`, `false`, `nil`) are coloured by the plugin's
lexer, which knows them from their text, so the server adds nothing for them.
A keyword message's span starts at its receiver, so the receiver's span is
excluded when the selector's tokens are found.

- **Declarations** come from the AST: class names, slots, message patterns,
  parameters, temporaries.
- **Uses** come from the bindings. `Local` and `OuterLocal` are a parameter or
  a local according to the declaration they bind to. The resolver records a
  slot read as a `LexicalSend { depth }`, as the specification makes it a send
  like any other; the server looks the selector up among the declarations of
  the class `depth` levels out — slot, nested class or method — and colours it
  accordingly. A name that class only inherits cannot be placed statically and
  is coloured `method`, which is what it is: a send.
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

While a file does not parse there is no AST and so no semantic tokens: the
colours fall back to the lexer's until it parses again. A fault-tolerant parser
("Planned later") would change that.

### Robustness

An editor sends invalid text all the time. `ns-syntax`'s contract is that
malformed source produces diagnostics, not a panic; the server nonetheless runs
each analysis under `catch_unwind`, turns a panic into one "internal error"
diagnostic at the start of the file, and keeps running. A panic there is an
ns-next defect and must be visible, but must not take the editor's server down
with it. The release profile unwinds (`Cargo.toml` deliberately does not set
`panic = "abort"`), which this relies on.

### Testing

`cargo test -p ns-lsp`:

- positions — byte offset to UTF-16 with non-ASCII text, umlauts and emoji
  (surrogate pairs), at line ends and at the end of the file;
- diagnostics — one `indoc!` module each with a lexer, a parser and a resolver
  error, checking range and message;
- semantic tokens — a small module per category (parameter, local, slot with
  and without `readonly`, nested class, declared method, inherited send,
  a receiver inside a keyword message's span), each expected category asserted
  directly rather than through an `insta` snapshot, so every expectation is
  stated and reviewed rather than accepted;
- the resolver entry — for every module in `corpus/modules`, `goldens/` and
  `Kernel.ns`, the same bindings and errors as `resolve`, so the two cannot
  drift;
- robustness — no analysis over that same set produces an "internal error",
  nor over truncations of each file at token boundaries, which is what a file
  looks like while it is being typed: every token boundary of every golden and
  of the kernel, and 64 evenly spaced token boundaries of each corpus module,
  since every prefix of every corpus module is quadratic in its size;
- protocol — one end-to-end test over an in-memory `lsp-server` connection:
  `initialize`, `didOpen`, the expected `publishDiagnostics`,
  `semanticTokens/full`, `shutdown`.

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
is where the server's semantic tokens are wired in, over `LspSemanticTokensSupport`;
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
  keys, so each one can be configured.
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
  project.
- **Settings** — *Languages & Frameworks → Newspeak*: a server path, empty by
  default, typed or chosen with a file picker, and a "Restart server" action. Stored at application level: the
  path is specific to a machine, so it does not belong in `.idea/`.

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
so the two always match. At first only `windows-x64` is bundled; the layout is
per platform so that `windows-arm64`, `macos-x64`, `macos-arm64`, `linux-x64`
and `linux-arm64` are added later by a CI build placing their binaries in the
same tree, with no change to the plugin's code.

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
  the binary in `bin/windows-x64/`, beside `THIRD-PARTY-NOTICES.txt` and
  `LICENSE`: `prepareSandbox`, which `buildPlugin` packages, and
  `prepareSandbox_runIde` with its split-mode siblings, which `runIde` starts
  from — a separate sandbox (`plugins_runIde/`), so configuring
  `prepareSandbox` alone leaves `runIde` without a server. The test sandboxes
  (`prepareTest…`) are left out: only these build `--release`, because the
  release profile's fat LTO and single codegen unit make that build slow and
  the tests use the debug build.
- `runIde` starts a sandbox IDE with the plugin, for trying it by hand. It has
  been tried end to end: the server starts from `plugins_runIde/`, the
  semantic colours land over the lexer's, and a resolver error
  (`^outer Missing foo`) is underlined with ns-next's message.
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
  which cover the binary lookup order and the semantic-token classification.

**Gates.** The runtime's per-commit suite is `cargo test --workspace
--exclude ns-lsp`, so runtime work does not pay for the editor's corpus-wide
tests, with `cargo test -p ns-lsp` its own per-commit gate for a change that
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
  groups, and `./gradlew -p editors/jetbrains verifyPlugin buildPlugin` —
  which downloads several IDEs, and so is not a per-commit gate. A resolver
  change that shifts the semantic tokens is then noticed within one batch.

## Known gaps

Reviewed and deliberately left for later; none affects a module that parses
and resolves cleanly in a JetBrains IDE today.

- **The IDE path has no automated test.** It was checked by hand under
  `runIde` (see "Build and packaging"); nothing but that check covers the
  platform handing `NewspeakLspIntegrationProvider` its descriptor
  (`PluginAware`) or the sandbox layout `runIde` starts from. The sandbox
  also disables the ultimate marker plugin for `runIde`, not only for tests.
- **Warning-severity diagnostics.** `ns_syntax::parse` answers a module or
  errors, so a lexer or parser warning on a module that parses is not shown.
  None exists today.
- **A name declared on both sides of a class** with different kinds (a slot on
  the instance side, a method on the class side) is coloured by the first
  declaration, as `ClassMembers` merges both sides the way the resolver's
  member set does.
- **A malformed notification** is logged to stderr and ignored, so the client
  sees no error for it.
- **Untested paths:** `dump-tokens` on an unreadable file (exit status 1); a
  block comment over a whole-line selection; TODO markers through the
  project-wide TODO index rather than a single open file.
- **Two copies of the platform-directory mapping:** `hostPlatform` in
  `build.gradle.kts` falls back to `linux`/`x64` for a host
  `ServerBinaryLocator.platformDirectory` does not name, and `plugin.xml`'s
  description says the server is bundled for Windows x64 whatever host built
  it. Both matter once the CI build for other platforms exists.

## Planned later

- **Formatting.** The native LSP API supports `textDocument/formatting` since
  2023.3; once the server announces it, the IDE's Reformat Code uses it with no
  change to the plugin. A platform-native formatter (`FormattingModelBuilder`)
  would need a PSI tree, which is the second parser this design avoids. The
  work is in Rust: the AST drops comments today, and a formatter must keep
  each one where it was, so `ns-syntax` would have to hand out comments with
  their spans, as tokens or as trivia attached to neighbouring tokens. The
  formatter would be one module that an `ns-next fmt` command shares, so the
  editor, the command line and any later formatting gate use one formatter.
  It would not touch a file with a syntax error.
- **Navigation.** Go to definition, find usages and the structure view need an
  AST for a file that does not parse, while it is typed: a fault-tolerant
  parser in `ns-syntax`, then `definition`, `references` and `documentSymbol`
  in the server. The same parser would keep semantic tokens alive through a
  syntax error.
- **More platforms.** A CI build producing the other five binaries into
  `bin/<os>-<arch>/`. The repository has no CI yet.
