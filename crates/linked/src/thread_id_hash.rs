use std::hash::{BuildHasher, Hasher};

/// A hasher implementation specialized for thread IDs.
pub(crate) struct ThreadIdHasher {
    state: u64,
}

impl ThreadIdHasher {
    pub(crate) fn new() -> Self {
        ThreadIdHasher { state: 0 }
    }
}

impl Hasher for ThreadIdHasher {
    fn finish(&self) -> u64 {
        self.state
    }

    // No mutation - we avoid hardcoding hash logic into tests, so expectations are minimal.
    #[cfg_attr(test, mutants::skip)]
    fn write(&mut self, bytes: &[u8]) {
        // We expect this to only be called once per hash operation.
        // We expect the contents to be a u64 that typically has only
        // the low bits set (rare to see more than 16 bits of data, often even 8 bits).

        self.state = u64::from_le_bytes(bytes.try_into().expect("expecting ThreadId to be u64"));

        // We copy the low byte into the high byte because HashMap seems to care a lot about
        // the high bits (this is used as the control byte for fast comparisons).
        self.state ^= u64::from(bytes[0]) << 56;
    }
}

/// A `BuildHasher` that creates `ThreadIdHasher` instances.
pub(crate) struct BuildThreadIdHasher;

impl BuildHasher for BuildThreadIdHasher {
    type Hasher = ThreadIdHasher;

    fn build_hasher(&self) -> Self::Hasher {
        ThreadIdHasher::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_byte_is_different() {
        // Even for tiny changes in the ID value, we expect the control byte (high byte) to be
        // different because the control byte comparison is performance-critical.
        let mut hasher = ThreadIdHasher::new();
        hasher.write(&0u64.to_le_bytes());
        let hash1 = hasher.finish();

        let mut hasher = ThreadIdHasher::new();
        hasher.write(&1u64.to_le_bytes());
        let hash2 = hasher.finish();

        // There has to be at least some difference.
        assert_ne!(hash1, hash2);

        // This is the control byte (high byte).
        assert_ne!(hash1 & 0xFF00_0000_0000_0000, hash2 & 0xFF00_0000_0000_0000);
    }
}
