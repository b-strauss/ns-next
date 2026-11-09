//! The processor features the binaries are built to require.
//!
//! Every crate of the workspace is built for x86-64-v2 on an x86_64 target
//! (`.cargo/config.toml`), so a processor without those features would fault
//! on an instruction the compiler chose. Both binaries ask first thing in
//! `main` and refuse with a message instead.
//!
//! The question goes to the processor itself, through CPUID. The standard
//! library's `is_x86_feature_detected!` would not do: it answers `true`
//! without asking for every feature the crate is compiled with, and this
//! crate is compiled for x86-64-v2.
//!
//! This code is compiled for v2 too, which is acceptable because it is plain
//! integer code (two CPUIDs and bit tests): nothing in it invites the compiler
//! to emit an SSE3 or later instruction before the message is printed. No
//! test pins that: it is a property of the compiler's output, not of this
//! code's behaviour, and a processor that lacks the instruction cannot run a
//! test of the refusal.

/// CPUID leaf 1's ECX bits of the x86-64-v2 features, with their names (Intel
/// SDM Vol. 2, CPUID, "Feature Information Returned in the ECX Register").
/// Only an x86_64 build asks CPUID; elsewhere no feature is required.
#[cfg(target_arch = "x86_64")]
const LEAF_ONE_FEATURES: [(u32, &str); 6] = [
    (1 << 0, "sse3"),
    (1 << 9, "ssse3"),
    (1 << 13, "cmpxchg16b"),
    (1 << 19, "sse4.1"),
    (1 << 20, "sse4.2"),
    (1 << 23, "popcnt"),
];

/// CPUID leaf 0x8000_0001's ECX bit of the one x86-64-v2 feature outside leaf
/// 1: LAHF and SAHF in 64-bit mode (Intel SDM Vol. 2, CPUID, "Extended
/// Function CPUID Information", bit 0). The same safe `__cpuid` reads it.
#[cfg(target_arch = "x86_64")]
const EXTENDED_LEAF_FEATURES: [(u32, &str); 1] = [(1 << 0, "lahf-sahf")];

/// The names of the features in `table` that CPUID's `ecx` does not set.
#[cfg(target_arch = "x86_64")]
fn missing_from(table: &[(u32, &'static str)], ecx: u32) -> Vec<&'static str> {
    table
        .iter()
        .filter(|(bit, _)| ecx & bit == 0)
        .map(|(_, name)| *name)
        .collect()
}

/// The names of the x86-64-v2 features this processor lacks: SSE3, SSSE3,
/// SSE4.1, SSE4.2, POPCNT, CMPXCHG16B and LAHF-SAHF. Empty on a processor
/// that has them all, and on every architecture but x86_64, which the flag
/// does not touch.
pub fn missing_processor_features() -> Vec<&'static str> {
    #[cfg(target_arch = "x86_64")]
    {
        // CPUID exists on every x86_64 processor, and so does leaf
        // 0x8000_0001, the leaf the 64-bit mode itself is reported in.
        let mut missing = missing_from(&LEAF_ONE_FEATURES, std::arch::x86_64::__cpuid(1).ecx);
        missing.extend(missing_from(
            &EXTENDED_LEAF_FEATURES,
            std::arch::x86_64::__cpuid(0x8000_0001).ecx,
        ));
        missing
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Vec::new()
    }
}

/// The message a binary refuses with when `missing` is not empty; `None`
/// when it is. Apart from the CPUID question so that a test can give it a
/// list.
fn refusal_message(program: &str, missing: &[&str]) -> Option<String> {
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "{program} requires an x86-64-v2 processor (SSE4.2, POPCNT); this one lacks: {}",
            missing.join(", ")
        ))
    }
}

/// What a binary prints to stderr and exits with status 1 on when its
/// processor lacks a required feature; `None` when it has them all.
/// `program` is the binary's name.
pub fn refusal_for_missing_features(program: &str) -> Option<String> {
    refusal_message(program, &missing_processor_features())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_arch = "x86_64")]
    const EVERY_BIT: u32 = u32::MAX;

    #[test]
    fn the_build_machine_has_every_feature_the_binary_requires() {
        assert!(missing_processor_features().is_empty());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_leaf_with_every_bit_set_lacks_nothing() {
        assert!(missing_from(&LEAF_ONE_FEATURES, EVERY_BIT).is_empty());
        assert!(missing_from(&EXTENDED_LEAF_FEATURES, EVERY_BIT).is_empty());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn a_leaf_lacking_one_bit_names_exactly_that_feature() {
        for (bit, name) in LEAF_ONE_FEATURES {
            assert_eq!(
                missing_from(&LEAF_ONE_FEATURES, EVERY_BIT & !bit),
                vec![name]
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn an_empty_leaf_lacks_every_feature() {
        assert_eq!(
            missing_from(&LEAF_ONE_FEATURES, 0),
            ["sse3", "ssse3", "cmpxchg16b", "sse4.1", "sse4.2", "popcnt"]
        );
        assert_eq!(missing_from(&EXTENDED_LEAF_FEATURES, 0), ["lahf-sahf"]);
    }

    #[test]
    fn a_processor_with_every_feature_is_not_refused() {
        assert_eq!(refusal_for_missing_features("ns-next"), None);
        assert_eq!(refusal_message("ns-next", &[]), None);
    }

    #[test]
    fn the_refusal_names_the_program_and_every_missing_feature() {
        assert_eq!(
            refusal_message("ns-next", &["sse4.2", "popcnt"]).as_deref(),
            Some(
                [
                    "ns-next requires an x86-64-v2 processor (SSE4.2, POPCNT);",
                    "this one lacks: sse4.2, popcnt",
                ]
                .join(" ")
                .as_str()
            )
        );
    }
}
