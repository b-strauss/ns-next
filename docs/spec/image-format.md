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
its source; "Running a decoded image" states what bounds it.

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
header-flag      retired                  1
header-flag      retired                  2
header-flag      start                    4
header-flag      has-source               8
value            nil                      0
value            false                    1
value            true                     2
value            small-integer            3
value            float                    4
value            object-id                5
value            class-tag                6
value            retired                  7
value            retired                  8
value            large-integer            9
value            fraction                 10
value            shipped-module           11
object-body      instance                 0
object-body      string                   1
object-body      symbol                   2
object-body      array                    3
object-body      retired                  4
object-body      retired                  5
object-body      closure                  6
object-body      class                    7
object-body      metaclass                8
object-body      retired                  9
object-body      retired                  10
object-body      byte-array               11
object-body      weak-array               12
object-body      ephemeron                13
object-body      mixin                    14
object-body      frame                    15
object-body      body-less-class          16
mixin-reference  row                      0
mixin-reference  retired                  1
mixin-reference  shipped                  2
mixin-side       instance                 0
mixin-side       class                    1
class-reference  retired                  0
class-reference  retired                  1
class-reference  record                   2
class-reference  retired                  3
class-reference  retired                  4
class-reference  retired                  5
class-reference  retired                  6
class-reference  retired                  7
class-reference  shipped                  8
frame-kind       method                   0
frame-kind       closure                  1
frame-kind       top-level                2
frame-kind       initializer              3
frame-kind       superclass-clause        4
frame-kind       retired                  5
frame-kind       retired                  6
frame-kind       own-initializer          7
frame-kind       standalone-initializer   8
frame-kind       factory                  9
frame-body       none                     0
frame-body       code                     1
frame-body       declared                 2
hashed           retired                  0
hashed           retired                  1
hashed           retired                  2
hashed           retired                  3
hashed           retired                  4
hashed           retired                  5
hashed           retired                  6
hashed           shipped                  7
slot-kind        immutable                0
slot-kind        mutable                  1
slot-kind        lazy-immutable           2
slot-kind        lazy-mutable             3
origin           retired                  0
origin           retired                  1
origin           project                  2
origin           core                     3
module-version   unversioned              0
module-version   versioned                1
shipped-kind     module-instance          0
shipped-kind     class                    1
shipped-kind     metaclass                2
shipped-kind     level                    3
shipped-kind     level-metaclass          4
access           public                   0
access           protected                1
access           private                  2
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
one; and, in an image with source, its text. The rows form a forest, one
tree per module, the module's root declaration its root and the children
of a row its nested declarations and then its class side; each tree is
written in preorder, a row before its children, and the trees in module
order.

The methods of a row are in the order of their selectors' text. Every
accessor of a slot follows from its name, kind and access and is not
listed, except a lazy slot's getter, which is one of the row's methods.

### The module section

r[image.modules]
The module section holds one record per module, in module order, the
application's module first: its origin and path -- a path relative to
the project for a module of the program, the file's name for the
binary's own manifest -- its id, its version when it declares one, and
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
Each body tag is followed by the fields its row gives: an instance's,
array's, weak array's and ephemeron's values, counted; a string's text; a
symbol's symbol; a closure's body, the record of the frame it captured
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
mixin-table                                count:u32  row:mixin-row*count
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
object-body/object-body/class              record:class-record
object-body/object-body/metaclass          class:class-reference
object-body/object-body/byte-array         bytes:bytes
object-body/object-body/weak-array         count:u32  element:value*count
object-body/object-body/ephemeron          count:u32  field:value*count
object-body/object-body/mixin              mixin:reified-mixin  side:mixin-side
object-body/object-body/frame              frame:frame
object-body/object-body/body-less-class    record:class-record  has_enclosing:u8  enclosing:value?has_enclosing  has_declaring_class:u8  declaring_class:class-reference?has_declaring_class
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
