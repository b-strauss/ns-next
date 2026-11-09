//! The processor features the binaries are built to require.
//!
//! Every crate of the workspace is built for x86-64-v2 on an x86_64 target
//! (`.cargo/config.toml`), so a processor without those features would fault
//! on an instruction the compiler chose. Both binaries ask first thing in
//! `main` and refuse with a message instead.

/// The names of the x86-64-v2 features this processor lacks: SSE3, SSSE3,
/// SSE4.1, SSE4.2, POPCNT and CMPXCHG16B. Empty on a processor that has them
/// all, and on every architecture but x86_64, which the flag does not touch.
pub fn missing_processor_features() -> Vec<&'static str> {
    #[cfg(target_arch = "x86_64")]
    {
        let mut missing = Vec::new();
        if !std::arch::is_x86_feature_detected!("sse3") {
            missing.push("sse3");
        }
        if !std::arch::is_x86_feature_detected!("ssse3") {
            missing.push("ssse3");
        }
        if !std::arch::is_x86_feature_detected!("sse4.1") {
            missing.push("sse4.1");
        }
        if !std::arch::is_x86_feature_detected!("sse4.2") {
            missing.push("sse4.2");
        }
        if !std::arch::is_x86_feature_detected!("popcnt") {
            missing.push("popcnt");
        }
        if !std::arch::is_x86_feature_detected!("cmpxchg16b") {
            missing.push("cmpxchg16b");
        }
        missing
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_machine_has_every_feature_the_binary_requires() {
        assert!(missing_processor_features().is_empty());
    }
}
