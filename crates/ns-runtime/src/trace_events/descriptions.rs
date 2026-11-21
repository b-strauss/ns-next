//! A body described for a trace file (`docs/design/developer-tool.md`,
//! decision 18): its module, its source range and a checksum of that
//! source, and where in the source each of its statements and sends
//! stands, from the debugger's metadata (`debugger/metadata/`). Only the
//! runtime knows which source a bytecode index came from, so the host
//! places every event, listing and position through the description.
//!
//! Made at the flush, from the body's code and its mixin -- never a
//! handle -- and only when the host asked for descriptions
//! (`TraceSettings::describe_bodies`). Every span of a description
//! counts in the text the body's version was read in -- the module's text
//! as loaded, or as the install that made the version left it -- which
//! the runtime does not keep whole: an edit installed before or during the
//! run changes the module's text around the bodies it leaves alone. So a
//! description carries the body's own source's checksum, by which the host
//! tells whether a text it holds -- the file it loaded the module from --
//! holds the body at its span, and the module's current text where that
//! one does.

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
#[derive(Clone, PartialEq, Eq)]
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
    /// The module's current text, shared with every other description
    /// made from it, where it holds the body's own source at `source`;
    /// `None` where an edit changed the text around the body or replaced
    /// it, and the host places the body only in a text of its own that
    /// holds it there.
    pub text: Option<Rc<str>>,
    /// The body's source, in bytes of the text its version was read in.
    pub source: Span,
    /// `source_checksum` of the body's own source: what tells a host
    /// whether a text it holds has the body at `source`, as it was
    /// compiled.
    pub checksum: u64,
    /// Every statement start of the body, in emission order, each span in
    /// bytes of the text `source` counts in; empty where the body compiled
    /// again does not compile alike, has no text, or its source in the
    /// current text is not its own.
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

/// A description holds a whole module's text: its debug form gives the
/// text's length, never the text.
impl std::fmt::Debug for BodyDescription {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BodyDescription")
            .field("body", &self.body)
            .field("family", &self.family)
            .field("module", &self.module)
            .field("shipped_path", &self.shipped_path)
            .field("text_length", &self.text.as_ref().map(|text| text.len()))
            .field("source", &self.source)
            .field("checksum", &format_args!("{:016x}", self.checksum))
            .field("statements", &self.statements)
            .field("sends", &self.sends)
            .finish()
    }
}

/// The description of `code`, which `origin` names and the trace numbered
/// `body`, a block of `family`'s where it is given; `None` for a body of a
/// mixin no heap registered, or whose source no text holds.
///
/// A body is described by its own source -- its version's text sliced by
/// its own span, which counts in the text the version was read in -- and
/// handed the module's current text only where that text holds the same
/// source at the same span. Its statements and sends come from its
/// metadata, compiled again from the current text, and are moved from
/// where the body compiled again stands there to the body's own span,
/// where the current text holds the body's own source at the former;
/// otherwise they are left out, as they would be counted in a text the
/// body was not compiled from. A version without a text of its own -- a
/// builtin's, an image's without source -- is described as its module's
/// current text holds its span, as nothing else says where it stands.
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
    let current = match &metadata {
        Some((_, text)) => Some(Rc::clone(text)),
        None if root.shipped => Some(shipped_module_text(root.name.as_str())),
        None => mutator.heap.known_mixin_id(&root).and_then(|id| {
            let text = mutator.heap.module_source(id)?;
            Some(mutator.debug_metadata.module_text(id, text))
        }),
    };
    let source = code.source;
    let own: String = match origin.mixin.text.as_ref() {
        Some(text) => text.slice(source)?.to_string(),
        None => current.as_deref()?.get(range_of(source))?.to_string(),
    };
    let text = current.filter(|text| text.get(range_of(source)) == Some(own.as_str()));
    let (statements, sends) = metadata
        .and_then(|(metadata, current)| {
            // The body compiled again stands at its own source in the
            // current text, which an edit above it moved.
            let again = metadata.source;
            if current.get(range_of(again)) != Some(own.as_str()) {
                return None;
            }
            let moved = |span: Span| moved(span, again, source);
            let statements = metadata
                .statements
                .iter()
                .map(|statement| {
                    Some(DescribedStatement {
                        instructions: statement.instructions.clone(),
                        span: moved(statement.span)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let sends = metadata
                .sends
                .iter()
                .map(|send| {
                    Some(DescribedSend {
                        index: send.index,
                        site: send
                            .call_site
                            .filter(|site| *site != CallSite::UNCACHED)
                            .map(|site| site.number),
                        call: moved(send.call)?,
                        selector: send
                            .selector
                            .iter()
                            .map(|token| moved(*token))
                            .collect::<Option<Vec<_>>>()?,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some((statements, sends))
        })
        .unwrap_or_default();
    Some(BodyDescription {
        body,
        family,
        module,
        shipped_path,
        text,
        source,
        checksum: source_checksum(own.as_bytes()),
        statements,
        sends,
    })
}

/// `span`, a span of the text `again` counts in, moved by as much as
/// `own` lies from `again`: the same source in the text `own` counts in.
/// Only a span within `again` moves, since only its bytes are known alike
/// in both texts; `None` for any other where the two differ.
fn moved(span: Span, again: Span, own: Span) -> Option<Span> {
    if again.start == own.start {
        return Some(span);
    }
    if span.start < again.start || span.end > again.end {
        return None;
    }
    Some(Span::new(
        span.start - again.start + own.start,
        span.end - again.start + own.start,
    ))
}

/// `span` as a range of byte offsets.
fn range_of(span: Span) -> Range<usize> {
    span.start as usize..span.end as usize
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

    /// A description's debug form gives its text's length, never the
    /// text, which is a whole module's.
    #[test]
    fn the_debug_form_leaves_the_text_out() {
        let description = BodyDescription {
            body: BodyName {
                number: 1,
                name: "Demo>>run".to_string(),
            },
            family: None,
            module: ModuleName::named("Demo"),
            shipped_path: None,
            text: Some(Rc::from("class Demo = Object new () ( run = ( ) )")),
            source: Span::new(29, 37),
            checksum: 1,
            statements: Vec::new(),
            sends: Vec::new(),
        };
        let debugged = format!("{description:?}");
        assert!(!debugged.contains("class Demo"), "{debugged}");
        assert!(debugged.contains("text_length: Some(40)"), "{debugged}");
        assert!(debugged.contains("Demo>>run"), "{debugged}");
    }

    /// The spans of a body compiled again move with its source to where
    /// the body's own stands; a span outside it moves only where the two
    /// stand alike.
    #[test]
    fn spans_move_with_the_source() {
        let again = Span::new(40, 60);
        let own = Span::new(10, 30);
        assert_eq!(
            moved(Span::new(45, 50), again, own),
            Some(Span::new(15, 20))
        );
        assert_eq!(moved(Span::new(5, 8), again, own), None);
        assert_eq!(moved(Span::new(5, 8), own, own), Some(Span::new(5, 8)));
    }

    /// A line end hashes alike whether it is `\n` or `\r\n`, as an editor shows both.
    #[test]
    fn the_checksum_leaves_out_carriage_returns() {
        assert_eq!(source_checksum(b"a\r\nb"), source_checksum(b"a\nb"));
        assert_eq!(source_checksum(b"\r"), source_checksum(b""));
        assert_ne!(source_checksum(b"a\nb"), source_checksum(b"ab"));
    }
}
