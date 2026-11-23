# Grammar

How the tokens of the lexical structure form a module. The grammar is
written over tokens: a quoted `"x"` is the token spelled `x`, a name in
italics-free lowercase is a production, and `identifier`, `keyword`,
`setterKeyword`, `binarySelector`, `number`, `string` and `symbol` are the
tokens of [Lexical structure](lexical-structure.md).

## How the grammar is read

r[syntax.peg]
The grammar is a parsing expression grammar over tokens. An optional part
`[x]` is taken whenever it matches and is never retried as absent; a
repetition `x*` or `x+` takes as many as match; of alternatives `a | b`
the first that matches in full wins. Where a production could be read two
ways, the production written first is tried in full and the other only
when it fails. A module whose tokens the production `compilationUnit`
does not derive, followed by the end of input, is refused.

r[syntax.whitespace-free]
Whitespace and comments never appear in a production: they only separate
tokens ([`lex.whitespace`](lexical-structure.md#r-lex.whitespace)). A
production that Bracha writes with `whitespace` after a word --
`accessModifier`, `lazyModifier` -- is the word alone, so `| public= 1. |`
and `| public = 1. |` are the same tokens and are refused alike (Bracha
§6.3.3 and §6.3.4; see the appendix).

r[syntax.reserved-word]
A reserved word ([`lex.reserved-words`](lexical-structure.md#r-lex.reserved-words))
may not name a parameter, a block parameter, a slot or a temporary, a
class or a method, may not be the target of a setter send (`nil:: 3`),
and may not stand as a unary selector (`3 nil`, `3 printString; self`).
Each such use is refused naming what the word may not be:
``  `nil` is a reserved word and cannot name a parameter (§4.1) ``. A
keyword that merely begins with a reserved word, `ifNil:` or `selfish:`,
is no reserved word.

r[syntax.unreserved-word]
`class`, `public`, `protected`, `private` and `lazy` are names wherever
the grammar takes an identifier -- a name read, written or sent, a
parameter, a block parameter, a class, a superclass clause's receiver,
`outer`'s class name, the language id. Where such a word could also begin
a declaration, [`syntax.peg`](#r-syntax.peg) decides: an access modifier's
word standing first in a slot or a method is the modifier, so it names a
slot, a temporary or a method only behind one -- `| public public = 1. |`
and `public private = ( ^1 )` are accepted, `| public |` and
`public = ( ^1 )` are refused.

r[syntax.nesting-limit]
Primaries, class declarations, chains of setter keywords and pattern
literals nest at most 128 deep. A source nesting one of them deeper is
refused, `expression nested too deeply`, `class nesting too deep` or
`pattern nested too deeply`. A chain of messages, `a b c + d …`, is not a
nesting in this sense: every length of it is accepted.

## Modules

r[syntax.compilation-unit]
A module is one compilation unit: an optional language id, an optional
class category, and one class declaration, the module's class. Nothing
may follow it but whitespace and comments:
`unexpected tokens after the top-level class`.

```text
compilationUnit → [languageId] [classCategory] classDeclaration
languageId → identifier
classCategory → string
```

r[syntax.language-id]
The language id and the class category are both optional, and neither
has a meaning: a module with them and the same module without them are
the same program. Bracha §8.1 requires the language id; see the appendix.
Because `class` and the access modifiers are names, a leading `class`,
`public`, `protected` or `private` is the language id exactly when a class
category or a class declaration follows it, and begins the class
declaration otherwise: `class class Root = () ()` has the language id
`class` and declares `Root`, `class class = () ()` declares the class
named `class`.

r[syntax.module-class-access]
The module's class takes no access modifier; only a nested class does:
`a module's class takes no access modifier; only a nested class does (§8.1)`.

## Class declarations

r[syntax.class-declaration]
A class declaration is `class`, the class's name, optional type
parameters, the primary factory's message pattern -- or none --, `=`, and
then either a class body, or a superclass clause followed by a class body
or by mixin applications. The alternative with a message pattern is tried
first, so `class Pair = other = Object new () ()` has the binary factory
`=`. A declaration without a message pattern has the factory `new`.

```text
classDeclaration → "class" identifier [typeParameters] [messagePattern] "="
                   (classBody | inheritanceClause (classBody | mixinApplications))
```

r[syntax.inheritance-clause]
A superclass clause, Bracha's inheritance clause, is an optional prefix --
`outer` and a class name, `self` or `super` -- an identifier naming the
superclass, and at most one message to it: a unary selector, a binary
message or a keyword message, whose arguments are binary or unary
expressions. A second message is refused, `a superclass clause sends at
most one message to the class it names (§6.1)`: `Object new foo`.

```text
inheritanceClause → [inheritancePrefix] identifier [typeArguments] [message]
inheritancePrefix → "outer" identifier | "self" | "super"
```

r[syntax.mixin-application]
After an explicit superclass clause, one or more mixin applications may
follow, each `<:` and an inheritance clause, ended by a class body or, for
a declaration that adds no members of its own, by `.`:
`class C = S new <: M1 new <: M2 new ( … ) ( … )`, or
`class C = S new <: M new.` The clauses keep their written order.

```text
mixinApplications → ("<:" inheritanceClause)+ ("." | classBody)
```

r[syntax.class-body]
A class body is a header in parentheses, the instance side in
parentheses, and optionally `:` and the class side in parentheses. The
instance side is required, however empty: `class C = () ()`. A header
without the instance side after it is refused, ``expected the instance side's `( ... )` after the class
header (§6.3)``.

```text
classBody → "(" classHeader ")" "(" instanceSide ")" [":" "(" classSide ")"]
```

r[syntax.class-header]
A class header is an optional slot section, sequential or simultaneous,
and then the initializer expressions, separated by single dots, a dot
after the last one allowed. Bracha's `classComment` is a comment and so,
like every comment, no production of the grammar.

```text
classHeader → [slotSection] [expression ("." expression)* ["."]]
```

r[syntax.instance-side]
The instance side is any number of members, each a nested class, a lazy
slot or a method, tried in that order, with nothing between them: a dot
between two members is refused, ``no `.` stands between a class's
members (§6.3)``.

```text
instanceSide → (nestedClass | lazySlot | method)*
nestedClass → [accessModifier] classDeclaration
```

r[syntax.class-side]
The class side is any number of methods, with nothing between them: a dot
between two is refused, ``no `.` stands between a class side's methods
(§6.3)``. A class side declares no slots, lazy slots or nested classes.

```text
classSide → method*
```

r[syntax.access-modifier]
An access modifier is one of `public`, `protected` and `private`. It may
precede a nested class, a slot, a lazy slot and a method.

## Slots

r[syntax.slot-section]
A slot section is a sequential one, between `|` and `|`, or a
simultaneous one, between `||` and `||`. The simultaneous form is tried
first, and an empty sequential section `||` is read when no simultaneous
one matches, so `[:x | || x + 1]` has an empty temporary section (Bracha
§6.3.2 writes the sequential form first, which over characters would make
the simultaneous one unreachable; Bracha's canonical grammar tries the
simultaneous form first, and so does this one).

```text
slotSection → "||" slotDefinition* "||" | "|" slotDefinition* "|"
```

r[syntax.slot-definition]
A slot definition is an optional access modifier, the slot's name, and
then either `=` and an expression -- an immutable slot --, `::=` and an
expression -- a mutable one --, or nothing -- an uninitialized mutable
slot. The dot after an initializer is required, ``expected `.` after a
slot's initializer (§6.3.2)``; after an uninitialized slot it is optional.

```text
slotDefinition → [accessModifier] slotName [("=" | "::=") expression "." | "."]
slotName → identifier [typeAnnotation]
```

r[syntax.lazy-slot]
A lazy slot is `lazy` and an access modifier in either order, or `lazy`
alone, then a slot's name, `=` or `::=`, an expression and a required dot.
It is a member of the instance side, not of the header. A member is a lazy
slot only if all of that matches: `lazy lazy = 1.` is a lazy slot named
`lazy`, `lazy public public = 1.` a public one named `public`, while
`lazy = ( ^1 )` and `private lazy = ( ^1 )` are methods named `lazy`, and
`lazy public = 1.` is refused.

```text
lazySlot → ("lazy" [accessModifier] | accessModifier "lazy") slotName ("=" | "::=") expression "."
```

## Methods

r[syntax.method]
A method is an optional access modifier, a message pattern, `=`, and the
method's code body in parentheses. A method whose body is not closed is
refused, ``expected `)` to end the method body``.

```text
method → [accessModifier] messagePattern "=" "(" codeBody ")"
```

r[syntax.message-pattern]
A message pattern is a unary selector; or a binary selector and one
parameter -- any binary selector, `||`, `=`, `<` and `->` included; or one or more keywords, each with a parameter. A setter keyword
never begins a message pattern. A parameter is an identifier with an
optional type annotation.

```text
messagePattern → identifier [returnType]
               | binarySelector parameter [returnType]
               | (keyword parameter)+ [returnType]
parameter → identifier [typeAnnotation]
```

## Type annotations

r[syntax.type-annotation]
A type annotation is accepted where Bracha's typed Newspeak writes one and
has no meaning: a program with its annotations and the same program
without them are the same program. The forms are `<Type>` after a
parameter or a slot's name, `^<Type>` after a message pattern, type
parameters `[T]` after a class's name or a selector, type arguments
`[E]` after a superclass or a mixin clause's receiver and at a value
(`Box[Integer] new`, `a foo[E]`, `(Box)[Integer]`), and a `where { … }`
constraint clause. Inside a `<…>` annotation and a `where { … }` clause,
`<:` is the bound operator. An annotation is kept as the tokens written; an
unclosed one is refused, ``unterminated `<...>` type annotation``,
``unterminated `[...]` type arguments``, ``unterminated `where { ... }`
clause``.

r[syntax.type-parameters]
A class's type parameters are written either as Bracha §6 writes them,
`class Map <[K, V]> new: capacity <Integer>`, or as Bracha's canonical
grammar does, `class List[E]`. Both are type annotations
([`syntax.type-annotation`](#r-syntax.type-annotation)).

## Metadata

r[syntax.metadata]
A metadata comment ([`lex.comment.metadata`](lexical-structure.md#r-lex.comment.metadata))
belongs to what it stands next to, nothing but whitespace and comments
between them (Bracha §4.3): one immediately before an expression belongs
to that expression, one immediately before a slot or a parameter to that
slot or parameter, one immediately after a method's or a factory's message
pattern to that method or factory, and one immediately after a class's
header to that class. One immediately before a module's class declaration
belongs to the module's class. Several metadata comments may belong to
one target. A metadata comment that stands next to none of these belongs
to nothing.
