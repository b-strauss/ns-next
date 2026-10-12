# Formatting — the printer and `ns-next-analyzer fmt`

> **Status:** built.
> **Code:** the printer in `crates/ns-syntax/src/printer/` -- `mod.rs`
> (`print_module`, `format`, `LineEnding`, the width), `document.rs` (the
> document and its layout), `declarations.rs` (the module, classes, slots,
> methods, patterns, type annotations), `expressions.rs` (statements,
> chains of messages, keyword messages, cascades, blocks, tuples, literals),
> `comments.rs` (the comments where the attachment put them); the AST it
> reads in `ast.rs`, the comment attachment in `attach.rs`, the comparison
> without positions in `positions.rs` ([frontend.md](frontend.md), "AST",
> "Comments"); the command in `crates/ns-analyzer/src/commands/fmt.rs`, the
> `textDocument/formatting` answer in `crates/ns-analyzer/src/server.rs`.
> **Tests:** `crates/ns-syntax/tests/format_rules.rs`, `format_properties.rs`,
> `attachment.rs`, `document.rs`'s unit tests;
> `crates/ns-analyzer/tests/formatting_gate.rs`, `fmt_command.rs`,
> `protocol.rs`.

The formatter writes a Newspeak module in one layout, whatever layout it was
written in. It is a full pretty printer: it reads the module's syntax tree,
not its text, and decides every line break and every indentation itself. It
has fixed rules and no options.

## Purpose

**A tool and a gate.** The formatter is what an editor's *Reformat Code*
runs (`textDocument/formatting`, [editor-support.md](editor-support.md),
"Formatting") and what `ns-next-analyzer fmt` runs from the shell. It is
also a repository gate: every `.ns` file outside `corpus/` and `prior-art/`
is a fixed point of the formatter, and `crates/ns-analyzer/tests/formatting_gate.rs`
fails when one is not. The corpus and the prior art stay upstream's verbatim
copies and are never formatted.

**No options.** Every formatted module looks the same, so a reader never
meets a second style and a diff never carries a layout change one setting
caused. The request's `tabSize` and `insertSpaces` are ignored.

**A printer over the tree, in `ns-syntax`.** The runtime is to generate
source from syntax trees it builds or edits (a class changed at run time
written back as source), so the printer takes a `Module` and needs no source
text, and it lives in `ns-syntax`, which the runtime reaches. That requires
the tree to be lossless except for layout: it keeps parentheses, literal
spellings, type annotations, the prologue, `<-:`, the source order of
members and every comment ([frontend.md](frontend.md), "AST"). A tree the
parser made reprints from itself alone into source that parses back to the
same tree, positions aside.

## The style rules

Each rule with its reason. `format_rules.rs` pins the rules with tests
named after them -- every rule but 1, 11 and 22, which the other rules'
tests exercise (the width and indentation, one statement per line) or which
asks for nothing to be decided (a long line left long).

1. **Width 100 columns, two spaces per indentation level.** A column is one
   `char`. Code is never indented with tabs. *Why:* Newspeak nests deeply by
   its structure -- the module class, nested classes, method bodies,
   blocks -- and four spaces per level would spend 16 to 24 columns before
   any code; languages with deep structural nesting (Dart, Scala, Lisp,
   Prettier's JavaScript) indent by two. In the opened-up style a method
   body starts at column 8 to 16, and at 100 columns typical sends fit from
   there, while 120 is unwieldy in split windows. A tab's width is the
   reader's setting, so a layout counted in tabs is no fixed layout.
2. **Prologue as written.** The language id (`Newspeak3`) and the category
   string each stand on their own line before the class when present;
   nothing is added when one is absent, and no blank line follows them.
   *Why:* both are optional ([frontend.md](frontend.md), "The module
   header"); the formatter changes layout, never what a module declares.
3. **Class header.** `[access] class Name [type parameters] [factory] =
   [superclass [message]] [<: mixin …] (` on one line when it fits.
   Otherwise the header breaks outside in: first before `=`, the
   `= Superclass … (` continuation at +4; on that `=` line the whole `<:`
   chain stays when it fits, else every `<:` starts a line of its own at +4,
   and only a clause still too long for its line breaks inside its message
   (the superclass message is the last thing broken). When the pattern line
   itself does not fit, every factory keyword goes on its own line at +4 as
   well, a single keyword included:
   ```
   class NewspeakPrettyPrinter
       usingPlatform: platform
       ast: astModule
       parser: commonParser
       metadata: mdModule
       = Object new (
   ```
   The same holds for a mixin application without a body:
   ```
     class LongMixinApplicationNameNumberOne
         = SomeVeryLongSuperclassName withArgument: 1
         <: FirstMixinWithLongName
         <: SecondMixinWithLongName
         <: Third.
   ```
   *Why:* the outermost structure gives way first (rule 12): the header is a
   declaration `=` a superclass expression, and the `<:` chain is the outer
   level of that expression.
4. **Class comment.** The first comment in the header stands on its own
   lines at +2 right after the header's `(`, then the slots. A comment above
   `class` stays directly above it, no blank line between. *Why:* the class
   comment is what an editor shows for the class
   (`ClassDeclaration::class_comment`), and the formatter never moves a
   comment from one owner to another.
5. **Slots.** The opening and the closing bar each on their own line at +2,
   one slot per line at +4, each with its period; `||` and the access
   modifiers as written; an initializer breaks by the expression rules
   relative to its slot. An empty slot list (`| |`, `|| ||`) is dropped,
   unless a comment stands between its bars. No alignment of `=` or `::=`.
   Never on one line, however short. *Why:* one slot per line makes adding
   or removing a slot a one-line diff, and alignment would make it touch
   every neighbour.
6. **Periods.** In a multi-line sequence -- a method body, a multi-line
   block, the header's initializer expressions, a multi-line tuple -- a
   period after every element, the last included. A one-line block or tuple
   has none after its last element. The header's initializer expressions
   stand one per line at +2 after the closing bar. *Why:* a period after the
   last element lets a line be added or moved without touching another, as
   the trailing comma does in rustfmt's, Prettier's and dart format's
   output; it is consistent with slots, which end in a period; a one-line
   block or tuple has no diff to save.
7. **Members** in source order, never reordered. Exactly one blank line
   between the members of a class side, and no blank line anywhere else --
   not between `) (` and the first member, not before `) : (`, not inside a
   body. A member's leading comment sits directly above it; the blank line
   goes above the comment. *Why:* the slot indices of lazy slots follow
   the source order ([frontend.md](frontend.md), "AST"), so reordering
   would change more than layout.
8. **Empty parts.** An empty header or empty instance side is `()` inline
   (`class Top = () ()`; `class Foo = () (` with members below). An empty
   class side `: ()` is dropped. A part holding only a comment is not empty:
   it is opened up, the comment on its own line at +2. *Why:* an empty part
   says nothing, and a comment is content.
9. **Method header.** `[access] pattern [^<Type>] = (` on one line when it
   fits; otherwise the first keyword stays on the line and every further
   keyword goes on its own line at +4, ` = (` after the last:
   ```
     public copyReplaceFrom: start
         to: stop
         with: replacement
         startingAt: replacementStart
         ifAbsent: absentBlock = (
   ```
   A comment on a line of its own between two keywords stands at +4, as the
   keywords do. The keywords stand one below the other, as those of a
   broken keyword message do (rule 13).
10. **Method body** always multi-line, `= (` … `)`, even for one statement;
    an empty body is `= ()` inline. **Temporaries are always opened up**, in
    methods and blocks alike: each bar on its own line at the body's
    indentation, one temporary per line two spaces deeper, an initialized
    one with its period, and no blank line after the closing bar:
    ```
      public countUpTo: limit = (
        |
          current
        |
        current:: 0.
      )
    ```
    Empty temporaries (`| |`) are dropped unless a comment stands between
    the bars. Temporaries are slot declarations and are laid out like
    the header's slots (rule 5).
11. **Statements** one per line; `^count` without a space; a statement
    ending in a multi-line block puts its period right after the `]` (`].`).
12. **Breaking in general.** Outside in: a statement too long breaks at its
    outermost (lowest-precedence) level first -- cascade, keyword, binary,
    unary -- and only a part still too long breaks inside. All or nothing:
    once a level breaks, every link of that level is on its own line.
    Continuation lines stand at +4 relative to the line the expression starts
    on. Binary chains break before the operator, which leads its line:
    ```
      message:: 'expected '
          , expected printString
          , ' but got '
          , actual printString.
    ```
    A unary chain likewise, one selector per line at +4. After a receiver
    that takes more than one line, where its messages go depends on them.
    A keyword message with an argument that hugs (rule 13) is decided on
    the receiver's last line, whatever that line ends with: the parts
    outside the block bodies need only fit there:
    ```
        (someReceiver withAVeryLongSelector: argumentNumberOne
            and: argumentNumberTwo
            and: argumentThree
            andMore: yy) ifTrue: [
          a foo.
          b bar.
        ] ifFalse: [
          c foo.
        ].
    ```
    Any other message sent to a receiver that takes more than one line by
    itself -- a multi-line block, a multi-line string, a message hugging
    its blocks, or a parenthesised expression holding one -- continues on
    the receiver's last line only when that line ends with a broken block's
    `]`, or with a multi-line string the receiver ends with (closing
    parentheses aside); otherwise it goes on a continuation line of its
    own:
    ```
          ] value
              == nil)
              printString;
    ```
    (`value` follows the block's `]`; `== nil` follows `value`, and
    `printString` follows a parenthesis that ends on `nil`, so each takes a
    line of its own.) A receiver that is a unary or binary run broken for
    width is a level of the same chain, not such a receiver: outside in,
    the message after it follows its last link (`^aaaa…`, `foo`,
    `bar baz: 1.`, each on its own line). A receiver whose line ends with a
    comment leaves no room on that line: its messages are broken (rule
    21). A continuation at +4 is told apart from a nested body at +2.
13. **Keyword messages.** The first keyword on the receiver's line, every
    further keyword on its own line at +4 when the message breaks. **A block
    argument does not break its message by itself**: a multi-line block
    keeps `[` at the end of the line, its body at +2 relative to the
    statement, `]` at the statement's indentation, and the next keyword
    follows the `]` on the same line. Only when the parts outside the block
    bodies do not fit does the message break by the keyword rule. When a
    message is laid out this way, every block argument of it is multi-line --
    the blocks of one message break together, all or nothing:
    ```
      count > limit ifTrue: [
        self reset.
        ^nil.
      ] ifFalse: [
        count:: count + 1.
      ].
    ```
    Here `[count:: count + 1]` alone would fit. An argument that is itself a
    message hugging its own blocks hugs as a block does, and so does a
    multi-line string argument: the next keyword follows the string's last
    line (`bar: y` after `two'`). Control flow in Newspeak is keyword
    messages with block arguments; breaking the keywords apart instead would
    put every conditional's body two levels further in.
14. **Cascades** always broken: the receiver alone on the first line, each
    message on its own line at +4, `;` at the end of each line but the last.
    A comment beside a `;`, before it or after it on its line, trails the
    message the `;` ends and is written after the `;` (`foo; /// why`);
    one on a line of its own between two messages stays there.
15. **Blocks.** No inner spaces: `[x]`, `[:each | each foo]`, `[]`,
    `[:each |]`. One line only with at most one statement, no temporaries,
    and when it fits; otherwise multi-line, the parameters on the `[` line.
    A block whose body breaks by itself (a statement hugging its own blocks)
    is never one line. A block that is not a keyword argument -- a receiver,
    an assigned value -- has the same shape: `[` at the end of the line, the
    body at +2, `]` at the statement's indentation, and the messages sent to
    a receiver block follow its `]` (`] value: 3.`, `] whileTrue.`). A
    receiver block breaks when it does not fit, whatever follows it.
16. **Parentheses as written**, redundant ones included; no inner spaces.
    The tree records them ([frontend.md](frontend.md), "AST").
17. **Tuples** like blocks: one line when it fits (`{a. b. c}`, `{}`, no
    period after the last), otherwise `{` at the end of the line, one element
    per line at +2, a period after each, `}` at the statement's indentation.
    A tuple whose element breaks by itself is never one line.
18. **Literals exactly as written** -- numbers, strings, symbols, characters
    (`16r1F`, `'it''s'`, `"a"`, `#at:put:`). A multi-line string is never
    re-indented, its line breaks kept byte for byte, and a literal is never
    broken. A string's line breaks and spaces are its value.
19. **Setters** `count:: value`, no space before `::`; slot initializers
    `name = value`, `name ::= value`. Never broken right after `::` or `=`:
    a long value breaks inside itself.
20. **Type annotations.** One space before `<`, none inside: `name <String>`,
    `at: index <Integer>`, `size ^<Integer> = (`. Inside a type: `List[String]`,
    `Map[K, V]` (a space after the comma, none before), `String | Nil`
    (spaces around `|`), `[:Integer | Boolean]`. Beyond these, the tokens of
    a type are spaced as an expression's are -- a space after a keyword and
    around a binary operator or the bound operator `<:` -- a `where` clause
    has a space inside its braces and around its `::`
    (`where { E <: Number }`), and two tokens that written together would lex
    as something else are always separated, so a reprinted type reads back
    as the same tokens. A type is never broken and stays with its name; its
    spelling is kept and only these spaces are decided. *Why:* the
    annotations are kept as tokens, not as a type AST
    ([frontend.md](frontend.md), "What is kept but ignored, and what a
    reprint drops"), so their spacing is decided from the tokens alone.
21. **Comments** stay with the node they are attached to
    ([frontend.md](frontend.md), "Comments"): a *leading* comment on its own
    lines belongs to the node after it and moves with it; a *trailing* one,
    after code on its line, belongs to that construct; a *dangling* one,
    after the last element before a closer, belongs to the container and is
    printed at the content's indentation. The form is kept: `///` stays
    `///`, `(* … *)` stays `(* … *)`. A `///` block is re-indented line by
    line, its text after `///` kept exactly. A multi-line `(* … *)` moves its
    continuation lines by as much as its `(*` moves; a line with less
    indentation than is taken away starts at column 0. A continuation line's
    own leading whitespace, tabs included, is the comment's text: it is
    shifted by adding spaces or taking leading whitespace characters away,
    never expanded, and a tab counts as one column. Metadata comments
    (`(* :exemplar: … *)`) are comments. A trailing comment gets one space
    before it, is never aligned, and does not count toward the width. A
    `///` or a comment that ends its line inside an expression forces the
    broken form of the level it stands in and stays after the token it
    followed -- one inside a binary chain breaks the whole chain:
    ```
        ^a
            + b /// why
            + c.
    ```
    So does one trailing the chain's receiver, every link then on a
    continuation line of its own (`^a /// why`, `+ b`, `+ c.`). A keyword
    message after a receiver whose line ends with a comment, or led by a
    comment on a line of its own, starts a continuation line at +4 as well:
    at the statement's indentation it would read as a new statement sending
    to the implicit receiver. Without a block argument it is broken, each
    further keyword on its own line at +4; one hugging its blocks stays on
    that line when it fits (`ifTrue: [a] ifFalse: [b].`), else hugs there,
    its blocks indented from that line. A comment trailing the first argument of a
    broken keyword message stands with the keywords after it, at +4.
    ```
        current = 16r5C /// \
            ifTrue: [
              pauseCapture.
            ] ifFalse: [
              read.
            ].
    ```
    A comment that leads an expression and ends its line, or stands on one
    of its own, puts the expression on the next line at +4: it continues the
    line it follows (after a keyword, an operator, `::`, `=` or `^`). An
    inline `(* … *)` between two tokens stays there with one space on each
    side (none next to an opening delimiter) and counts toward the width.
    Only a comment's indentation moves; its text is never rewritten.
22. **An unbreakable long line stays long**, without a message: the gate
    checks only that formatting changes nothing.
23. **File level.** The line ending is taken from the file: the first line
    ending of the input decides every line ending of the output
    (`LineEnding::of`; LF when the input has none). Line breaks inside a
    string literal stay byte for byte. No trailing whitespace on any line,
    comment lines included, never inside a string. Exactly one line ending
    at the end of the file. No tabs in the indentation the printer writes.
    *Why:* a Windows checkout with `core.autocrlf=true` has CRLF line
    endings; a fixed LF would make the gate and the editor rewrite every
    line ending of such a checkout. rustfmt's `Auto`, Black and clang-format
    take the line ending from the file too. A string's line breaks are its
    value.
24. **Other declarations.** A lazy slot is a member like a method, on its
    own line with its period, `lazy` and the access modifier in the order
    written (`lazy public cache`, `public lazy other` -- the parser accepts
    both, and `SlotDeclaration::access_before_lazy` records which). A mixin
    application without a body is one line, breaking before `=` and each
    `<:` at +4 when too long (rule 3). Nested classes follow the same rules,
    indented.
25. **Spacing between tokens.** One space around a binary operator, one
    after a keyword, none before `.` or `;`, `receiver <-: message`.

## How the printer works

`print_module` lowers the module to a *document*, then lays the document
out within the width. Lowering decides what may break and how; the layout
decides what does.

### The document

`document.rs`'s `Document`:

- `Text` -- text without a line break.
- `Verbatim` -- text that may hold line breaks (a multi-line string),
  written byte for byte and never trimmed; a `\r` is a line break whether or
  not a `\n` follows it.
- `Shifted` -- the lines of a block comment, the first where the layout
  stands, each further one moved by as much as the first moved from its
  source column (rule 21).
- `FreshLine` -- the next text starts a line of its own: a line break before
  it unless its line holds nothing yet (a comment that must stand alone, or
  must end its line).
- `Trailing` -- text after something on its line (a comment trailing a
  token), counted toward the width only when more text follows it on the
  line.
- `LeadingStart`, `LeadingEnd` -- around a comment leading the text after
  it: `LeadingEnd` is a space, or a `FreshLine` when the comment started its
  line.
- `Line` (a space when flat, a line break when broken), `SoftLine` (nothing
  when flat), `HardLine` (always a line break, and breaks every group around
  it).
- `IfBreak` -- one document when the enclosing group is broken, another when
  flat.
- `LineSuffix` -- text written at the end of the current line, before its
  line break, never counted toward the width.
- `BreakParent` -- writes nothing and breaks every group around it.
- `BlockEnd` -- writes nothing; stands right after a block's `]`, so that a
  message sent to what ends there may follow it on its line.
- `Follow` -- a receiver and the messages sent to it: when the receiver ends
  on a broken block's `]` or a multi-line string (only `)` after it), or did
  not break, the messages are decided where the receiver ends; when it broke
  otherwise, the messages break (rule 12's continuation).
- `Indent` -- line breaks inside start further in.
- `Group` -- flat if it fits on the line and holds nothing that forces a
  break, else broken, as a whole.
- `Choice` -- alternatives: the first whose first line fits, laid out flat
  (its groups that hold a forced break still break), else the last, broken.
- `Concat`, and `Shared` -- a document held once that stands in several
  alternatives of `Choice`s.

### Outside in, all or nothing

A level of an expression is a `Group`: a run of unary messages, a run of
binary messages, the keywords of a keyword message. A group nests the groups
of the levels inside it, and the layout decides the outermost group first:
it stays flat if it fits, and breaks as a whole if not, every `Line` in it a
line break; only then are the groups inside it decided, each against what
is left of its line. That is rule 12. A forced break -- a `HardLine`, a
`BreakParent`, a `FreshLine`, a `Verbatim` or `Shifted` with a line break --
breaks every group around it up to the nearest `Choice` (`broken_groups`,
one pass before the layout): an alternative's break is the alternative's,
and choosing among alternatives is how the line around it avoids breaking.

A chain of messages is walked link by link, never by recursing into each
receiver (`send_chain`): consecutive unary or binary links form one run,
grouped so the level around it breaks first.

### Block hugging

A keyword message with a block argument (or an argument that hugs, rule 13)
is a `Choice` of three layouts (`keyword_message`):

1. the whole message on one line -- left out when a part of it breaks by
   itself, since a group that must break would "fit" on its first line;
2. hugged: the keyword parts joined by plain spaces outside any continuation
   indent, every block argument opened at the line end and broken, so the
   blocks of one message break together;
3. the keyword group broken, each block on one line where it fits.

The `Choice` takes the first whose first line fits, so a block argument is
fitted only by the text outside its body. Each argument is lowered once and
shared (`Shared`) by every alternative: without sharing, nested messages
with block arguments would multiply their arguments exponentially.

A receiver that breaks by itself (a multi-line block, a multi-line string,
a parenthesised hugging message) is wrapped in a one-alternative `Choice`
(`receiver_head`), which keeps its break from breaking the group around it;
where the messages sent to it go is then decided by where it ends
(`Follow`). A keyword message with an argument that hugs is the exception:
it is written after the receiver and decided on the receiver's last line
however the receiver broke (`keyword_message`; rule 12).

A receiver that ends its line with a comment leaves a `FreshLine` waiting
rather than a line break written. `Follow` counts that as a break and lays
its messages out broken, and the keyword parts written after a receiver
stand in `IndentAfterFreshLine`: an indentation taken only when a fresh
line waits where it starts, so on the receiver's own line they continue it
unchanged, and after a comment they start a continuation line with what
they hold indented from it.

### Measuring again after a line break

A group is measured only up to its first line break. When a group or
`Choice` is laid out flat and a line break is written inside it anyway (a
hugged block's body), what follows on the new line was never measured. So
after a line break written inside flat content, the next `Group` or `Choice`
met is measured again, as in broken content, rather than taken flat. The
lowering therefore makes everything breakable that follows a hugged block's
`]` a group of its own; an argument that cannot break is left bare in the
hugged form, so the measuring again goes to the breakable argument after
it. A `Line` of a group already taken flat stays a space.

### Comments and line suffixes

The comments come from the tree (`comments.rs`), each written where its
`Anchor` puts it -- before its node, after it, or after its node's `n`-th
own token, counted as `attach.rs` counts them (`OwnTokens`). A one-line
comment that ended its line in the source still ends it: it is a
`LineSuffix` with a `BreakParent`, never counted toward the width, and a
`FreshLine` starts whatever follows on a new line. A one-line `(* … *)`
trailing a token is `Trailing` text, counted only when more follows it on
its line. A comment that stood on a line of its own keeps one. A block
comment over several lines is written as `Shifted`.

### Linear layout

The layout walks the document with an explicit stack, as do the pass that
marks forced breaks and the document's drop: a document nests as deep as
the source does (a chain of a hundred thousand messages lays out on a
256 KiB stack). A measurement stops at the first line break or as soon as
the width is exceeded, and the marking pass walks each shared document
once, remembering whether a forced break leaves it, so the work stays
linear in the output.

## Entry points

- `ns_syntax::print_module(&Module, LineEnding) -> String` -- a module
  printed in the one layout, ending with one line ending. It needs no
  source: the entry the runtime uses for a tree it built. Such a tree reads
  back the same only when each comment's `anchor` and `placement` are what
  `attach.rs` would answer for the printed text: an `After` comment keeps
  its owner only as `EndOfLine` or `Inline` -- as `OwnLine`, the placement
  `Module::without_positions` gives every comment, it is printed on a line
  of its own and read back `Before` the next node -- and two `OwnLine`
  `///` comments on consecutive nodes with no token between them are read
  back as one block. `ClassDeclaration::class_comment` compares spans, so
  it can miss the class comment of a tree without positions. A tree
  `parse` answered reads back with every comment where it was.
- `ns_syntax::format(&str) -> Result<String, Vec<Diagnostic>>` -- parse,
  take the line ending from the source (`LineEnding::of`), print. A source
  with any error is refused with its diagnostics and never printed; a
  recovered tree is never printed.
- `LineEnding` -- `LineFeed`, `CarriageReturnLineFeed`, `CarriageReturn`.
- `ns-next-analyzer fmt [--check] [paths…]` -- formats every `.ns` file in
  `paths` (a directory stands for the `.ns` files below it, none for the
  current directory). It formats exactly what it is given: run at the
  repository root it would rewrite `corpus/` and `prior-art/`, which stay
  upstream's copies, so pass the directories the gate covers --
  `ns-next-analyzer fmt goldens benchmarks crates/ns-runtime/core-library`.
  A file is rewritten only when it changes, written
  beside the target and renamed over it, so a failure part way never leaves
  a truncated source. With `--check` nothing is written: each file that
  would change is listed on stdout and the exit status is 1. A file with a
  syntax error is left alone, its diagnostics printed, the exit status 1,
  and the other files are still formatted. A UTF-8 byte-order mark is
  kept: the text after it is formatted and the mark written back in front.
- `textDocument/formatting` -- the server announces
  `documentFormattingProvider` and answers one `TextEdit` replacing the open
  document's whole text, or none when it is formatted already or does not
  parse ([editor-support.md](editor-support.md), "Formatting").

## Guarantees and the tests that hold them

- **Total.** Every module `parse` answers prints, without a panic and
  without a stack overflow. `format_properties.rs`'s
  `a_long_chain_formats_on_a_small_stack`, and `document.rs`'s
  `a_deeply_nested_document_lays_out_and_drops_on_a_small_stack`.
- **Idempotent.** Formatting a formatted source changes nothing:
  `formatting_is_idempotent`, over every source in `corpus/modules`,
  `goldens`, `benchmarks` and the core library (more than 590).
- **Preserving.** The formatted source parses to the same tree as the
  original, compared by `Module::without_positions` (spans, a block
  comment's column and each comment's `Placement` erased, since they are
  layout; anchors compared; an empty part read as absent):
  `formatting_preserves_the_module` over the same sources, and
  `comments_everywhere_keep_their_owners` over sources with comments
  around and inside every kind of node, in every placement.
- **Deterministic.** The output depends on the tree and the line ending
  alone.
- **The rules.** `format_rules.rs`, tests named after the rules (all but
  1, 11 and 22); `attachment.rs`, the
  attachment rule; `document.rs`'s unit tests, the layout's forms.
- **The repository.** `formatting_gate.rs` requires `format(file) == file`
  for every `.ns` under `goldens/`, `benchmarks/` and
  `crates/ns-runtime/core-library/`; `crates/ns-cli/tests/source_formatting.rs`
  holds the one rule the formatter does not enforce, `///` rather than
  `(* … *)`. Every golden keeps its outcome under the formatted layout; none
  pins a line or a column.
- **The command and the server.** `fmt_command.rs` runs the binary over
  temporary directories (rewriting, `--check`, a syntax error, the current
  directory, CRLF kept); `protocol.rs` the formatting request.

## What is deliberately not done

- **Range formatting and formatting on type** (`textDocument/rangeFormatting`,
  `onTypeFormatting`) -- left open ([OPEN-WORK.md](../OPEN-WORK.md),
  "Planned: editor support and the analyzer").
- **Wrapping comments.** A comment's text is the author's; only its
  indentation moves.
- **Normalising literals.** A literal keeps its spelling (rule 18).
- **Reordering members** (rule 7), aligning `=` or comments (rules 5, 21),
  and options of any kind.

The known limits of the layout -- a long tail on a hugged block's `]` line,
a group breaking where breaking does not shorten the line -- are in
[OPEN-WORK.md](../OPEN-WORK.md), "Known defects and gaps".

## Prior art

The canonical implementation's printer, `prior-art/newspeak/NewspeakPrettyPrinter.ns`,
walks the canonical AST and writes source, aiming at 80 columns and
indenting with tabs; it recovers comments from the source text between the
previous node and the next, so a tree without source loses them. Here the
comments are part of the tree, the layout is decided by the document's
groups rather than while writing, and the indentation is spaces. The
document and its groups follow Wadler's "A prettier printer".
