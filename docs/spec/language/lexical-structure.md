# Lexical structure

A module's source is read as a sequence of *tokens*. This chapter says
which characters form which token, what lies between tokens, and which
character sequences are refused. The grammar (the next chapter) is written
over these tokens, never over characters, so spacing has no meaning beyond
separating tokens.

The lexical rules are written as a grammar over characters: `"x"` is the
character `x`, `"a" - "z"` a range, `|` an alternative, `*`, `+` and `?`
repetition and option, and `!x` a position where `x` does not follow. As
everywhere in this book, an optional or repeated part is taken whenever it
matches, and of alternatives the first that matches wins.

## Source text

r[lex.source]
A module's source is Unicode text, encoded as UTF-8. Every token is
written in ASCII characters; a character outside ASCII may stand only
inside a comment or a string literal. Any other character outside both
is refused, one diagnostic per character, `unexpected character 'é'`.

r[lex.whitespace]
Whitespace is a space, a horizontal tab, a line feed, a form feed or a
carriage return. Whitespace and comments separate tokens and have no other
meaning: two sources that differ only in the whitespace and comments
between their tokens are the same sequence of tokens.

r[lex.longest-token]
Tokens are read from the start of the source to its end, each the longest
token that begins at its position, except where a rule below says
otherwise. The sequence ends with an end-of-input token.

## Comments

r[lex.comment.block]
A block comment begins with `(*` and ends with the matching `*)`. Block
comments nest: every `(*` inside one opens another that must close before
the outer one does. Inside a block comment no other token is recognized,
`///` and quotes included. A block comment that reaches the end of the
source unclosed is refused, `unterminated comment`.

```text
blockComment → "(*" (blockComment | !"*)" any)* "*)"
```

r[lex.comment.line]
A line comment begins with `///` wherever it stands outside a string
literal and a block comment, and runs to the next line feed or carriage
return, or to the end of the source. Inside a line comment `(*` opens
nothing. Since a line comment begins wherever `///` stands, no binary
selector contains `///` ([`lex.binary-selector`](#r-lex.binary-selector)).
Bracha's Newspeak has no line comment; see the appendix.

```text
lineComment → "///" (!lineBreak any)*
lineBreak → "\n" | "\r"
```

r[lex.comment.metadata]
A block comment is a *metadata comment* when its text after `(*`, past any
whitespace, begins with `:`, a *tag* and a second `:`. The tag is one or
more characters, none of them whitespace or `:`; the *payload* is the
rest of the comment's text before `*)`, without the whitespace at either
end. Lexically a metadata comment is a comment like any other. Where it
stands decides what it belongs to
([`syntax.metadata`](grammar.md#r-syntax.metadata)). Bracha §4.3 calls the
tag an identifier, yet writes tags such as `baz-metadata` in its own
examples; this book takes the examples.

```text
metadataComment → "(*" whitespace* ":" tag ":" payload "*)"
tag → (!(whitespace | ":") any)+
```

## Names

r[lex.identifier]
An identifier is a letter or an underscore followed by letters, digits
and underscores, where a letter is one of `a` to `z` and `A` to `Z`.

```text
identifier → (letter | "_") (letter | digit | "_")*
letter → "a" - "z" | "A" - "Z"
digit → "0" - "9"
```

r[lex.keyword]
A keyword is an identifier followed directly by one `:` that no second
`:` and no `=` follows: `at:` in `at: 1 put: 2`. A keyword names a part of
a keyword message.

r[lex.setter-keyword]
A setter keyword is an identifier followed directly by `::` that no `=`
follows: `count::` in `count:: count + 1`. An identifier followed by `::=`
is that identifier and the token `::=`
([`lex.punctuation`](#r-lex.punctuation)).

```text
keyword → identifier ":" !(":" | "=")
setterKeyword → identifier "::" !"="
```

r[lex.reserved-words]
Six words are reserved: `self`, `super`, `outer`, `true`, `false` and
`nil` (Bracha §4.1). A reserved word is lexed as an identifier; the
grammar refuses one wherever it would name a slot, a temporary, a
parameter, a method or a class
([`syntax.reserved-word`](grammar.md#r-syntax.reserved-word)).

```spec-table reserved-words
self super outer true false nil
```

r[lex.unreserved-words]
Every other word is a name, the words that begin a declaration among
them: `class`, `public`, `protected`, `private` and `lazy` are
identifiers wherever the grammar takes an identifier. The first four are
lexed as tokens of their own, so that a declaration can be recognized by
its first token; the grammar accepts each of them wherever it accepts an
identifier. `lazy` is lexed as an identifier.

```spec-table words-with-tokens
class public protected private
```

## Operators and punctuation

r[lex.special-characters]
The special characters are the characters a binary selector is made of.
`-` is not one of them: it may begin a binary selector and nothing more
([`lex.binary-selector`](#r-lex.binary-selector)).

```spec-table special-characters
+ * / \ ~ < > = & | @ % , ? !
```

r[lex.binary-selector]
A binary selector is a special character or `-`, followed by as many
special characters as follow it, stopping before any `///`, which begins a
line comment instead ([`lex.comment.line`](#r-lex.comment.line)). So `+-`
is the selector `+` followed by `-`, `a +/// x` is `a +` followed by a
comment, and `//` is a binary selector. Where a binary selector would be
exactly `=`, `<` or `>`, the token is the punctuation of that name instead;
the grammar accepts each of them as a binary selector where a binary
message stands. A `|` begins a binary selector only when a special
character other than the start of `///` follows it; otherwise it is the
punctuation `|` or `||`.

```text
binarySelector → ("-" | specialCharacter) (!"///" specialCharacter)*
```

r[lex.punctuation]
The punctuation tokens are `(`, `)`, `[`, `]`, `{`, `}`, `^`, `.`, `;`,
`:`, `|`, `||`, `=`, `<`, `>`, `::=`, `<:` -- the mixin application
operator -- and `<-:` -- the asynchronous send operator. `<:` is one token
although `<` is a special character, because `:` is not one, so no binary
selector is spelled `<:`; `<-:` likewise, since `-` cannot continue a
binary selector.

r[lex.assignment-refused]
`:=` is no token: it is refused where it stands, `` `:=` is not Newspeak
syntax; a setter send is written `name:: expression` ``. An identifier
directly followed by `:=` is that identifier, then the refusal.

## Literals

r[lex.number]
A decimal number is an optional `-`, digits, an optional fraction -- `.`
and digits -- and an optional exponent -- `e`, an optional `-`, and
digits. Each optional part is taken only when it is complete: `1.foo` is
the number `1` followed by `.` and `foo`, and `2e` is `2` followed by the
identifier `e`.

```text
decimalNumber → "-"? digit+ ("." digit+)? exponent?
exponent → "e" "-"? digit+
```

r[lex.number.radix]
A radix number is digits, `r`, an optional `-`, extended digits -- digits
and the uppercase letters `A` to `Z` -- an optional fraction of extended
digits, and an optional exponent. The `r` and what follows it are taken
only when an extended digit follows the `r` or its `-`: `16r` is `16`
followed by the identifier `r`. A radix number has no leading `-`:
`-16rFF` is the number `-16` followed by the identifier `rFF`.

```text
radixNumber → digit+ "r" "-"? extendedDigit+ ("." extendedDigit+)? exponent?
extendedDigit → digit | "A" - "Z"
```

r[lex.number.sign]
A `-` directly followed by a digit is read as the sign of a number. Where
the grammar takes a binary message rather than a literal, the parser reads
that number as the binary selector `-` followed by the number without its
sign, so that `3 -4` subtracts four from three while `{3. -4}` holds a
negative number ([`syntax.literal.number`](grammar.md#r-syntax.literal.number)).

r[lex.string.single-quoted]
A single-quoted string literal is `'`, any characters, and `'`, a `'`
inside it written twice: `'it''s'`. It may span lines. One that reaches
the end of the source unclosed is refused, `unterminated string literal`.

```text
singleQuotedString → "'" ("''" | !"'" any)* "'"
```

r[lex.string.double-quoted]
A double-quoted string literal is `"`, any characters but `"`, and `"`:
it has no escape, and ends at the next `"`. One that reaches the end of
the source unclosed is refused, `unterminated string literal`. Each kind
holds the other kind's quote freely: `'"'` and `"'"`.

r[lex.string.one-character]
The one exception to [`lex.string.double-quoted`](#r-lex.string.double-quoted)
is a single character between two `"`: whatever the character is, `"`
included, the three form a one-character string. So `"""` is the string
holding one `"`, and `""""` is that string followed by the start of
another. There are no character literals: `"a"` is a string of one
character (Bracha §5.1.4 announces that it does away with them; see the
appendix).

```text
doubleQuotedString → "\"" any "\"" | "\"" (!"\"" any)* "\""
```

r[lex.symbol]
A symbol literal is `#` followed directly by one of: a single-quoted
string, `#'a symbol'`; an identifier, `#size`; one or more keywords,
`#at:put:`, where an identifier after the last keyword's `:` belongs to
the symbol only when its own `:` follows, so `#at:put` is `#at:` followed
by the identifier `put`; or a binary selector, `#+`. A `#` that none of
these follows is refused, `` expected symbol after `#` ``, a `#` before
`///` among them: `#/// x` is a refused `#` and a comment.

```text
symbol → "#" (singleQuotedString | keyword+ | identifier | binarySelector)
```

r[lex.literal-array-refused]
`#(` is refused where it stands, `` `#( ... )` is not Newspeak syntax; a
tuple is written `{ element. element }` ``: Newspeak has no literal array.
