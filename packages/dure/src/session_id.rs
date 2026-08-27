//! Positive integer session identity.

use std::num::NonZero;

/// Positive integer that identifies a live session for this user.
///
/// Unique among live sessions and reusable after that session ends
/// (design.md, "Session identity").
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(NonZero<u32>);

impl SessionId {
    /// Smallest legal session id.
    pub const MIN: Self = Self(NonZero::<u32>::MIN);

    /// Wraps a positive id.
    #[inline]
    #[must_use]
    pub const fn new(id: NonZero<u32>) -> Self {
        Self(id)
    }

    /// Converts a raw integer, rejecting zero.
    #[inline]
    #[must_use]
    pub fn from_u32(id: u32) -> Option<Self> {
        NonZero::new(id).map(Self)
    }

    /// Inner positive integer.
    #[inline]
    #[must_use]
    // A mutation that returns a constant collides every `allocate_id`
    // reservation and hangs the exclusive-create loop under cargo-mutants.
    #[cfg_attr(test, mutants::skip)]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero() {
        assert!(SessionId::from_u32(0).is_none());
    }

    #[test]
    fn accepts_positive() {
        let id = SessionId::from_u32(1).unwrap();
        assert_eq!(id.get(), 1);
        assert_eq!(id, SessionId::MIN);
    }
}
