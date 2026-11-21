//! A body described for a trace file (`docs/design/developer-tool.md`,
//! decision 18): its module, its source range and a checksum of that
//! source, and where in the source each of its statements and sends
//! stands, from the debugger's metadata (`debugger/metadata/`). Only the
//! runtime knows which source a bytecode index came from, so the host
//! places every event, listing and position through the description.
//!
//! Made at the flush, from the body's code and its mixin -- never a
//! handle -- and only when the host asked for descriptions
//! (`TraceSettings::describe_bodies`).

use std::ops::Range;
use std::rc::Rc;

use ns_syntax::Span;

use super::BodyName;
use crate::bytecode::{CallSite, CompiledCode};
use crate::debugger::ModuleName;
use crate::debugger::metadata::Cache;
use crate::debugger::metadata::again::{BodyOrigin, declaration_path};
use crate::interpreter::Mutator;
use crate::interpreter::core_library::{shipped_module_text, shipped_source_files};

/// A body, as `TraceSink::body` receives it, once per body number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyDescription {
    /// The body, as every event and listing names it.
    pub body: BodyName,
    /// A block's family root, the body whose code created it at any
    /// depth, whose number the `caches` category's transitions name for
    /// the block's sites; `None` for a root.
    pub family: Option<BodyName>,
    /// The module the body is declared in.
    pub module: ModuleName,
    /// The path of a shipped module's source among the library's files
    /// (`ShippedSourceFile::path`: `Kernel.ns`,
    /// `platform/TimeFactory.ns`); `None` for a program's module, whose
    /// file the host knows.
    pub shipped_path: Option<String>,
    /// The body's source, in bytes of the module's text.
    pub source: Span,
    /// `source_checksum` of the body's source: what tells a host whether
    /// the text it holds is still the text the body was compiled from.
    pub checksum: u64,
    /// Every statement start of the body, in emission order; empty where
    /// the body compiled again does not compile alike, or has no text.
    pub statements: Vec<DescribedStatement>,
    /// Every send of the body, by ascending index; empty where
    /// `statements` is.
    pub sends: Vec<DescribedSend>,
}

/// The instructions one statement compiled to, and the statement's
/// source, in bytes of the module's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescribedStatement {
    pub instructions: Range<u32>,
    pub span: Span,
}

/// One send instruction and the source it was lowered from, in bytes of
/// the module's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescribedSend {
    /// The send instruction's index in its body.
    pub index: u32,
    /// The send's site number within its family, as the `caches`
    /// category's transitions number it; `None` for a send no cache
    /// serves: a mirror send, or one the reference lowering compiled.
    pub site: Option<u32>,
    /// The call, from its receiver to its last argument.
    pub call: Span,
    /// The selector's tokens.
    pub selector: Vec<Span>,
}

/// The FNV-1a 64 hash of `text` (offset basis `0xcbf29ce484222325`,
/// prime `0x100000001b3`) with every `\r` byte left out: a checksum of a
/// body's source that a host on any platform computes alike, not a
/// cryptographic one -- it detects an edit, not an adversary. The `\r`s
/// are left out because an editor holds a file's text with `\n` line
/// ends whatever the file's are, so that a file checked out with CRLF, or
/// with mixed line ends, still matches the text the editor shows.
pub fn source_checksum(text: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    text.iter()
        .filter(|byte| **byte != b'\r')
        .fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
        })
}

/// The description of `code`, which `origin` names and the trace numbered
/// `body`, a block of `family`'s where it is given; `None` for a body of a mixin no heap registered, or whose
/// module has no text that holds its source.
pub(crate) fn describe(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    origin: &BodyOrigin,
    body: BodyName,
    family: Option<BodyName>,
) -> Option<BodyDescription> {
    let (root, _) = declaration_path(mutator, &origin.mixin)?;
    let module = ModuleName::of_root(&root);
    // Decided by the root, never by the module's name, as every shipped
    // decision is (`Mixin::shipped`).
    let shipped_path = root.shipped.then(|| {
        shipped_source_files()
            .into_iter()
            .find(|file| file.name == root.name.as_str())
            .map(|file| file.path.to_string())
    });
    let shipped_path = shipped_path.flatten();
    let metadata = Cache::metadata_with_text(mutator, code, origin);
    let text: Rc<str> = match &metadata {
        Some((_, text)) => Rc::clone(text),
        None if root.shipped => shipped_module_text(root.name.as_str()),
        None => {
            let id = mutator.heap.known_mixin_id(&root)?;
            Rc::from(mutator.heap.module_source(id)?)
        }
    };
    let source = code.source;
    let bytes = text
        .as_bytes()
        .get(source.start as usize..source.end as usize)?;
    let (statements, sends) = match metadata {
        Some((metadata, _)) => (
            metadata
                .statements
                .iter()
                .map(|statement| DescribedStatement {
                    instructions: statement.instructions.clone(),
                    span: statement.span,
                })
                .collect(),
            metadata
                .sends
                .iter()
                .map(|send| DescribedSend {
                    index: send.index,
                    site: send
                        .call_site
                        .filter(|site| *site != CallSite::UNCACHED)
                        .map(|site| site.number),
                    call: send.call,
                    selector: send.selector.to_vec(),
                })
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    Some(BodyDescription {
        body,
        family,
        module,
        shipped_path,
        source,
        checksum: source_checksum(bytes),
        statements,
        sends,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checksum is FNV-1a 64, as its published test vectors give it.
    #[test]
    fn the_checksum_is_fnv_1a_64() {
        assert_eq!(source_checksum(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(source_checksum(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(source_checksum(b"foobar"), 0x8594_4171_f739_67e8);
    }

    /// A line end hashes alike whether it is `\n` or `\r\n`, as an editor shows both.
    #[test]
    fn the_checksum_leaves_out_carriage_returns() {
        assert_eq!(source_checksum(b"a\r\nb"), source_checksum(b"a\nb"));
        assert_eq!(source_checksum(b"\r"), source_checksum(b""));
        assert_ne!(source_checksum(b"a\nb"), source_checksum(b"ab"));
    }
}
