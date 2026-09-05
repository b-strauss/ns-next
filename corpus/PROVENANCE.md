# Corpus Provenance

Files under `corpus/modules/` and `corpus/reference/` are copied verbatim from:

- Repository: https://github.com/newspeaklanguage/newspeak
- Commit: a76295549defe53047a65d464b63a9796d823a69
- Retrieved: 2026-09-01

`corpus/modules/` is a flattened copy of every `*.ns` file in that tree, used
only as a parser test corpus. `corpus/reference/NewspeakGrammar.ns` is the
grammar this parser follows.

## Refresh procedure

1. Clone upstream at the new commit.
2. Replace `corpus/modules/*.ns` and `corpus/reference/NewspeakGrammar.ns`.
3. Update the commit hash and date above.
4. Run `cargo test -p ns-syntax --test corpus` and reconcile `corpus/blacklist.toml`.

## Renamed files

none

## `newspeak-spec.pdf`

Not part of the `newspeaklanguage/newspeak` repo above — a separate
provenance. `corpus/reference/newspeak-spec.pdf` is Gilad Bracha's "Newspeak
Programming Language Draft Specification", v0.106, retrieved 2026-09-04 from
https://newspeaklanguage.org/spec/newspeak-spec.pdf. It is the authoritative
source for language *semantics* (name resolution, dispatch, access control,
instantiation), where `NewspeakGrammar.ns` above only covers *syntax*. It is
explicitly a draft and may itself disagree with the reference implementation
in places (noted in its own front matter) — where it and the vendored corpus's
actual behavior disagree, prefer the corpus and note the discrepancy.

Refresh procedure: re-download from the URL above, update the version and
retrieved date here, and re-check `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md`'s
section citations still point at the same content (section numbers could
shift between spec versions).
