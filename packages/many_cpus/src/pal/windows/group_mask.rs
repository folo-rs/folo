use crate::pal::ProcessorImpl;
use crate::pal::windows::{ProcessorGroupIndex, ProcessorIndexInGroup};

/// Collects processors from the same group into a mask.
#[derive(Debug)]
pub(crate) struct GroupMask {
    // Yes, the mask is a usize, not a u64, even though processor groups are always 64-sized.
    // This is because in the 32-bit Windows API, processor group masking was never really properly
    // implemented so don't use 32-bit Windows if you want things to work right.
    mask: usize,

    // If set, must match - adding processors from different groups to the same mask is nonsense.
    group: Option<ProcessorGroupIndex>,
}

impl GroupMask {
    /// Creates a new mask with all bits cleared.
    pub(crate) fn none() -> Self {
        Self {
            mask: 0,
            group: None,
        }
    }

    /// Creates a new mask with the given bit set, configured for the indicated group.
    pub(crate) fn from_components(mask: usize, group_index: ProcessorGroupIndex) -> Self {
        Self {
            mask,
            group: Some(group_index),
        }
    }

    pub(crate) fn value(&self) -> usize {
        self.mask
    }

    pub(crate) fn contains_by_index_in_group(&self, index: ProcessorIndexInGroup) -> bool {
        self.mask & (1 << usize::from(index)) != 0
    }

    #[cfg_attr(test, mutants::skip)] // False positive due to no-op mutation from | to ^.
    pub(crate) fn add(&mut self, p: &ProcessorImpl) {
        if let Some(group) = self.group {
            assert_eq!(
                group, p.group_index,
                "adding processors from different groups to the same mask is nonsense"
            );
        } else {
            self.group = Some(p.group_index);
        }

        self.mask |= 1 << usize::from(p.index_in_group);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EfficiencyClass;

    #[test]
    fn smoke_test() {
        let mut mask = GroupMask::none();
        assert_eq!(mask.value(), 0);

        let p0 = ProcessorImpl::new(0, 0, 0, 0, EfficiencyClass::Performance);
        let p7 = ProcessorImpl::new(0, 7, 7, 0, EfficiencyClass::Performance);

        mask.add(&p0);
        assert_eq!(mask.value(), 1 << 0);

        mask.add(&p7);
        assert_eq!(mask.value(), (1 << 0) | (1 << 7));

        let mut mask = GroupMask::none();

        let p0_g1 = ProcessorImpl::new(1, 0, 1, 0, EfficiencyClass::Performance);

        mask.add(&p0_g1);
        assert_eq!(mask.value(), 1 << 0);
    }

    #[test]
    #[should_panic]
    fn wrong_group_is_panic() {
        let mut mask = GroupMask::none();
        assert_eq!(mask.value(), 0);

        let p_g0 = ProcessorImpl::new(0, 0, 0, 0, EfficiencyClass::Performance);
        let p_g1 = ProcessorImpl::new(1, 0, 1, 0, EfficiencyClass::Performance);

        mask.add(&p_g0);
        mask.add(&p_g1);
    }

    #[test]
    fn from_components() {
        // Create a mask with bits 0, 3, and 7 set, in group 2
        let mask_value = (1 << 0) | (1 << 3) | (1 << 7);
        let group_index = 2;

        let mask = GroupMask::from_components(mask_value, group_index);

        // Verify mask value
        assert_eq!(mask.value(), mask_value);

        // Verify group is set correctly
        assert_eq!(mask.group, Some(group_index));
    }

    #[test]
    fn contains_by_index_in_group() {
        // Create a mask with bits 0, 3, and 7 set
        let mut mask = GroupMask::none();
        let p0 = ProcessorImpl::new(0, 0, 0, 0, EfficiencyClass::Performance);
        let p3 = ProcessorImpl::new(0, 3, 3, 0, EfficiencyClass::Performance);
        let p7 = ProcessorImpl::new(0, 7, 7, 0, EfficiencyClass::Performance);

        mask.add(&p0);
        mask.add(&p3);
        mask.add(&p7);

        // Check bits that should be set
        assert!(mask.contains_by_index_in_group(0));
        assert!(mask.contains_by_index_in_group(3));
        assert!(mask.contains_by_index_in_group(7));

        // Check bits that should not be set
        assert!(!mask.contains_by_index_in_group(1));
        assert!(!mask.contains_by_index_in_group(2));
        assert!(!mask.contains_by_index_in_group(4));
        assert!(!mask.contains_by_index_in_group(8));
    }
}
