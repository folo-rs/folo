use many_cpus::HardwareInfo;

#[cfg_attr(test, mockall::automock)]
pub(crate) trait HardwareInfoClient {
    fn max_memory_region_count(&self) -> usize;
}

#[derive(Debug)]
pub(crate) struct HardwareInfoClientImpl;

impl HardwareInfoClient for HardwareInfoClientImpl {
    #[cfg_attr(test, mutants::skip)] // Trivial fn, tested on lower levels - skip mutating.
    fn max_memory_region_count(&self) -> usize {
        HardwareInfo::current().max_memory_region_count()
    }
}
