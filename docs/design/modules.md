# Modules: identity, the manifest, metadata

How a program's modules are found, named and wired. The language has no
global namespace (§1, PDF pp. 3-4; §6.4, PDF p. 45): a module declaration is
a top-level class, a module declares what it needs through its factory's
parameters -- "These act as 'imports'" (§6.3, PDF pp. 42-43) -- and the
application instantiates and links the modules in `main:args:` (§8.4,
PDF p. 48). Everything here is outside the language: which files a program
has, which of them the application asks for, and how tools and images name
them.

Status: built. Ids, versions, ranges and a module's declared identity
are `ns-runtime`'s `modules.rs` (`ModuleId`, `Version`, `VersionRange`,
`ModuleIdentity`, `declared_identity`); metadata comments are collected by
their §4.3 targets in `ns-syntax`'s `metadata.rs` (`metadata_of`); each
module's identity is held by its root mixin (`Mixin::module_identity`);
the manifest is asked with strings, `newspeak.json` takes an `as`, and the
project's refusals stand before anything runs (decisions 1-5 and 9;
[core-library.md](core-library.md), "The manifest";
[image-format.md](image-format.md), "Packaging"); an image records each
module's path, id and version, held to the project's rules by the decoder
(decision 10; [image-format.md](image-format.md), "Module section",
"Verification"); the debugger, the DAP agent, the MCP server and the
JetBrains plugin name a module by its id and version and a class inside
it after a `/` (decision 8; [debugger.md](debugger.md), "The agent's
sources"); and the analyzer reports a malformed `:module:` and shows a
module's and a parameter's in its hover
([editor-support.md](editor-support.md), "Module metadata"). Several
versions of one module live in one running program side by side
(`goldens/tool/manifest/two_versions_of_a_module_run_side_by_side`).

## The three roles

| Role | Who | How |
|---|---|---|
| What a module *is* | the module | metadata above its class: `(* :module: com.acme.json.Json@2.1.0 *)` |
| Which modules *exist* (resolution) | `newspeak.json` | paths, optionally with an `as` |
| Which module goes *where* (wiring) | the application | `manifest at: 'com.acme.json.Json' version: '2'` |

A module's own code names no other module: it declares a factory parameter
and is handed what the application chooses. This is the separation
Newspeak makes and the CommonJS `require` breaks, which "encourages code to
hardwire external dependencies" (`docs/references/newspeak-modules.pdf`,
p. 20).

## The decisions

Decided with the user on 2026-11-18 and 2026-11-19.

**1. A module's identity is its path relative to the project.** Tools and
images tell modules apart by the path of the source file relative to the
directory of `newspeak.json`, against which its paths are resolved
already, or, for a program that is one `.ns` file, relative to that file's
directory: `libs/util-1/Util.ns` and `libs/util-2/Util.ns` are two modules
whatever they declare; `..` reaches a library beside the project. It is
the one name every module has and two versions do not share. Every such
path ends in `.ns`, a one-file program's too, so `ns-next run Solo.txt` is
refused ([image-format.md](image-format.md), "Packaging"). A module on
another drive or share than the project has no path relative to it and is
refused ([OPEN-WORK.md](../OPEN-WORK.md)); a verbatim path, `\\?\C:\…`,
lies on the drive its plain form names. SOMns
identifies a module the same way, by its path relative to a base directory
and its class's name (`prior-art/somns`, `MixinDefinition.java:865-889`),
and loads two files declaring one class name side by side; canonical
Newspeak and Primordial Soup key one flat map by the class's name, the last
writer winning (`prior-art/newspeak/Namespacing.ns:35-82`,
`prior-art/primordialsoup/newspeak/CompilerApp.ns:53-56`).

**2. A module declares its id, fully, in metadata above its class.**

```newspeak
(* :module: com.acme.json.Json@2.1.0 *)
class Json usingPlatform: platform = Object new ( ... ) ( ... )
```

- The id is Java's kind of fully qualified name: identifiers separated by
  dots, the last the class's name, which it must equal -- a module whose
  `:module:` ends otherwise is refused, and the analyzer reports it -- so
  that the string the application asks for stands in the module's file
  word for word and cannot drift from the class silently. A reverse domain
  as namespace is the convention for libraries, not a rule.
- `@` separates the version, as npm (`json@2.1.0`) and Go (`module@v1.2.3`)
  write a coordinate; the same notation names a module in metadata, in
  messages and in tools.
- The version is semver 2.0.0, `MAJOR.MINOR.PATCH` with an optional
  pre-release (`2.1.0-beta.1`) ordered by semver's rules; build metadata
  (`+…`) is refused, since it orders nothing.
- The metadata stands immediately above the class: a module declaration is
  a class expression (§6.4) and a metadata comment immediately before an
  expression is that expression's (§4.3, PDF p. 21), as Primordial Soup
  places its modules' `:exemplar:` (`Collections.ns:1-3`). A `///` doc
  comment goes above it.
- **A module without metadata** has its class's name as its id and no
  version: `manifest at: 'Util'` finds it. A module with metadata is found
  by its full id only, never also by its class's name, or `at: 'Json'`
  would be ambiguous beside an unannotated `Json`.

**3. The manifest is asked with strings.** `Manifest.ns` answers
`at: id` -- the one module of that id, an `Error` `` no module `<id>` ``
when there is none and `` `<id>` names <count> modules; ask with
at:version: `` when there are several -- and `at: id version: range`,
whose `Error` for a range none of the id's versions matches names the
versions there are (`` no module `<id>` of version <range>; there are
1.0.0, 2.0.0 ``) and whose `Error` for a range of no range's form, `'2.'`
or `'02'`, is the loader's own, `` `<range>` is no version: <reason> ``,
the reason `VersionRange::parse` gives
(`crates/ns-runtime/tests/manifest.rs`,
`a_malformed_range_is_no_version`), and nothing through
`doesNotUnderstand:`: an id is any string, so a dotted one, and no key
collides with a selector `Object` answers, so no module is refused for
being named like one. Canonical Newspeak's unary `manifest Browsing` is
not kept (decided with the user: the string is the form, and the version
is a second argument beside it).

**4. A range is a prefix, and the highest version that matches is taken.**
`version: '2'` matches 2.x.x, `'2.1'` 2.1.x, `'2.1.0'` exactly; a
pre-release matches only a range that names it, so that `'2'` never picks a
beta silently. When several match, the highest is taken, as npm and Cargo
resolve, and a warning on standard error names the one taken and the ones
passed over: two versions of one major line in a project are likely a
mistake, which the choice must not hide.

**5. `newspeak.json` lists paths, and may give a module another id.**
`modules` stays a list, in the module order it has today:

```json
"modules": [
  "libs/json/Json.ns",
  "libs/util-1/Util.ns",
  { "path": "vendor/soup/Collections.ns", "as": "org.newspeak.soup.Collections" }
]
```

`as` replaces the id the module declares, under the rules of every id (its
last part the class's name, outside the reserved namespace); the version
stays the module's own or absent -- a version the project file claimed
could pick the wrong module within a range silently. It exists for code
written without this module system, which is all of canonical Newspeak's:
26 files of the Newspeak and Primordial Soup repositories share a name
(`prior-art/newspeak/tool/build.sh:124-126` copies one over the other), and
for a fork beside its original, which declares the original's id. Refused
before anything runs: two modules of one id with one version, or of one id
where either has none (no request could pick between them), and an id in
the reserved namespace.

**6. Metadata comments are parsed, generally.** `ns-syntax` recognizes
every `(*:tag: payload*)` (§4.3) as `Metadata { tag, payload, span }` beside
the comment it is, and attaches it as §4.3 says: to the expression, slot or
parameter it immediately precedes, to the method or factory whose pattern
it immediately follows, to the class whose header it follows, to the module
declaration it precedes. `:module:` is one reader of it. The lexer does not
change -- metadata are comments lexically -- and the formatter keeps them
where they are, which it does already: a factory of many annotated
parameters breaks one keyword a line (formatting rule 3). Exposing metadata
through the mirrors, as §4.3 asks, is open ([OPEN-WORK.md](../OPEN-WORK.md)).

**7. What a factory parameter expects is documentation only.**

```newspeak
class Reports usingPlatform: platform json: (* :module: com.acme.json.Json@2 *) json = ( ... )
```

The expectation is parsed and shown -- the language server's hover and the
factory's signature -- and checked by nothing, now or later: a mock, or
another implementation of the same interface, must always be passable.
"Alternate module implementations conforming to the same interface can
coexist" (`docs/references/newspeak-modules.pdf`, p. 21), and §4.3 keeps
metadata from burdening a running program.

**8. Tools write a module's id, `@` its version, and `/` before a class
inside it.** `com.acme.json.Json>>parse:` is the module's class's method,
`com.acme.json.Json/Parser>>parse:` a nested class's,
`com.acme.json.Json@2.1.0/Parser>>parse:` one version's;
`Util/Inner>>run` an unannotated module's nested class. A name without `@`
names every version -- a method breakpoint stands in each, a trace filter
passes each -- and output always writes the version a module has, so that
each stack frame and trace line names one module. The `/` ends the id
where the eye looks for it, and no lookup against the known ids is needed
to read a name: with dots throughout, a module `a.B` with a nested `C`
and a module `a.B.C` would read alike, and a class's name need not be
capitalized. It is the form of the debugger's frame names, its method and
exception breakpoints, the memory view's classes, the MCP server's
locations and the plugin's breakpoint types; the dotted form of before,
`Demo.Account>>deposit:`, is refused with the hint that a class inside a
module is written `Demo/Account`. Within a program an id and a version
name one module (decision 5's refusals), so no tool name holds a path; the
path maps a module to its source file.

**9. `newspeak.` is reserved for the modules the binary ships.** Today's
shipped modules declare `newspeak.core.Kernel`, `newspeak.core.PlatformFactory`,
`newspeak.core.TranscriptFactory`, `newspeak.core.TimeFactory`,
`newspeak.core.CollectionsFactory`, `newspeak.core.TypedArraysFactory`,
`newspeak.core.MirrorsFactory` and `newspeak.core.Manifest`, without a
version: they are the binary's, and an image is held to the binary by its
fingerprint already. Later groups take branches of their own
(`newspeak.file.…`, `newspeak.net.…`), named when they exist. A program's
module declaring an id under `newspeak.`, or given one by `as`, is refused,
as Java's class loaders refuse a class named `java.*` outside the platform
(`prior-art/jdk/src/java.base/share/classes/java/lang/ClassLoader.java:843-847`).
A program's own `Kernel` is then simply `Kernel`, and no tool name meets
both. The line each core-library file gains changes the shipped code's
fingerprint, and `tests/fixtures/golden-image.bin` is regenerated with it.

**10. An image records each module's path, id and version.** The module
record carries the path relative to the project in place of the origin
the build had -- an image no longer holds the building machine's
directories -- and the id and version as resolved, after an `as`, so that
an image written with `--strip-source` names its modules as the program
did and `ns-next load`'s tools name them as `run`'s do. The decoder holds
them to everything decision 2, 5 and 9 hold a project to, before any of
them is used ("An image is untrusted input", `CLAUDE.md`). The origin tags
`ORIGIN_PROJECT` and `ORIGIN_CORE` took the next free numbers, and
`ORIGIN_FILE` and `ORIGIN_EMBEDDED` are retired
([image-format.md](image-format.md), "Module section").

## What follows from it

- The main module has an id like any module, takes part in decision 5's
  refusals, and is not in the manifest.
- `ns-next run-corpus`'s generated application asks `manifest at:` for the
  module it runs, which it copies into its scratch directory beside the
  generated project, so that the module has a path relative to it.
- The debugger's `ModuleName` is the module's id and version
  (`debugger/remote.rs`); a breakpoint set on one version stands in that
  version only, one set without a version in every version, and whether a
  frame is the core library's is decided by its root mixin's `shipped`,
  never by the id's `newspeak.core.` prefix.
- The analyzer reports, in the editor: an id whose last part is not the
  class's name, a malformed version, an id in the reserved namespace
  outside the core library's own sources, and, as a warning, a malformed
  expectation before a parameter; its hover shows a module's `:module:`
  and a parameter's expectation.
