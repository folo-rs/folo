use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};

use many_cpus::{ProcessorSet, SystemHardware};

use crate::worker_policy::WorkerGroups;

/// Final pool configuration supplied to
/// [`WorkerPolicy::bind`][crate::worker_policy::WorkerPolicy::bind].
#[derive(Clone, Copy)]
pub struct BindContext<'a> {
    hardware: &'a SystemHardware,
    eager: Option<&'a ProcessorSet>,
    groups: &'a WorkerGroups,
    pool_name: &'a str,
}

impl Debug for BindContext<'_> {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("pool_name", &self.pool_name)
            .field("eager_processor_count", &self.eager.map(ProcessorSet::len))
            .field("worker_group_count", &self.groups.len())
            .field(
                "hardware_processor_count",
                &self.hardware.all_processors().len(),
            )
            .finish()
    }
}

impl BindContext<'_> {
    pub(crate) fn new<'a>(
        hardware: &'a SystemHardware,
        eager: Option<&'a ProcessorSet>,
        groups: &'a WorkerGroups,
        pool_name: &'a str,
    ) -> BindContext<'a> {
        BindContext {
            hardware,
            eager,
            groups,
            pool_name,
        }
    }

    /// Returns the pool's fixed
    /// [`many_cpus::SystemHardware`](https://docs.rs/many_cpus/latest/many_cpus/struct.SystemHardware.html)
    /// instance.
    #[must_use]
    #[inline]
    pub fn hardware(&self) -> &SystemHardware {
        self.hardware
    }

    /// Returns the optional
    /// [`many_cpus::ProcessorSet`](https://docs.rs/many_cpus/latest/many_cpus/struct.ProcessorSet.html)
    /// configured for eager construction.
    #[must_use]
    #[inline]
    pub fn eager(&self) -> Option<&ProcessorSet> {
        self.eager
    }

    /// Returns this pool's worker-group collection.
    #[must_use]
    #[inline]
    pub fn groups(&self) -> &WorkerGroups {
        self.groups
    }

    /// Returns the identifier included in built-in worker thread names.
    #[must_use]
    #[inline]
    pub fn pool_name(&self) -> &str {
        self.pool_name
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::Pool;

    assert_impl_all!(
        BindContext<'static>: Copy,
        Send,
        Sync,
        UnwindSafe,
        RefUnwindSafe
    );

    #[test]
    fn binding_context_formats() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let eager = hardware.all_processors();
        let pool = Pool::builder().hardware(hardware.clone()).build().unwrap();
        let context = BindContext::new(&hardware, Some(&eager), &pool.inner.groups, "formatting");

        _ = format!("{context:?}");
    }
}
