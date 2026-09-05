//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! "Image format" section. Layout: header, source text, `SourceOrigin`,
//! code table, object table, frame table, roots.

pub mod decode;
pub mod encode;
