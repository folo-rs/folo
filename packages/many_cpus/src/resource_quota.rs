/// The resource quota that the operating system enforces for the current process.
///
/// The active resource quota may change over time. You can use [`SystemHardware::resource_quota()`]
/// to obtain fresh information about the current resource quota at any time.
///
/// [`SystemHardware::resource_quota()`]: crate::SystemHardware::resource_quota
#[derive(Debug)]
pub struct ResourceQuota {
    max_processor_time: f64,
}

impl ResourceQuota {
    pub(crate) fn new(max_processor_time: f64) -> Self {
        Self { max_processor_time }
    }

    /// How much processor time the process is allowed to use.
    ///
    /// This is measured in seconds of processor time per second of real time.
    ///
    /// This will never be more than the number of processors available to the current process.
    /// For example, on a 16-processor system, the maximum value is 16 - by fully loading all
    /// processors, a process could use up to 16 seconds of processor time per second of real time.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let quota = SystemHardware::current().resource_quota();
    /// let max_time = quota.max_processor_time();
    ///
    /// println!("Process is allowed {max_time:.2} seconds of processor time per second");
    /// ```
    #[must_use]
    #[inline]
    pub fn max_processor_time(&self) -> f64 {
        self.max_processor_time
    }
}
