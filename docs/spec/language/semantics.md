# Semantics

What a module means when it runs. Not written yet but for its gaps: the
second batch of the specification writes it (`docs/design/specification.md`,
"The order of building it").

## Gaps

r[semantics.gap.actors]
Bracha §5.6 makes an asynchronous send, `receiver <-: message`, answer a
promise at once and deliver the message in a later turn of the receiver's
actor. ns-next builds no actors: an asynchronous send evaluates its
receiver and its arguments, left to right, as an ordinary send does, and
then signals an error, ``asynchronous send of `printString` needs actors,
which ns-next does not build yet (§5.6)``, naming its message's selector.
It fails when it runs, never when its module is compiled.

r[semantics.gap.pattern]
Bracha §5.1.9 makes a pattern literal a send to `Pattern`: `<_>` is
`Pattern wildcard`, `<l>` `Pattern literal: l`. ns-next builds no pattern
matching: a pattern literal that is evaluated signals an error, `a pattern
literal needs pattern matching, which ns-next does not build yet (§5.1.9)`.
It fails when it is evaluated, never when its module is compiled.
