/// Information about the resource quota that the operating system enforces for the current process.
///
/// The active resource quota may change over time. You can use [`SystemHardware`][1] to obtain
/// fresh information about the current resource quota at any time.
///
/// [1]: crate::SystemHardware
#[derive(Debug)]
pub struct ResourceQuota {
    max_processor_time: f64,
}

impl ResourceQuota {
    pub(crate) fn new(max_processor_time: f64) -> Self {
        Self { max_processor_time }
    }

    /// How many seconds of processor time the process is allowed to use per second of real time.
    ///
    /// This will never be more than the number of processors available to the current process.
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
    ///
    /// if max_time < 1.0 {
    ///     println!("Process is significantly resource-constrained");
    /// } else if max_time.fract() == 0.0 {
    ///     println!("Process can use {} full processors", max_time as usize);
    /// } else {
    ///     println!("Process has fractional processor allocation");
    /// }
    /// ```
    #[must_use]
    #[inline]
    pub fn max_processor_time(&self) -> f64 {
        self.max_processor_time
    }
}
