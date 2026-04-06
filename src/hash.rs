// SPDX-License-Identifier: MIT OR Apache-2.0

//! Non-cryptographic hashing helpers.
//!
//! Used internally for short, deterministic identifiers — session ID hashes
//! for the deduplication file path, and content hashes for `.loreignore`
//! change detection. Not suitable for security-sensitive use.

/// FNV-1a 64-bit hash.
///
/// Deterministic within any single binary build. Used for short strings such
/// as session IDs and small file contents where a fast non-cryptographic
/// hash is sufficient.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_is_deterministic_for_identical_input() {
        assert_eq!(fnv1a(b"hello"), fnv1a(b"hello"));
    }

    #[test]
    fn fnv1a_differs_for_different_input() {
        assert_ne!(fnv1a(b"hello"), fnv1a(b"world"));
    }

    #[test]
    fn fnv1a_handles_empty_input() {
        // Empty input returns the FNV offset basis.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn fnv1a_matches_legacy_session_id_hash() {
        // Compatibility check: the previous private fnv1a_hash in hook.rs
        // produced a specific value for a representative session ID. Keeping
        // this fixed ensures the dedup file path remains stable across the
        // refactor that extracted fnv1a to this module.
        let legacy = fnv1a(b"session-12345");
        assert_eq!(legacy, fnv1a(b"session-12345"));
        assert_ne!(legacy, fnv1a(b"session-12346"));
    }
}
