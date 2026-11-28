# Image format

An *image* is an object graph together with the code and the declarations
it runs: the program's modules, their compiled bodies, the mixins they
declare and the objects a run built from them. There is one format. An
image written just before an application is sent `main:args:` -- a
started image, which `load` starts -- and one written from any roots a
host names -- a snapshot -- differ only in a flag of the header and in
the roots they carry.

This chapter gives the image's structure -- its encodings, its header,
its sections and the tags that select among their records -- the
validation a load performs on it, the bounds a load keeps to, and the two
operations that read and write an image, *LoadImage* and *WriteImage*,
which state what a program and a host observe of a load and a write. A
body's encoding is the bytecode chapter's
([The wire encoding](bytecode.md#the-wire-encoding)), as are the rules
the decoder checks on a body; this chapter refers to them and does not
repeat them.

## Encodings

r[image.encoding]
Every integer is little-endian; a floating-point number is an IEEE-754
`binary64`; every string and every symbol's text is UTF-8. An integer
that fits an `i64` is written as an `i64`; every other is written as its
sign and its 64-bit digits, least significant first. An image holds no
native code, no machine addresses, no tagging of the running VM's values
and nothing that depends on the writing host's endianness: an image
written on one architecture loads on every other.

## Untrusted input

r[image.untrusted]
An image is untrusted input. No byte sequence makes a load panic or abort
the process: every id, tag, count and length is checked before it
indexes, allocates or slices, and every recursion whose depth the bytes
choose is bounded. A byte sequence that is no valid image is refused with
an error.

The guarantee covers decoding. What a decoded image's code does once it
runs is the program's, with the exposure of running the same program from
its source; [Running a decoded image](#r-image.run) states what bounds
it.

## The header

r[image.header]
An image begins with its header, in this order: the four bytes `NSNX`,
the magic; the format version, a `u32`, which is 1; the flag byte, a
`u8`; and the fingerprint, a `u64`. An image whose magic is not `NSNX`,
or whose format version is not 1, is refused.

r[image.header.flags]
The flags are the bits the table of tags gives in the space
`header-flag`: `has-source`, set when the image carries its program's
source, and `start`, set when the image is a started image. An image
whose flag byte sets any other bit -- a retired one included -- is
refused, since a flag a load does not know may stand for a section it
cannot see.

r[image.header.fingerprint]
The fingerprint identifies the shipped world of the binary that wrote the
image: the core library's modules and the declarations an image names by
their shipped names. How it is computed is the binary's to define. A load
whose binary computes another fingerprint refuses the image before it
reads anything past the header.

## Sections

r[image.sections]
After the header the image holds its sections, in this order: the symbol
table; the code table; the text table, only in an image whose header sets
`has-source`; the mixin table; the module section; the shipped names; the
graph, its object table followed by its roots; and the tag-named hashes.

r[image.trailing-bytes]
The last section ends the image. An image with a byte after the end of
its last section is refused.

## Tags

r[image.tags]
A tag selects among the forms a record or a field can take. The tags of
the fourteen spaces the image's sections use -- the header's flags,
values, object bodies, mixin references, mixin sides, class references,
frame kinds, frame bodies, tag-named hashes, slot kinds, module origins,
module versions, the kinds of a shipped name and the accessibilities of
a declaration, a slot or a method -- are those of the
table, a header flag written as its bit's value. `retired` marks a number
no longer assigned, which a load refuses as it refuses any number the
table does not give, as for a body's tags
([`bytecode.wire.tags`](bytecode.md#r-bytecode.wire.tags)). A tag is
assigned once and never changes meaning
([`bytecode.wire.tag-assigned-once`](bytecode.md#r-bytecode.wire.tag-assigned-once)).

```spec-table image-tags
header-flag      retired                      1
header-flag      retired                      2
header-flag      start                        4
header-flag      has-source                   8
value            nil                          0
value            false                        1
value            true                         2
value            small-integer                3
value            float                        4
value            object-id                    5
value            class-tag                    6
value            retired                      7
value            retired                      8
value            large-integer                9
value            fraction                     10
value            shipped-module               11
object-body      instance                     0
object-body      string                       1
object-body      symbol                       2
object-body      array                        3
object-body      retired                      4
object-body      retired                      5
object-body      closure                      6
object-body      class                        7
object-body      metaclass                    8
object-body      retired                      9
object-body      retired                      10
object-body      byte-array                   11
object-body      weak-array                   12
object-body      ephemeron                    13
object-body      mixin                        14
object-body      frame                        15
object-body      body-less-class              16
object-body      instance-under-construction  17
mixin-reference  row                          0
mixin-reference  retired                      1
mixin-reference  shipped                      2
mixin-side       instance                     0
mixin-side       class                        1
class-reference  retired                      0
class-reference  retired                      1
class-reference  record                       2
class-reference  retired                      3
class-reference  retired                      4
class-reference  retired                      5
class-reference  retired                      6
class-reference  retired                      7
class-reference  shipped                      8
frame-kind       method                       0
frame-kind       closure                      1
frame-kind       top-level                    2
frame-kind       initializer                  3
frame-kind       superclass-clause            4
frame-kind       retired                      5
frame-kind       retired                      6
frame-kind       own-initializer              7
frame-kind       standalone-initializer       8
frame-kind       factory                      9
frame-body       none                         0
frame-body       code                         1
frame-body       declared                     2
hashed           retired                      0
hashed           retired                      1
hashed           retired                      2
hashed           retired                      3
hashed           retired                      4
hashed           retired                      5
hashed           retired                      6
hashed           shipped                      7
slot-kind        immutable                    0
slot-kind        mutable                      1
slot-kind        lazy-immutable               2
slot-kind        lazy-mutable                 3
origin           retired                      0
origin           retired                      1
origin           project                      2
origin           core                         3
origin           none                         4
module-version   unversioned                  0
module-version   versioned                    1
shipped-kind     module-instance              0
shipped-kind     class                        1
shipped-kind     metaclass                    2
shipped-kind     level                        3
shipped-kind     level-metaclass              4
access           public                       0
access           protected                    1
access           private                      2
```

## Structure

The table of this section, `image-structures`, gives the whole image
field by field: a load reads an image by it, and an image no walk by it
reads to its last byte is no image.

A row `<structure> <item> …` defines a structure as its items, in order.
The first row is `image`, the whole file. A row `<structure>/<space>`
defines a *tagged* structure: a `u8` tag of that space of the table of
tags, and the rows `<structure>/<space>/<tag>` give the fields that
follow each tag. A tagged structure has no other form: a retired tag, or
one the table of tags does not name, is no image.

An item is `<field>:<type>`. The types are:

- `u8`, `u16`, `u32`, `u64` and `i64`, integers of that width, and
  `f64`, a `binary64`, each little-endian
  ([`image.encoding`](#r-image.encoding));
- `magic`, the four bytes `NSNX`;
- `bytes`, a `u32` length and that many bytes, and `text`, `bytes` that
  are UTF-8;
- the ids, each a `u32` that names an entry of a section by its index
  from 0: `symbol` an entry of the symbol table, `code-id` a body of the
  code table, `text-index` a text of the text table, `row-id` a row of
  the mixin table, `shipped-index` a name of the shipped names and
  `object-id` a record of the graph's object table. An id is less than
  the count its section begins with, whether the section comes before
  the id or after it;
- `body`, a body as the bytecode chapter writes it
  ([`bytecode.wire.body`](bytecode.md#r-bytecode.wire.body), its
  instructions and literals by
  [`bytecode.wire.instruction`](bytecode.md#r-bytecode.wire.instruction)
  and [`bytecode.wire.literal`](bytecode.md#r-bytecode.wire.literal));
- the name of a structure of the table, which stands there whole.

An item `<type>*<field>` is that many items of the type, `<field>` an
earlier field of the same row; `<type>?<field>` is present only when
that earlier field is not 0; and `<type>?flag:<name>` is present only
when the header sets the flag `<name>`. A field named `has_…` is written
1 for present and 0 for absent.

r[image.structure]
An image is laid out as the table `image-structures` gives it, every
field in the order and the width its row gives, the walk by the table
ending at the image's last byte.

### The symbol table

r[image.symbols]
The symbol table holds every selector and every name the image's other
sections write -- a send's selector, a block's parameters, a slot's,
method's and declaration's name, a `Symbol` object's text, a shipped
name's module and declaration -- each text once, and every other section
names one by its index. The symbols the code table and then the mixin
table name come first, numbered in the order those two tables first name
them.

### The code table

r[image.code-table]
The code table holds every body the image's declarations compile -- each
row's methods, its three initializer bodies and its superclass clause --
and every body that only a closure or a frame of the graph runs, the
code of a version an edit superseded, each with the blocks it creates;
each body is written as
[`bytecode.wire.body`](bytecode.md#r-bytecode.wire.body) gives it. A
block comes before the body that creates it, so that every `code`
operand names an earlier body.

The bodies of the mixin table's rows come in row order, then those only
the graph runs.

### The text table

r[image.texts]
The text table is present exactly when the header sets `has-source`. It
holds one text per declaration of the mixin table -- per row that is no
row's class side -- as its parts: the range of source it spans, its
holes, the ranges of the declarations nested in it, and its text with
the holes cut out. A row names its text by index, and a class side's row
names its declaration's text.

### The mixin table

r[image.mixins]
The mixin table holds one row per class declaration the image's modules
make, a declaration's class side a row of its own: its name, access and
source range; its slots, each with its name, kind and access; its
factory's selector and arity, when it has one; whether it has initializer
expressions; its superclass clause's message and its `<:` clauses'
messages, each a selector and whether it takes arguments; whether it
writes a body; its compiled methods, each with its selector, access,
arity and body; the rows of its nested declarations and of its class
side; the bodies of its full initializer, its initializer's own part,
its standalone initializer and its superclass clause, each when it has
one; and, in an image with source, its text. The rows form a forest, the
children of a row its nested declarations and then its class side: first
one tree per module, the module's root declaration its root, in module
order, and then one obsolete tree per declaration an install removed
that the graph still reaches
([`runtime.install`](runtime.md#r-runtime.install)), the removed
declaration its root, with every declaration nested in it, each as it
last was. Each tree is written in preorder, a row before its children.
After the rows, the table holds how many of its last trees are obsolete
trees. An obsolete tree belongs to no module: no module record names it,
and its root is nested in no row.

The methods of a row are in the order of their selectors' text. Every
accessor of a slot follows from its name, kind and access and is not
listed, except a lazy slot's getter, which is one of the row's methods.

### The module section

r[image.modules]
The module section holds one record per module, in module order, the
application's module first: its origin and path -- a path relative to
the project for a module of the program, the file's name for the
binary's own manifest, and no path for a module a program declared from
source, which no file holds -- its id, its version when it declares one, and
the object id of its root class's record in the graph, a record of the
class whose row is the root of the module's tree in the mixin table.

### The shipped names

r[image.shipped-names]
The shipped names list every object of the binary's shipped world the
graph and the tag-named hashes name, each once: the symbols of its module
and of its declaration, its kind and, for a level a `<:` clause of a
shipped declaration added or that level's metaclass, the index of that
clause. A record names one by its index in the list.

The names are listed in the order the graph first uses them, then those
only the tag-named hashes use.

### The graph

r[image.graph]
The graph is its object table -- a count, then that many records -- and
its roots, a count and that many values. A record is a header and a body:
the header the record's class as a class reference and its identity
hash, a `u32`, 0 for an object that was never asked for one; the body a
tag of the space `object-body` and its fields.

r[image.graph.record]
Each body tag is followed by the fields its row gives: an instance's --
one under construction under a tag of its own -- an array's, weak
array's and ephemeron's values, counted; a string's text; a symbol's
symbol; a closure's body, the record of the frame it captured
and its copied values; a class's record, the mixin it applies -- a row
of the mixin table or a shipped name -- then its superclass, enclosing
object, declaring class and, for a level a `<:` clause added, the
applying row, the clause's index and the class the clause named, a level
over a shipped mixin writing only its superclass and its clause; a
metaclass's class; a byte array's bytes; a reified mixin's instance side
and the side it stands for; a frame's kind, its body, its program
counter, whether it can no longer continue, its receiver, its sender and
home, its current class and its registers; and a body-less declaration's
class's record followed by its declaration's enclosing object and
declaring class.

r[image.graph.value]
A value is a tag of the space `value` and its payload: none for `nil`,
`false` and `true`; an `i64` for an integer that fits one, the sign and
the digits for every other ([`image.encoding`](#r-image.encoding)); the
`binary64` for a float, immediate or boxed; an object id for an object
with a record; a class reference for a class; the numerator and
denominator, each an integer value, for a fraction; and the index of a
shipped name for a module's instance.

### The tag-named hashes

r[image.hashes]
The graph is followed by the tag-named hashes: a count, then that many
entries, each the tag `shipped` of the space `hashed`, the index of a
shipped name and the object's identity hash, which is never 0. They give
the hash of every object of the shipped world that was given one.

### The table

```spec-table image-structures
image                                      header:header  symbols:symbol-table  code:code-table  texts:text-table?flag:has-source  mixins:mixin-table  modules:module-section  shipped-names:shipped-name-section  graph:graph  hashes:hash-table
header                                     magic:magic  version:u32  flags:u8  fingerprint:u64
symbol-table                               count:u32  symbol:text*count
code-table                                 count:u32  body:body*count
text-table                                 count:u32  text:declaration-text*count
declaration-text                           start:u32  end:u32  hole_count:u32  hole:hole*hole_count  text:text
hole                                       start:u32  end:u32
mixin-table                                count:u32  row:mixin-row*count  obsolete:u32
mixin-row                                  name:symbol  access:access  start:u32  end:u32  slot_count:u32  slot:slot*slot_count  has_factory:u8  factory:factory?has_factory  has_initializer_expressions:u8  has_superclass:u8  superclass:clause-message?has_superclass  application_count:u32  application:clause-message*application_count  has_body:u8  method_count:u32  method:method*method_count  nested_count:u32  nested:row-id*nested_count  has_class_side:u8  class_side:row-id?has_class_side  has_initializer:u8  initializer:code-id?has_initializer  has_own_initializer:u8  own_initializer:code-id?has_own_initializer  has_standalone_initializer:u8  standalone_initializer:code-id?has_standalone_initializer  has_superclass_clause:u8  superclass_clause:code-id?has_superclass_clause  text:text-index?flag:has-source
slot                                       name:symbol  kind:slot-kind  access:access
slot-kind/slot-kind
slot-kind/slot-kind/immutable
slot-kind/slot-kind/mutable
slot-kind/slot-kind/lazy-immutable
slot-kind/slot-kind/lazy-mutable
access/access
access/access/public
access/access/protected
access/access/private
factory                                    selector:symbol  arity:u16
clause-message                             selector:symbol  has_arguments:u8
method                                     selector:symbol  access:access  arity:u16  code:code-id
module-section                             count:u32  module:module*count
module                                     origin:origin  id:text  version:module-version  root:object-id
origin/origin
origin/origin/project                      path:text
origin/origin/core                         file:text
origin/origin/none
module-version/module-version
module-version/module-version/unversioned
module-version/module-version/versioned    version:text
shipped-name-section                       count:u32  name:shipped-name*count
shipped-name                               module:symbol  declaration:symbol  kind:shipped-kind
shipped-kind/shipped-kind
shipped-kind/shipped-kind/module-instance
shipped-kind/shipped-kind/class
shipped-kind/shipped-kind/metaclass
shipped-kind/shipped-kind/level            clause:u32
shipped-kind/shipped-kind/level-metaclass  clause:u32
graph                                      count:u32  object:object*count  root_count:u32  root:value*root_count
object                                     class:class-reference  hash:u32  body:object-body
object-body/object-body
object-body/object-body/instance           count:u32  field:value*count
object-body/object-body/string             text:text
object-body/object-body/symbol             symbol:symbol
object-body/object-body/array              count:u32  element:value*count
object-body/object-body/closure            code:code-id  frame:object-id  copied_count:u16  copied:value*copied_count
object-body/object-body/class              stamp:u64  record:class-record
object-body/object-body/metaclass          stamp:u64  class:class-reference
object-body/object-body/byte-array         bytes:bytes
object-body/object-body/weak-array         count:u32  element:value*count
object-body/object-body/ephemeron          count:u32  field:value*count
object-body/object-body/mixin              mixin:reified-mixin  side:mixin-side
object-body/object-body/frame              frame:frame
object-body/object-body/instance-under-construction  count:u32  field:value*count
object-body/object-body/body-less-class    stamp:u64  record:class-record  has_enclosing:u8  enclosing:value?has_enclosing  has_declaring_class:u8  declaring_class:class-reference?has_declaring_class
class-record/mixin-reference
class-record/mixin-reference/row           row:row-id  has_superclass:u8  superclass:class-reference?has_superclass  has_enclosing:u8  enclosing:value?has_enclosing  has_declaring_class:u8  declaring_class:class-reference?has_declaring_class  has_clause:u8  applying_row:row-id?has_clause  clause:u32?has_clause  named:class-reference?has_clause
class-record/mixin-reference/shipped       name:shipped-index  has_superclass:u8  superclass:class-reference?has_superclass  applying_row:row-id  clause:u32  named:class-reference
reified-mixin/mixin-reference
reified-mixin/mixin-reference/row          row:row-id
reified-mixin/mixin-reference/shipped      name:shipped-index
mixin-side/mixin-side
mixin-side/mixin-side/instance
mixin-side/mixin-side/class
frame                                      kind:frame-kind  body:frame-body  program_counter:u32  uncontinuable:u8  receiver:value  has_sender:u8  sender:object-id?has_sender  has_home:u8  home:object-id?has_home  has_current_class:u8  current_class:class-reference?has_current_class  register_count:u32  register:value*register_count
frame-kind/frame-kind
frame-kind/frame-kind/method               selector:symbol  access:access
frame-kind/frame-kind/closure              closure:object-id
frame-kind/frame-kind/top-level
frame-kind/frame-kind/initializer          class:class-reference
frame-kind/frame-kind/own-initializer      class:class-reference
frame-kind/frame-kind/standalone-initializer  class:class-reference
frame-kind/frame-kind/superclass-clause    class:class-reference
frame-kind/frame-kind/factory              class:class-reference
frame-body/frame-body
frame-body/frame-body/none
frame-body/frame-body/code                 code:code-id
frame-body/frame-body/declared
class-reference/class-reference
class-reference/class-reference/record     object:object-id
class-reference/class-reference/shipped    name:shipped-index
value/value
value/value/nil
value/value/false
value/value/true
value/value/small-integer                  value:i64
value/value/float                          value:f64
value/value/object-id                      object:object-id
value/value/class-tag                      class:class-reference
value/value/large-integer                  negative:u8  digit_count:u32  digit:u64*digit_count
value/value/fraction                       numerator:value  denominator:value
value/value/shipped-module                 name:shipped-index
hash-table                                 count:u32  entry:hashed*count
hashed/hashed
hashed/hashed/shipped                      name:shipped-index  hash:u32
```

## The rules the decoder checks

A load refuses an image unless every rule below holds, and every rule the
bytecode chapter holds each of its bodies to
([The rules the decoder checks](bytecode.md#the-rules-the-decoder-checks)).
It checks them before any code of the image runs, and no byte sequence
makes it fail otherwise than by refusing
([`image.untrusted`](#r-image.untrusted)).

r[image.verify.counts]
1. Every count is held to the bytes that follow it: a load reserves room
   for no more of a count's items than the bytes left could hold, and
   refuses a count of more items than the bytes left hold, before it
   reads past them.

r[image.verify.ids]
2. Every id names an entry of the section it indexes, as
   [`image.structure`](#r-image.structure) types it -- a symbol, a body of
   the code table, a text of the text table, a row of the mixin table, a
   shipped name, a record of the object table -- and an id past its
   section is refused before it indexes anything.

r[image.limit.shipped-names]
3. The shipped names number at most 65,536; a count past it is refused
   where it is read.

r[image.limit.clause-index]
4. A clause index -- a level's, in a class record, and a level's or a
   level's metaclass's, in a shipped name -- is below 1,024, the most
   `<:` clauses a declaration has
   ([`image.limit.clauses`](#r-image.limit.clauses)); one at or past it
   is refused where it is read, before it is looked up, so that a reader
   of the shipped names that asks no shipped world refuses it too.

r[image.limit.clauses]
5. A mixin row has at most 1,024 `<:` clauses, the most a resolved
   declaration has; a count past it is refused where it is read, before
   anything is reserved for the clauses.

r[image.verify.texts]
6. Each text holds to itself: its range does not end before it starts;
   its holes lie within the range, in order, none empty and no two
   overlapping; its text is as long as the range less its holes; and every
   piece of it between two holes begins and ends on a character boundary.
   And each text holds to the rows ([`image.texts`](#r-image.texts)): a
   row's text index names a text of the table; every text is named by
   exactly one row that is no row's class side, and a class side's row
   names its declaration's text; and a text has as many holes as its
   declaration's row has nested declarations.

r[image.verify.mixin-rows]
7. A mixin row holds to the declaration it describes: its source range
   does not end before it starts; its factory and each of its methods
   take at most 255 parameters
   ([`bytecode.limit.arguments`](bytecode.md#r-bytecode.limit.arguments)),
   each method record's argument count being its selector's arity
   ([`bytecode.verify.attachment`](bytecode.md#r-bytecode.verify.attachment));
   no two of its members share a name -- its methods, its eager slots'
   getters, its mutable slots' setters, its nested declarations and, on a
   row that is a declaration's class side, that declaration's factory, two
   lazy slots of one name declaring one getter twice; every lazy slot has
   its getter among the row's methods, a method of the slot's name and
   access that takes no argument; a row with `<:` clauses has a superclass
   clause; and a row that writes no body has `<:` clauses and declares
   nothing -- no slot, no initializer expression, no method, no nested
   declaration and no class side.

r[image.verify.mixin-tree]
8. The rows form the forest [`image.mixins`](#r-image.mixins) gives: every
   row a row names as a nested declaration or a class side is a row of the
   table; walking each tree in preorder from its first row reaches every
   row once and in table order, so that no row is its own ancestor, none
   has two parents and none is a child of a row of another tree; no tree
   nests more than 130 rows deep, its root counted, the 128 nested classes
   the parser takes under a root and the innermost one's class side, an
   obsolete tree as a module's; the table counts no more obsolete trees
   than it holds trees, read before anything is split by it; and the trees
   but the obsolete ones are as many as the module section holds records.

r[image.verify.initializers]
9. A row's bodies hold to its declaration, each of the kind and the
   arity [`bytecode.verify.attachment`](bytecode.md#r-bytecode.verify.attachment)
   gives it: every row has a full initializer; a row with a slot or an
   initializer expression has its initializer's own part; a row has a
   standalone initializer exactly when it has `<:` clauses and writes a
   body; a row with a superclass clause has that clause's body; and a
   body-less row's full initializer, its declaration's own code, is named
   as no other body, of another row or of its own.

r[image.verify.module-identity]
10. Each module record names its module as a project names it
    ([`semantics.module.identity`](language/semantics.md#r-semantics.module.identity)):
    a `project` record's path is relative to the project -- separated by
    `/`, without a root or a `\`, holding no `:` anywhere, so neither a
    drive nor a `core:` path, without an empty or a `.` segment, a `..`
    only leading, ending in `.ns` and naming more than `.ns` -- and its id
    lies outside the namespace `newspeak.`; a `core` record's file is a
    name ending in `.ns`, naming more than `.ns` and holding no `/`, `\`
    or `:`, and the record is the manifest's alone, `newspeak.core.Manifest`
    at `core:Manifest.ns` without a version; a `none` record has no path,
    and its id lies outside the namespace `newspeak.`; every id is identifiers
    separated by dots, the last the name of its module's root
    declaration, the first row of the module's tree; every version is a
    semantic version without build metadata; and no path, id or version
    holds a control character or a format character (Unicode's general
    categories Cc and Cf).

r[image.verify.modules]
11. No two module records are of one id and one version, nor of one id
    where either has no version. Each record's root names a record of the
    graph that is its own module's root class: a class record applying
    the root row of its module's tree as the outermost level -- no
    metaclass, no level a clause added, no class of another row -- with
    neither an enclosing object nor a declaring class; for a root that
    writes no body, the body-less class record of that row, its
    declaration's enclosing object and declaring class absent.

r[image.verify.root-rule]
12. Each module's root class has a superclass chain the root rule builds
    ([`semantics.module.root-rule`](language/semantics.md#r-semantics.module.root-rule)):
    a root row with no `<:` clause has `Kernel Object` or `Kernel Value`
    as its superclass; a root row with one `<:` clause has the level that
    clause added, its clause index 0, applying `Kernel Value`'s mixin over
    `Kernel Object`; a root row with more is refused. A root that writes
    no body has one `<:` clause and is the level it adds, applying
    `Kernel Value`'s mixin over `Kernel Object` and naming `Kernel Value`
    as its clause's class.

r[image.verify.shipped-names]
13. Every shipped name is one of the loading binary's: its module one of
    the binary's shipped modules; its declaration that module's root --
    the module's own name -- or a class the root declares, a module
    instance's always the root; and a level's or a level's metaclass's
    clause one of that declaration's `<:` clauses. No name is listed
    twice. A load checks every name before it builds any part of the
    shipped world the names select
    ([`image.shipped-names`](#r-image.shipped-names)), so that an image
    it refuses builds nothing of it.

r[image.verify.started-image-names]
14. In a started image every shipped name is of the module `Kernel`, and
    none is a module instance, the kernel's included: before `main:args:`
    a program reaches nothing of the other shipped modules, and no
    program reaches a module's instance.

r[image.shipped.positions]
15. A shipped name stands only where what it names may stand. The places
    a record names a shipped object are six positions, and each allows
    what the table below gives; a load refuses a name anywhere else, and
    a writer refuses to write one. A module instance is no class and
    stands nowhere a class does. The VM mirror's class has no
    declaration, so no shipped name names it.

    | Position | Where it stands | What may stand there |
    |---|---|---|
    | a value | a slot, an element, a copied value, a root, a frame's register, an enclosing object, and every class written as a value (`class-tag`) | a shipped class that is public, or that a public class accessor of its module answers, and that class's metaclass -- but `Kernel IndexedCollection`, public only for another shipped module |
    | a superclass | a class record's superclass, the class a `<:` clause named, the shipped mixin a level applies | a class that may be a value, never its metaclass, and never `Behavior`, `Class` or `Metaclass` |
    | a record's class | an object record's class, a metaclass record's class | any shipped class but `Top` and a module's root, never a metaclass or a level; `Activation` only on a frame record, and `FutureImplementation` only on an unresolved future's instance record |
    | a frame's class | a frame's current class, the class its kind names, a class record's declaring class | any shipped class, metaclass or level, a module's root, its metaclass and its levels included |
    | a frame's register | a `shipped-module` value, a frame record's receiver or register | a module's instance, and only there |
    | a reified mixin's mixin | the shipped mixin a `mixin` record names | any shipped class, whatever its accessibility, a module's root included; never a metaclass, a level or a module instance |

    So `Top`, `Activation`, `FutureImplementation`, the module roots and
    the module instances are no value, and a private class without a
    public accessor, `NonBooleanReceiver` among them, stands only as a
    record's class, a frame's class or a reified mixin's mixin.

    A frame's receiver and registers take both kinds of name: a class
    written there as a value is held to the value position, and a module
    instance written there (`shipped-module`) to the frame's-register
    position.

r[image.verify.class-records]
16. A class record holds to the declaration it applies:
    - The row it applies is no row's class side, since a metaclass is a
      record of its own.
    - The row writes a body, since no class applies a body-less
      declaration's mixin.
    - A level a `<:` clause added names the applying row, a clause that
      row declares and the class that clause named. A level over a
      shipped mixin is always such a level.
    - The body-less class record is written exactly for the level that a
      body-less declaration's last clause adds. Every other class is
      written as a class record.
    - No class's superclass chain closes on itself, and neither do the
      classes the levels' clauses named.
    - An enclosing object -- a class's, or a body-less declaration's -- is
      an object, never a scalar.
    - Every field the format types as a class reference names a class
      object: a record's class, a superclass, a declaring class, the class
      a clause named, a frame's current class and the class its kind
      names.
    - An object that is no class object is no instance of `Class`,
      `Metaclass` or a metaclass.

r[image.verify.metaclass-records]
17. Class records and metaclass records pair one to one:
    - a class record's class is a metaclass record whose class is that
      class record;
    - a metaclass record's class is a class record;
    - every metaclass record is the class of exactly one class record;
    - a metaclass record's own class is `Kernel Metaclass`.

r[image.verify.frames]
18. A frame record holds to the body it ran:
    - Every field the format types as a frame -- a frame's sender and
      home, a closure's captured frame -- names a frame record.
    - A frame record's class is `Kernel Activation`.
    - Every frame record is marked as one that can no longer continue,
      since an image is written only when no activation runs.
    - A closure frame's kind names a closure record, and a frame has a
      home exactly when its kind is a closure's.
    - A frame writes its body as its kind allows:
      - a top-level frame names none;
      - a factory frame names its body by declaration, the factory its
        class derives;
      - a closure frame names a body of the code table;
      - every other kind names a body of the code table, or names it by
        declaration when it is a shipped declaration's body.
    - Its body is one its kind runs: a method frame's is a method body of
      its method's argument count, a closure frame's is its closure's own
      body, an initializer frame's is an initializer, a superclass clause
      frame's is a superclass clause, and a factory frame's is a method
      body.
    - A body-less declaration's own code runs in a frame exactly when the
      frame runs it at that declaration's class object.
    - Its program counter is an instruction index of its body.
    - Its registers are as many as its body's arguments and its registers
      above the header, together
      ([`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout)).
    - A closure's captured frame runs the body that creates the closure's
      body.

r[image.verify.byte-arrays]
19. A byte-array record is written only under `Kernel ByteArray`, and
    `Kernel ByteArray` is the class of no other record.

r[image.verify.weak-records]
20. A weak-array record is written only under `Kernel WeakArray`. The
    only other record of that class is an instance record of its slot
    count, none, as the instance `withoutSlots` makes. An ephemeron record
    is written only under `Kernel Ephemeron`, and `Kernel Ephemeron` is
    the class of no other record. An ephemeron record holds exactly as
    many values as its class has slots, the key first.

r[image.verify.mixin-records]
21. A mixin record is written only under `Kernel Mixin`, and `Kernel
    Mixin` is the class of no other record.
    - It names the instance side of a class's mixin: a row that is no
      row's class side, or a shipped name of a class -- never of a module
      instance, a metaclass or a level.
    - It names the side it stands for.
    - No two records stand for one mixin, and none stands for a mixin
      whose object the loading world already holds.

r[image.verify.symbols]
22. A symbol is the one object of its text (§5.1.6), so:
    - a symbol record is written only under `Kernel Symbol`, and a string
      record never is;
    - no two symbol records name one symbol's text;
    - no symbol record names a text the loading world already holds a
      `Symbol` of.

r[image.verify.program-instances]
23. Every object of a class the image's modules declare is an instance
    record of exactly its class's slot count, as every such instance is
    made and as an edit migrates it. A class object is the exception: it
    is written as a class record.

r[image.verify.under-construction]
24. Only an instance of a class that inherits `Value` is written as an
    `instance-under-construction` record: an instance of a value class
    whose construction was cut short before the image was written
    ([`semantics.value.under-construction`](language/semantics.md#r-semantics.value.under-construction)).
    A load rebuilds it still under construction, so that it answers no
    value question after the load either, and refuses the record for an
    object of any other class. The record is laid out as an `instance`
    record and held to every rule an instance record is held to.

r[image.verify.shipped-instances]
25. Every object of a shipped class whose layout the loading binary
    declares is an instance record of exactly that class's slot count.
    That is every shipped class but those whose objects are written as
    records of their own: arrays, strings, symbols, closures, classes,
    metaclasses, byte arrays, weak arrays, ephemerons, mixins and frames.
    Two classes are held further:
    - an instance record under `Kernel FutureImplementation` is an
      unresolved future's: two values, the second the state 0;
    - an instance record under `Kernel Fraction` holds no value, as
      `Fraction new` makes it, since a fraction is written as a value.

r[image.verify.value-classes]
26. A load does not trust the image about whether a class meets the
    value-class conditions (§3.1.1). It decides them again for every
    class the image carries, as building the class from its source
    decides them. So a value class whose enclosing objects are not all
    values refuses to be instantiated after a load, as it did before.

r[image.verify.numbers]
27. A number is written in its one canonical form:
    - A `LargeInteger` holds to
      [`bytecode.verify.large-integer`](bytecode.md#r-bytecode.verify.large-integer):
      its sign byte is 0 or 1, its value does not fit an `i64`, its most
      significant digit is not 0, and it is not a negative zero.
    - A fraction's numerator and denominator are each an integer value,
      its denominator is greater than 1, and the two share no factor.

r[image.verify.hashes]
28. Every identity hash fits the 22 bits an object's header holds: it is
    below 4,194,304. A tag-named hash is never 0, and each shipped name
    has at most one. Every class, body-less class and metaclass record's
    creation stamp is below 2^59, half the `SmallInteger` range, so that
    the loading world keeps stamps to draw above it, and no two records
    have the same.

r[image.verify.started-image-roots]
29. A started image has exactly one root, the application `load` starts.

r[image.verify.mirror]
30. No image holds the VM mirror, nor an instance of its class or of
    `Top`, and neither class stands where a program could reach it
    ([`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror)):
    - the mirror's class has no declaration, so no shipped name names it
      or its metaclass;
    - `Top` is never a value, a superclass or a record's class. It stands
      only as a frame's class and as a reified mixin's mixin, which
      carries no authority
      ([`image.shipped.positions`](#r-image.shipped.positions));
    - a writer refuses a graph that reaches the mirror; one that reaches
      the mirror's class or its metaclass, wherever either stands; one
      that holds a reified mixin of the mirror's class, of either side;
      and one that puts `Top` or its metaclass where
      [`image.shipped.positions`](#r-image.shipped.positions) does not let
      it stand. It writes no image of any of them.

r[image.verify.not-checked]
A load checks no more of an image than these rules and the bytecode
chapter's. In particular it leaves these to the run, where they are
checked or cannot go wrong:

- **A ready-made instance's slots against §3.1.1's value conditions.** An
  instance of a value class is rebuilt from its record, and its slots
  are not initialized again. A crafted image can therefore hand back a
  value whose slot holds a mutable object. That is a wrong answer, never
  a crash: every slot access is bounded by its object. (`docs/OPEN-WORK.md`,
  "Known defects and gaps".)
- **A second class record applying a module's root row**, beside the one
  the module section names. It builds a class of that declaration, as a
  run could, and the module's root stays the record the section names.
  (`docs/OPEN-WORK.md`, "Planned: images, liveness and reflection".)
- **That a level sits directly above a class of its applying row.** The
  initializer instructions check every level they reach as they run, and
  signal an error rather than reach past it. (`docs/OPEN-WORK.md`,
  "Known defects and gaps".)

What a body leaves to the run is given by
[`bytecode.verify.not-checked`](bytecode.md#r-bytecode.verify.not-checked).

## Bounds

r[image.bound.sizing]
Before it allocates any object of the graph, a load sizes every record of
the object table from the record's own bytes. It refuses the image, with
nothing of the graph allocated, when a record claims more than its bytes
hold, or when the objects the records describe would take more than eight
words a byte of the input left from the object table's start. Every
record's object is bounded by its own record's bytes, at about a word a
byte, so the format's records never reach the eight-word limit: that
refusal is a guard.

r[image.bound.linear]
What a load that refuses an image allocates is bounded by a constant
multiple of the image's length plus a constant, and nothing of it
outlives the load.

The factors depend on the build and are the implementation's:
`docs/design/memory.md`, "The amplification bound of a rejected load".

## Running a decoded image

r[image.run]
A load runs none of the image's code before the application starts. The
shipped modules it builds first are built by the binary's own code
([`image.verify.shipped-names`](#r-image.verify.shipped-names)).

Once it runs, a decoded image's code runs as compiled code would, with
the exposure of running its program from source:
- Every body holds to the bytecode chapter's rules, among them that a
  send passes as many arguments as its selector takes
  ([`bytecode.verify.send-arity`](bytecode.md#r-bytecode.verify.send-arity)).
  So whatever goes wrong in it goes wrong as in a run from source: as a
  Newspeak error, never as a crash of the process.
- Its recursion is bounded by the depth limit
  ([`runtime.depth-limit`](runtime.md#r-runtime.depth-limit)).
- Its blocks nest no deeper than a source's
  ([`bytecode.verify.blocks`](bytecode.md#r-bytecode.verify.blocks)), so
  the activations it stacks are bounded as a source program's are.

## Loading an image

### LoadImage(bytes)

r[image.load]
Decodes *bytes* into a world of its own and hands the host what it holds.
A step that refuses ends the load with an error, and the host receives
nothing of the world: what the load built, the shipped modules step 6
built included, is dropped with it
([`image.bound.linear`](#r-image.bound.linear)).

1. Read the header ([`image.header`](#r-image.header)). Refuse *bytes*
   unless its magic, its version and its flags are the format's
   ([`image.header.flags`](#r-image.header.flags)) and its fingerprint is
   the loading binary's
   ([`image.header.fingerprint`](#r-image.header.fingerprint)); a header
   that fails reads nothing past itself.
2. Let *world* be a new world, booted as the loading binary boots every
   run: the kernel and the platform's module built and the platform
   made. The image is decoded into *world*, never into a world a host
   already runs.
3. Read the symbol table ([`image.symbols`](#r-image.symbols)) and the
   code table ([`image.code-table`](#r-image.code-table)), holding every
   body to the bytecode chapter's rules
   ([The rules the decoder checks](bytecode.md#the-rules-the-decoder-checks))
   but the attachment rule
   ([`bytecode.verify.attachment`](bytecode.md#r-bytecode.verify.attachment)),
   which step 4 checks as it reads the mixin table.
4. If the header sets `has-source`, read the text table
   ([`image.texts`](#r-image.texts)). Read the mixin table
   ([`image.mixins`](#r-image.mixins)) and the module section
   ([`image.modules`](#r-image.modules)), the module section's records
   naming the mixin table's trees but its obsolete ones, which load
   obsolete ([`image.load.obsolete`](#r-image.load.obsolete)), and hold
   them and the texts to rules 5 to 10 ([`image.limit.clauses`](#r-image.limit.clauses) to
   [`image.verify.module-identity`](#r-image.verify.module-identity)) and
   to the first half of rule 11, no two records of one module
   ([`image.verify.modules`](#r-image.verify.modules)). Every count and
   every id, here and in every later step, is held to rules 1 and 2
   ([`image.verify.counts`](#r-image.verify.counts),
   [`image.verify.ids`](#r-image.verify.ids)).
5. Read the shipped names
   ([`image.shipped-names`](#r-image.shipped-names),
   [`image.limit.shipped-names`](#r-image.limit.shipped-names),
   [`image.limit.clause-index`](#r-image.limit.clause-index)), and check
   every one of them against the loading binary
   ([`image.verify.shipped-names`](#r-image.verify.shipped-names),
   [`image.verify.started-image-names`](#r-image.verify.started-image-names))
   before step 6 builds any part of the shipped world: an unknown name, a
   name given twice and, in a started image, a name of any module but the
   kernel or of a module's instance refuse *bytes* with nothing built.
6. Build in *world*, by the loading binary's own code, every shipped
   module and class the names name that *world* has not built yet: a
   module as a program's first demand for it builds it, a class as its
   accessor answers it. Each is built once. This runs the binary's code
   alone, none of the image's ([`image.run`](#r-image.run)).
7. Resolve each shipped name to *world*'s own object of that name.
8. Decode the graph ([`image.graph`](#r-image.graph)): every record of
   the object table sized from its own bytes before any is made
   ([`image.bound.sizing`](#r-image.bound.sizing)), then each made as a
   new object of *world* ([`image.graph.record`](#r-image.graph.record)),
   each value as [`image.graph.value`](#r-image.graph.value) gives it,
   and the roots. Hold each module record's root to its module, the
   second half of rule 11 and rule 12
   ([`image.verify.modules`](#r-image.verify.modules),
   [`image.verify.root-rule`](#r-image.verify.root-rule)); read the
   tag-named hashes ([`image.hashes`](#r-image.hashes)); and hold the
   graph to rules 15 to 30
   ([`image.shipped.positions`](#r-image.shipped.positions) to
   [`image.verify.mirror`](#r-image.verify.mirror)), each a record shows
   alone as it is read and the rest once every record exists. Refuse a
   byte past the hashes ([`image.trailing-bytes`](#r-image.trailing-bytes)).
9. Answer to the host *world*; each module, in module order, the
   application's first, with its root class -- the record its module
   record names -- its root declaration, its identity and, when the
   header sets `has-source`, its source text; the roots, in their order;
   and whether the header sets `start`.
10. A host that starts the image -- `ns-next load` does -- refuses it
    unless the header sets `start` and the image has one root, with
    ``error: <path> is not a started image: `build` writes one, with the application as its only root``,
    running nothing. Otherwise it answers
    [Send](runtime.md#r-runtime.send)(the root, `main:args:`, *world*'s
    platform and an `Array` of the program's arguments as `String`s), as
    a run from source sends it
    ([`semantics.program.main`](language/semantics.md#r-semantics.program.main)).

No step before step 10 runs the image's code. The `images` profile of
the suite runs every golden through `build` and then `load` and holds it
to the outcome recorded for its run from source: the check of a round
trip as a whole.

### What a program observes after a load

r[image.load.shipped-world]
Every shipped object an image names is the loading world's own: a name
resolves to the very object the loading binary's world holds under it --
a class answers `==` to what its accessor answers there -- built by the
binary's own code if the world had not built it yet, and never built
twice. The image chooses which of the binary's modules and classes a load
builds, never what they are: their code and their layout are the
binary's.

r[image.load.identity]
Two references to one record load as references to one object, and that
object is new: no object of the world that wrote the image is the object
a load of it makes, in the same process or in another. A value written by
value -- `nil`, `true`, `false`, an integer, a float and a fraction --
loads as an equal value of the same class
([`semantics.value.identity`](language/semantics.md#r-semantics.value.identity)).

r[image.load.hashes]
A record's object answers the identity hash the record was written with
([`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash)),
and an object never asked for one before the write is still unasked after
the load. A shipped object a tag-named hash names answers that hash; one
none names is as the loading world's boot left it. A hash given after the
load is past every hash the image holds, so it collides with none of them
until the 22-bit hashes wrap.

r[image.load.class-stamps]
A class object loaded from a record keeps the creation stamp the record
was written with, so that the class objects of an image keep the order
they were made in; a class object made after the load is ordered after
every one the image holds
([`runtime.install.superclass`](runtime.md#r-runtime.install.superclass)).

r[image.load.symbols]
A symbol record's object is new, and it becomes the loading world's
canonical symbol of its text: `==` to every symbol of that text the
loaded code makes, a literal `#foo` included. A world that already holds
a symbol of that text refuses the image
([`image.verify.symbols`](#r-image.verify.symbols)), so no record ever
stands beside another symbol of its text.

r[image.load.applications]
A class's behaviour is rebuilt from its mixin and its superclass, as
[ApplyMixin](runtime.md#r-runtime.apply-mixin) builds it, so two class
objects that shared one application before the write share one after it.
Nothing about an application is read from the bytes, and whether a class
meets the value conditions is decided again
([`image.verify.value-classes`](#r-image.verify.value-classes)).

r[image.load.caches]
An image carries no cache and no machine code: after a load every cache
holds only what the loading world's boot left in it, and the first send
at each site of the image's code decides afresh, as in a world that
never ran that code.

r[image.load.weak]
A weak array's slots and an ephemeron's fields hold what their records
wrote: a load clears nothing, not even an element or a key nothing else
in the graph reaches. The next collection treats them as
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak) and
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron)
say.

r[image.load.obsolete]
An obsolete tree loads obsolete, as the install that removed its root
left it in the world that wrote the image
([`runtime.install`](runtime.md#r-runtime.install)): its root is nested
in no declaration and no module's tree holds it, so no module's source
text holds its text and no enclosing class answers its name; its class
objects, its subclasses and its instances run its last version, and its
declarations' texts are kept. A full collection frees it once nothing
reaches it, as it frees a removed declaration in a running world. An
image written from the loaded world writes it again as an obsolete tree
while the graph reaches it.

r[image.load.modules]
Each module keeps the identity its record gives -- its path relative to
its project, its id and its version ([`image.modules`](#r-image.modules)):
the host receives it, the module's root declaration carries it, and the
tools name the module and its methods by it
(`<id>[@<version>][/<Inner>(.<Inner>)*][ class]>><selector>`), never by a directory of the
machine that wrote the image or of the one that loads it.

r[image.load.start]
The platform a started image's application receives in `main:args:` is
the first it ever sees: a started image holds no platform and names
nothing of the shipped world but the kernel's classes, their metaclasses
and their levels, never a module's instance
([`image.verify.started-image-names`](#r-image.verify.started-image-names)),
and a writer refuses an application that holds one, with `image encoding
refuses PlatformFactory Platform in a started application: nothing
before main:args: reaches it`. A load runs no factory and no initializer
again: the application holds what it held when it was written.

r[image.load.snapshot]
An image without `start` is decoded under the same rules but those a
started image alone is held to
([`image.verify.started-image-names`](#r-image.verify.started-image-names),
[`image.verify.started-image-roots`](#r-image.verify.started-image-roots)).
Its frames may hold the kernel, a module's instance or an `Activation` in
their registers, as frames of shipped code do in a running world, and a
crafted frame could hand one to the program's code. So a host loads such
an image only when it decides to trust it, and the load still never
panics (`docs/OPEN-WORK.md`, "A running world's image is trusted").

## Writing an image

### WriteImage(modules, roots, start)

r[image.write]
Writes an image of a world at rest and answers its bytes. *modules* is a
list of modules, each given by its root class, its identity -- its path
relative to its project, its id and its version
([`semantics.module.identity`](language/semantics.md#r-semantics.module.identity))
-- and whether the image carries its source; *roots* is a list of values;
*start* says whether the image is a started one. A step that refuses ends
the write with an error, and nothing is written. A write runs no code, the
program's or the binary's, and changes nothing of the world it reads.

1. Refuse if an activation is running or suspended, with
   ``image encoding cannot snapshot a world mid-execution: a snapshot is taken only between sends, when no activation is running or suspended``:
   an image holds a frame only as one that can no longer continue
   ([`image.verify.frames`](#r-image.verify.frames)).
2. Refuse the modules wherever a load would refuse their records
   ([`image.write.refusals`](#r-image.write.refusals)): a root declaration
   the root rule forbids, an identity the project's rules refuse, two
   modules no request could tell apart, and source asked for some of the
   modules but not for all.
3. Let the declarations be each module's root declaration and every
   declaration it nests, each as it now is
   ([`image.write.edited`](#r-image.write.edited)), one tree per module in
   the order of *modules*: the first rows of the mixin table
   ([`image.mixins`](#r-image.mixins)). Refuse a tree nested more than
   130 rows deep, which the loader would refuse
   ([`image.verify.mixin-tree`](#r-image.verify.mixin-tree)).
4. Walk the graph: every object *roots* reach, in their order, and then
   every object each module's root class reaches, in the order of
   *modules*, so that a module the roots never reach -- one carried only
   for its code -- still has its root class's record. An object of the
   shipped world is not entered: it is written as its shipped name
   ([`image.write.not-written`](#r-image.write.not-written)). A value
   written by value takes no record
   ([`image.graph.value`](#r-image.graph.value)). A weak array's slots
   and an ephemeron's fields are followed as
   [`image.write.weak`](#r-image.write.weak) gives. Refuse a module's root
   class that is not its module's record, breaks the root rule or is
   shipped, and a graph that reaches the VM mirror, its class or a shipped
   class no shipped name names
   ([`image.write.refusals`](#r-image.write.refusals)). Then let the
   obsolete declarations be every declaration an install removed that the
   graph reaches -- one a class object applies, declares or adds through a
   `<:` clause, is nested in, one a reified mixin stands for, and one a
   closure or a frame runs a body of, of its last version or of one an
   install superseded -- in the order the world registered them, whether
   or not *modules* holds the module it was removed from. Each, with every
   declaration nested in it as it last was, is a tree of the mixin table
   after the modules' trees, held to the same ceiling; a removed
   declaration nothing reaches is not written.
5. Write the code table ([`image.code-table`](#r-image.code-table)): every
   body the declarations and the obsolete declarations carry, the blocks
   nested in them, and every body a closure or a frame of the graph runs
   that an older version of one of them compiled
   ([`image.write.edited`](#r-image.write.edited)). Refuse a closure or a
   frame that runs any other body.
6. If the modules carry source, write the text table
   ([`image.texts`](#r-image.texts),
   [`image.write.source`](#r-image.write.source)), refusing a module or
   an obsolete declaration built without its text. Write the mixin table,
   the modules' trees, the obsolete trees and their count, and the module
   section, a record for each module in the order of
   *modules*, naming its root class's record
   ([`image.modules`](#r-image.modules)).
7. Write the object table, a record for each object the walk entered, and
   the roots, in their order ([`image.graph`](#r-image.graph)). Every
   program class, level and reified mixin the graph reaches is of one of
   the declarations or of an obsolete one: refuse one of a module not
   among *modules*, and a
   shipped name where [`image.shipped.positions`](#r-image.shipped.positions)
   does not let it stand ([`image.write.refusals`](#r-image.write.refusals)).
   Then write the tag-named hashes: the identity
   hash of every object of the shipped world that has been given one,
   whether or not the graph reaches it ([`image.hashes`](#r-image.hashes)).
8. If *start*, refuse an image whose shipped names -- those of the graph
   and those of the tag-named hashes -- name anything but the kernel's
   classes, their metaclasses and their levels
   ([`image.write.started`](#r-image.write.started)).
9. Answer the header ([`image.header`](#r-image.header)) -- the writing
   binary's fingerprint, `start` set if *start*, and `has-source` set if
   the modules carry source -- followed by the sections in the order
   [`image.sections`](#r-image.sections) gives: the symbol table, holding
   every symbol the other sections name, the code table, the text table,
   the mixin table, the module section, the shipped names the graph and
   the hashes name, each once, the graph and the hashes.

A host writes a snapshot as *WriteImage*(*modules*, *roots*, false), from
whatever roots it names. `ns-next build` writes a started image
([`image.write.started`](#r-image.write.started)): its *modules* are the
program's in module order -- the application module first, a source file
or a project's `main`, then the project file's `modules` in the order
written, then, for a project, the embedded manifest
([`semantics.module.manifest`](language/semantics.md#r-semantics.module.manifest))
-- each with source unless `--strip-source` is given. A load rebuilds the
modules in that order ([`image.load`](#r-image.load), step 9), which
keeps the identity hashes the boot draws the same across `run`, `build`
and `load`.

### What a write writes

r[image.write.started]
`build` assembles the application exactly as `run` does -- the root
class's factory and every initializer run, with no platform in existence
-- and writes it just before `main:args:` would be sent, as
*WriteImage*(the program's modules, « the application », true): an image
whose header sets `start` and whose graph has the application as its one
root ([`image.verify.started-image-roots`](#r-image.verify.started-image-roots)).
An error raised while assembling fails the build, with the status and the
report `run` gives the same error. The image holds no platform: nothing
before `main:args:` hands a program one, and a started image names
nothing of the shipped world but the kernel's classes, their metaclasses
and their levels ([`image.verify.started-image-names`](#r-image.verify.started-image-names)).
A writer refuses a started image that would name anything else, with
`image encoding refuses <name> in a started application: nothing before main:args: reaches it`
-- for an application that holds the platform,
`image encoding refuses PlatformFactory Platform in a started application: nothing before main:args: reaches it`.
The names it checks include those of the tag-named hashes, so a shipped
object of another module that a host has given an identity hash refuses
the write even where the graph does not reach it; no program reaches such
an object before `main:args:`.

r[image.write.not-written]
An image carries nothing of the shipped world: every object of it the
graph reaches -- a shipped module's instance, a class its root declares,
the builtins among them, their metaclasses and the levels their `<:`
clauses add -- is written as its shipped name
([`image.shipped-names`](#r-image.shipped-names)), never its code, its
layout or its state, and a load resolves the name to its own object
([`image.load.shipped-world`](#r-image.load.shipped-world)). `nil`, `true`
and `false` are written as their value tags
([`image.graph.value`](#r-image.graph.value)). No mixin application, no
cache entry, no dispatch id and no binding of a name to what it resolved
to is written: a load rebuilds the applications from the mixins
([`image.load.applications`](#r-image.load.applications)) and starts with
empty caches ([`image.load.caches`](#r-image.load.caches)), and the
compiled code names no binding.

r[image.write.weak]
A writer writes what a collection would keep
([`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron)). A
weak array's slot names its object only if the graph reaches that object
strongly otherwise, wherever in the walk that strong path lies, and is
written `nil` if not. An ephemeron's value and finalizer are walked only
once its key is reached -- through another ephemeron's value too, however
long the chain -- and an ephemeron whose key the graph never reaches is
written with every field `nil`. A weak slot holding what takes no record
-- an immediate, a shipped name, or a value written by value -- is
written as it is, whether or not anything else reaches it: a
collection's timing is unspecified, and keeping what a collection might
clear is never wrong. What a weak slot holds is still met by the walk for
what it refuses: a weak slot holding the VM mirror or its class refuses
the write as a strong one does
([`image.write.refusals`](#r-image.write.refusals)).

r[image.write.lazy-slots]
A lazy slot ([`semantics.slot.lazy`](language/semantics.md#r-semantics.slot.lazy))
is written with its value, as any slot is: a slot filled before the write
answers that value after a load without running its initializer again,
and an empty one runs its initializer at its first read after it. §6.3.3
intends that a lazy slot need never be persisted; whether an image should
carry its value is open (`docs/OPEN-WORK.md`, "Whether an image carries a
lazy slot's value").

r[image.write.source]
An image with source carries every declaration's text as the world now
holds it -- each declaration's text once, never the file a module was
read from, and no text an older version of a declaration held -- and an
image without source carries no program text at all. Source is carried
for every module or for none, since the header has one flag for all of
them ([`image.header.flags`](#r-image.header.flags)); a write asked for
the source of some modules only is refused with
`image encoding carries source for every module or for none: one module has none`.
A module built without its text -- one an image without source loaded --
cannot be written with source, and the write is refused with
``image encoding cannot carry the source of module `<name>`, which was built without it``;
an obsolete declaration built without its text, with
``image encoding cannot carry the source of the removed class `<name>`, which was built without it``.

r[image.write.edited]
A program a MirrorBuilder edited
([Install](runtime.md#r-runtime.install)) is written as it now is: each
edited declaration is the row of the declaration it replaced, with its
code, its slots and its text as they now are, and every class object and
reified mixin built over any version of a declaration names that
declaration's one row. The code table also carries every body an older
version of a carried declaration compiled that a closure or a frame of
the graph still runs, so that a closure of a removed or a replaced method
runs its old code after a load. A body no version of a carried
declaration compiled is refused: a closure over one with
`image encoding cannot serialize a closure over code that no module the image carries compiled`,
and a frame running one with
`image encoding cannot serialize a frame running code that no module the image carries compiled`.

r[image.write.deterministic]
One world writes one byte sequence: *WriteImage* with the same modules,
roots and *start* answers the same bytes each time, whatever order the
writer's own tables are kept in, and before and after a collection that
moved every object. Two worlds a program built alike from the same
sources write the same bytes too.

r[image.write.refusals]
A writer refuses to write what a load would refuse, so that no image it
writes fails to load for its structure. Each refusal names what it
refuses:

- **A module root the root rule forbids**
  ([`image.verify.root-rule`](#r-image.verify.root-rule),
  [`image.verify.modules`](#r-image.verify.modules)): a root declaration
  whose superclass clause the root rule does not allow, with
  `image encoding cannot serialize <name>: <what the root rule says of it>`,
  among them
  `image encoding cannot serialize Root: a module's root class may inherit only Object or Value, or write Object <: Value, and its superclass clause names Array`;
  a root class whose superclass chain the root rule does not build, with
  `image encoding cannot serialize <class> as a module's root class: it does not inherit what the root rule allows, Object or Value, or Value applied over Object`;
  a root class that is no record of its module's root, with
  `image encoding cannot serialize <class> as a module's root class: it is a metaclass, a level, a class applying another declaration's mixin, or a class with an enclosing object or a declaring class, such as a nested class`;
  and a shipped class handed as a module's root class, which is a shipped
  name and never a record, with
  `image encoding cannot serialize <class> as a module's root class: it is shipped`.
- **A class tree the mixin table cannot hold**
  ([`image.verify.mixin-tree`](#r-image.verify.mixin-tree)): one nested
  more than 130 rows deep, with
  `image encoding refuses a class tree nested <depth> levels deep: the mixin table's ceiling is 130 levels, and an image past it could never be loaded`.
  No parsed source nests that deep; only a host's tree does.
- **A class, a level, a reified mixin or a body of a module the image
  does not carry** ([`image.verify.ids`](#r-image.verify.ids)): a class
  object applying such a module's mixin, with
  ``image encoding cannot serialize a class object applying mixin `<name>`, which is not part of any module the image carries``;
  a level such a module's `<:` clause added, with
  ``image encoding cannot serialize a level added by class `<name>`, which is not part of any module the image carries``;
  a reified mixin of one of its declarations, with
  `image encoding cannot serialize the reified mixin <name>, which is not part of any module the image carries`;
  and a closure or a frame running one of its bodies, with the refusals
  [`image.write.edited`](#r-image.write.edited) gives. A declaration an
  install removed is none of these: it is written as an obsolete tree,
  whichever module it was removed from ([`image.write`](#r-image.write),
  step 4).
- **A module root that is no class object**, with
  `image encoding's root object is not a class`.
- **An object whose record the loader would refuse**
  ([`image.verify.metaclass-records`](#r-image.verify.metaclass-records),
  [`image.verify.shipped-instances`](#r-image.verify.shipped-instances)): a
  class object whose header is not its metaclass, with
  `image encoding cannot serialize a class object without a metaclass`;
  and an unresolved future whose fields a program's code corrupted, with
  `image encoding cannot serialize a malformed future`. Three more guard
  what no host can reach today: a body-less declaration's class object
  without its declaration's scope, with
  `image encoding cannot serialize a body-less declaration's class object without its declaration's scope`,
  since every class object has the room for one;
  a reified mixin of a class side whose instance side is gone, with
  `image encoding cannot serialize the reified mixin of a class side whose instance side no longer exists`,
  since a class side lives no longer than its instance side; and a row
  without its text in an image with source, with
  ``image encoding cannot carry the source of `<name>`, which was built without it``,
  since the module's own refusal
  ([`image.write.source`](#r-image.write.source)) comes first.
- **The VM mirror, its class, `Top` and their metaclasses, anywhere in the
  graph** ([`image.verify.mirror`](#r-image.verify.mirror)): the mirror,
  checked before every other refusal of what the graph reaches, with
  `image encoding refuses the VM mirror: it is a capability the bootstrap hands to the core library alone, and an artifact that carried it would hand it to whatever loaded it`;
  the mirror's class and its metaclass, with
  `image encoding refuses the VM mirror's class: it has no declaration, so no shipped name names it`
  and
  `image encoding refuses the VM mirror's class class: it has no declaration, so no shipped name names it`;
  a reified mixin of the mirror's class, of either side, with
  `image encoding refuses the mixin of the VM mirror's class: it has no declaration, so no shipped name names it`
  and
  `image encoding refuses the mixin of the VM mirror's class class: it has no declaration, so no shipped name names it`;
  and `Top` and its metaclass wherever
  [`image.shipped.positions`](#r-image.shipped.positions) does not let
  them stand, as every shipped name (below).
- **A shipped object where [`image.shipped.positions`](#r-image.shipped.positions)
  does not let it stand**: a shipped name, with
  `image encoding refuses <name> as <position>: no program may name it there, and the loader refuses an image that does`
  -- `image encoding refuses Kernel Top as a value: no program may name it there, and the loader refuses an image that does`
  among them -- and a module's instance outside a frame's registers, with
  `image encoding refuses the module instance <module> outside a frame: it holds the core library's state`.
  A weak slot holding the mirror or its class is refused as a strong one
  is ([`image.write.weak`](#r-image.write.weak)).
- **A shipped object no shipped name names**
  ([`image.verify.shipped-names`](#r-image.verify.shipped-names)): a class
  object compiled from shipped source that the shipped world does not
  record -- one a second instance of a shipped module's root builds --
  with
  `image encoding refuses the class object <class>: it is compiled from shipped source, and no shipped name names it`.
  A level a program's `<:` clause added over a shipped mixin that no
  shipped name names is refused, with
  ``image encoding cannot serialize a level that `<:` clause <n> of class `<name>` added: it applies the shipped mixin <mixin>, which no shipped name names``:
  a clause naming a shipped metaclass applies a shipped class side, and
  no shipped name names a level over one, so
  ``image encoding cannot serialize a level that `<:` clause 1 of class `Listy` added: it applies the shipped mixin class side, which no shipped name names``
  refuses a level over `List class`, while one over `List` is written
  (`docs/OPEN-WORK.md`, "A level over a shipped class side cannot be
  written into an image"). One more refusal of this kind guards what no
  host can reach today: a reified mixin compiled from shipped source that
  no shipped name names, with
  `image encoding refuses the reified mixin <name>: it is compiled from shipped source, and no shipped name names it`;
  a reified mixin names its instance side, and every instance side
  compiled from shipped source is the mixin of a class the shipped world
  names or will build under a name.
- **A module identity the project's rules refuse**
  ([`image.verify.module-identity`](#r-image.verify.module-identity),
  [`image.verify.modules`](#r-image.verify.modules)): one module's, with
  `image encoding cannot name module <index> so: <the rule's refusal>`,
  such as
  ``image encoding cannot name module 0 so: `Other` does not end in the class's name `Util` ``;
  and two modules no request could tell apart, with
  `image encoding cannot name its modules so: <the rule's refusal>`.
- **Source for some modules only**
  ([`image.header.flags`](#r-image.header.flags),
  [`image.verify.texts`](#r-image.verify.texts)), and source for a
  module built without it, with the refusals
  [`image.write.source`](#r-image.write.source) gives.

r[image.write.limits]
Every object id and every object count an image holds is a `u32`, so an
image holds fewer than 2^32 objects; a writer handed a graph of more
aborts today, where it should refuse (`docs/OPEN-WORK.md`, "Writing an
image of more than about four billion objects aborts instead of
refusing"). A length a running program decides that does not fit its
field is refused instead,
such as a register list of more than 65,535 registers, with
`image encoding cannot write a register list of 65536: it does not fit the image format's 16-bit field`;
and a body past the argument limit
([`bytecode.limit.arguments`](bytecode.md#r-bytecode.limit.arguments)),
which the decoder refuses, is refused with
`image encoding cannot write a <what> of <count>: it is past the limit of 255 arguments`
-- a body's, a closure's, a factory's or a method's -- such as
`image encoding cannot write a body arity of 256: it is past the limit of 255 arguments`;
a body arity the compiler saturated at 65,535 with
`image encoding cannot write a body arity of 65535 or more: it is past the limit of 255 arguments`.
Every other 16-bit field a program decides -- a register list's length, a
closure's copied value count -- is refused past 65,535 with
`image encoding cannot write a <what> of <count>: it does not fit the image format's 16-bit field`.
A string's, a symbol's or a byte array's length, a value list's and an
integer's digit count past a `u32` are refused likewise, with
`image encoding cannot write a <what> of <length>: it does not fit the image format's 32-bit field`;
no test writes one, since it takes an object of more than four billion
bytes or elements.
