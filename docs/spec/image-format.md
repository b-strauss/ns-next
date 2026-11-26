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
the thirteen spaces the image's sections use -- the header's flags,
values, object bodies, mixin references, mixin sides, class references,
frame kinds, frame bodies, tag-named hashes, slot kinds, module origins,
module versions and the kinds of a shipped name -- are those of the
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
```
