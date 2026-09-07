//! The input fingerprint: a stable hash of everything that defines what a run
//! *executes*, used by `proef flaky` as the equivalence class for its history
//! window (0.18 survey §6).
//!
//! A pack, feature, config, or fragment edit changes what a scenario *is*, so
//! runs of materially different inputs must not share a flakiness window. The
//! fingerprint makes "same inputs" precise — the Develocity model.
//!
//! **This is a proef-*computed* fact about proef's own inputs, not a
//! *harvested* environment fact.** ADR-0020 forbids proef from reading git
//! state, the hostname, or CI variables; this reads none of them. It is the
//! same category as the artifact slug or the shard hash — a derived identifier
//! over inputs proef already holds. Git-commit grouping stays *handed over*
//! via `--meta commit=…` and `proef flaky --by commit`, never harvested.
//!
//! The hash is a hand-rolled 128-bit value (two independent FNV-1a passes), not
//! a cryptographic digest, deliberately: a collision merely merges two windows
//! in an advisory report — no security boundary — and this avoids a
//! crypto-hash dependency in the sans-IO core. Parts are length-delimited so
//! `["ab","c"]` and `["a","bc"]` cannot collide by concatenation.

/// FNV-1a over `bytes` with the given 64-bit offset basis; the multiplier is
/// the standard FNV prime. Two different bases give two near-independent
/// hashes, combined into the 128-bit fingerprint.
fn fnv1a_with(bytes: &[u8], basis: u64) -> u64 {
    let mut hash = basis;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The 128-bit input fingerprint over an ordered sequence of parts, rendered
/// as 32 lowercase hex characters. The caller supplies the parts in a
/// deterministic order (sorted paths, a `BTreeMap`'s natural order); each part
/// is length-prefixed so the boundaries between parts are part of the hash.
#[must_use]
pub fn of<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    // Two standard FNV-1a offset bases (the canonical one, and a second odd
    // constant) run over the same length-delimited byte stream.
    let mut buf: Vec<u8> = Vec::new();
    for part in parts {
        buf.extend_from_slice(&(part.len() as u64).to_le_bytes());
        buf.extend_from_slice(part.as_bytes());
    }
    let h1 = fnv1a_with(&buf, 0xcbf2_9ce4_8422_2325);
    let h2 = fnv1a_with(&buf, 0x9e37_79b9_7f4a_7c15);
    format!("{h1:016x}{h2:016x}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_over_identical_inputs() {
        let a = super::of(["feature\nsource", "pack\nsource", "url:base=x"].into_iter());
        let b = super::of(["feature\nsource", "pack\nsource", "url:base=x"].into_iter());
        assert_eq!(a, b, "identical inputs must fingerprint identically");
        assert_eq!(a.len(), 32, "128 bits as hex");
    }

    #[test]
    fn changes_when_any_input_changes() {
        let base = super::of(["feature", "pack", "config"].into_iter());
        assert_ne!(base, super::of(["FEATURE", "pack", "config"].into_iter()));
        assert_ne!(base, super::of(["feature", "PACK", "config"].into_iter()));
        assert_ne!(base, super::of(["feature", "pack", "CONFIG"].into_iter()));
    }

    #[test]
    fn part_boundaries_are_hashed() {
        // Without length-delimiting these two would collide.
        assert_ne!(
            super::of(["ab", "c"].into_iter()),
            super::of(["a", "bc"].into_iter())
        );
    }
}
