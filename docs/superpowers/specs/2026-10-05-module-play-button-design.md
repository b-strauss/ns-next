# Running a module from its `main:args:` — design

Temporary: once the batch is finished, its content lives in
`docs/design/editor-support.md`, "The project file and running projects", and
this file is deleted.

## Goal

A Run/Benchmark play button beside `main:args:` of a `.ns` module's root
class, when `ns-next run <file>.ns` can start that module, and "Run" in the
context menu of such a file. The "Newspeak" run configuration takes a single
`.ns` file as well as a `newspeak.json`. Decided with the user on 2026-10-05.

## Which module is runnable

`ns-next run` instantiates a source file's root class with `new` and sends the
instance `main:args:` (`crates/ns-cli/src/program.rs`,
`instantiate_application`; `entry.rs`, `send_main`). The send is a public one:
a `protected` or `private` `main:args:` -- and a method without a modifier is
protected (§6.3.1) -- answers `doesNotUnderstand: #main:args:` (checked by
hand on 2026-10-05). So a module is runnable when its document symbols show:

- the top-level symbol, the root class, has a `Constructor` child named `new`
  (the server names an omitted factory `new` as well), and
- it has a `Method` child named `main:args:` whose `detail` is exactly
  `public` -- instance side (no `class`) and public.

A pure Kotlin function, `runnableMain(symbols): DocumentSymbol?`, answers the
`main:args:` symbol or null. The analyzer does not change; a Rust test in
`crates/ns-analyzer` pins the contract the function relies on (a written and
an omitted `new` are both named `new`; a public instance-side method's
`detail` is `public`, a class-side one's `public class`). A project's main
module, whose factory is `packageUsing:`, gets no button: it is run from its
`newspeak.json`.

## How the symbols reach the plugin

The structure view's per-file cache -- the last usable answer kept on the
`VirtualFile` with the document stamp it was given for -- becomes a shared
`DocumentSymbolCache` in the `lsp` package, with:

- `current(file)`: the cached answer when its stamp is the document's current
  one, else null; never asks.
- `fetch(project, file)`: asks synchronously, off the EDT, as the structure
  model does today (starting the client when there is none), stores a usable
  answer, and answers it.
- `request(project, file, onAnswer)`: asks on a pooled thread and calls
  `onAnswer` once a usable answer is stored; at most one request per file and
  stamp is in flight.

The structure view model keeps its behaviour (retries, the EDT rule) and
reads and writes through the cache.

The **line marker** never blocks: a daemon pass runs in a read action, and
waiting for the server there would hold up every write action, typing
included. It reads `current(file)`; with none, it calls `request`, whose
answer restarts the daemon for that file (`DaemonCodeAnalyzer.restart(psiFile)`),
and the next pass places the button. After an edit the button is therefore
gone until the server has answered the new text -- milliseconds. Keeping a
stale answer's position was rejected: after an edit above `main:args:` the
button would sit on the wrong line.

The **context-menu producer** reads `current(file)` and, with none, `fetch`es
off the EDT: it runs once per user gesture, as the structure view's question
does.

## The play button and the context menu

- `NewspeakModuleRunLineMarkerContributor`, a `RunLineMarkerContributor` for
  the language `Newspeak`, registered in `plugin.xml` (it reads no JSON PSI,
  so it does not live in `newspeak-json.xml`). It answers for the leaf token
  at which `main:args:`'s `selectionRange` starts (`main:`), converted from
  the symbol's UTF-16 position through the document, and for no other
  element. Icon and menu as for `newspeak.json`: the platform's run marker,
  "Run" and "Benchmark" (`NewspeakRunAction`). Tooltip `Run module '<root
  class name>'`.
- A producer for `.ns` files: "Run" in the Project view's context menu of a
  runnable `.ns` file and anywhere in its editor.

## The run configuration

- The stored option `projectFile` becomes `program`: a project's
  `newspeak.json` or a `.ns` file. The form's field is "Program", its chooser
  accepts both. Saved configurations lose their path (the plugin is
  unpublished; only local configurations are affected).
- For a `.ns` file the command line is `ns-next <run|bench> [--iterations
  <n>] <file>.ns [-- <arguments>]`, run in the file's directory; for a
  `newspeak.json` it stays as it is (the project's directory).
- The script field is disabled for a `.ns` program. A configuration that
  names a `.ns` file and a script -- stored by hand -- is refused by
  `checkConfiguration` with the CLI's reason (a script belongs to a project's
  `newspeak.json`).
- A program that is neither a `newspeak.json` nor a `.ns` file, or names no
  file, is refused as a missing project file is today, with a message naming
  both kinds.
- A configuration created from the button or the context menu is named
  `<file>.ns`, with ` (benchmark)` for `bench`; it is temporary, reused while
  equal (same program file, no script, same command), and made unique as
  today.

## Testing

- Kotlin unit tests: `runnableMain` (factory omitted, `new` written, another
  factory, a protected and a private `main:args:`, a class-side
  `main:args:`, no `main:args:`, an empty answer); the command line and
  working directory for a `.ns` program; the gutter names; the refusal of a
  `.ns` program with a script; the contributor and the producer over a cache
  filled by a test fetcher; the structure view's existing tests unchanged.
- The Rust test in `crates/ns-analyzer` named above.
- By hand under `runIde`: the button appears beside `main:args:` of a
  runnable module and nowhere in one that is not, returns after an edit, runs
  and benchmarks; the context menu in the Project view and the editor; the
  form with a `.ns` program disables the script field.

## Documentation

`docs/design/editor-support.md`, "The project file and running projects" and
"Structure" (the shared cache); `plugin.xml`'s description and change notes;
the item in `docs/OPEN-WORK.md` is removed.
