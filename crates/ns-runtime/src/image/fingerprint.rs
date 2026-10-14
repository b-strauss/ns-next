//! The shipped world's fingerprint, which an image's header carries
//! (`docs/design/image-format.md`, "Shipped names"):
//! an image names the shipped world by declaration, so it is loaded only
//! by a binary whose shipped world is the one that wrote it.

use std::sync::OnceLock;

use crate::builtins::primitive::MIRROR;
use crate::types::BuiltinClass;

/// FNV-1a over the core library's fingerprint, which `build.rs` takes over
/// its sources, the builtin classes' names and the mirror's selectors, each
/// preceded by its length so that no two sequences of parts hash alike by
/// running together. The lowering is not in it: `load` always compiles the
/// shipped code with the optimized lowering, and an application carries its
/// own code.
pub fn shipped_world_fingerprint() -> u64 {
    static FINGERPRINT: OnceLock<u64> = OnceLock::new();
    *FINGERPRINT.get_or_init(|| {
        let mut hasher = Fnv::new();
        hasher.part(&crate::shipped_code::CORE_LIBRARY_FINGERPRINT.to_le_bytes());
        for kind in BuiltinClass::ALL {
            hasher.part(format!("{kind:?}").as_bytes());
        }
        for entry in MIRROR {
            hasher.part(entry.selector.as_bytes());
            hasher.part(entry.reported.as_bytes());
        }
        hasher.finish()
    })
}

struct Fnv {
    state: u64,
}

impl Fnv {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Fnv {
            state: Self::OFFSET_BASIS,
        }
    }

    fn part(&mut self, bytes: &[u8]) {
        self.bytes(&(bytes.len() as u64).to_le_bytes());
        self.bytes(bytes);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.state
    }
}
