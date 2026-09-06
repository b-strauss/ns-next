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

## Language specification

The Newspeak language specification is not part of this corpus: it has its own
source and its own release cadence, independent of the repository above. It is at
`docs/references/newspeak-spec.pdf`, with its provenance, its draft caveat and its
refresh procedure in `docs/references/PROVENANCE.md`.
