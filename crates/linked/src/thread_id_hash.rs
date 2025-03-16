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

    fn write(&mut self, bytes: &[u8]) {
        // We expect this to only be called once per hash operation.
        // We expect the contents to be a u64 that typically has only
        // the low bits set (rare to see more than 16 bits of data, often even 8 bits).

        self.state = u64::from_le_bytes(bytes.try_into().expect("expecting ThreadId to be u64"));

        // We copy the low byte into the high byte because HashMap seems to care a lot about
        // the high bits (this is used as the control byte for fast comparisons).
        self.state ^= (bytes[0] as u64) << 56;
    }
}

/// A BuildHasher that creates ThreadIdHasher instances.
pub(crate) struct BuildThreadIdHasher;

impl BuildHasher for BuildThreadIdHasher {
    type Hasher = ThreadIdHasher;

    fn build_hasher(&self) -> Self::Hasher {
        ThreadIdHasher::new()
    }
}
