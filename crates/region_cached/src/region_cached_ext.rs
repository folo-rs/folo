use crate::RegionCached;

/// Extension trait that adds convenience methods to region-cached static variables
/// in a `region_cached!` block.
pub trait RegionCachedExt<T> {
    /// Executes the provided function with a reference to the cached value
    /// in the current memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use region_cached::{region_cached, RegionCachedExt};
    ///
    /// region_cached!(static FAVORITE_COLOR: String = "blue".to_string());
    ///
    /// let len = FAVORITE_COLOR.with_cached(|color| color.len());
    /// assert_eq!(len, 4);
    /// ```
    fn with_cached<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R;

    /// Publishes a new value to all memory regions.
    ///
    /// The update will be applied to all memory regions in a [weakly consistent manner][1].
    ///
    /// # Example
    ///
    /// ```
    /// use region_cached::{region_cached, RegionCachedExt};
    ///
    /// region_cached!(static FAVORITE_COLOR: String = "blue".to_string());
    ///
    /// FAVORITE_COLOR.set_global("red".to_string());
    /// ```
    ///
    /// Updating the value is [weakly consistent][1]. Do not expect the update to be
    /// immediately visible. Even on the same thread, it is only guaranteed to be
    /// immediately visible if the thread is pinned to a specific memory region.
    ///
    /// ```
    /// use many_cpus::ProcessorSet;
    /// use region_cached::{region_cached, RegionCachedExt};
    /// use std::num::NonZero;
    ///
    /// region_cached!(static FAVORITE_COLOR: String = "blue".to_string());
    ///
    /// // We can use this to pin a thread to a specific processor, to demonstrate a
    /// // situation where you can rely on consistency guarantees for immediate visibility.
    /// let one_processor = ProcessorSet::builder()
    ///     .take(NonZero::new(1).unwrap())
    ///     .unwrap();
    ///
    /// one_processor.spawn_thread(move |processor_set| {
    ///     let processor = processor_set.processors().first();
    ///     println!("Thread pinned to processor {} in memory region {}",
    ///         processor.id(),
    ///         processor.memory_region_id()
    ///     );
    ///
    ///     FAVORITE_COLOR.set_global("red".to_string());
    ///
    ///     // This thread is pinned to a specific processor, so it is guaranteed to stay
    ///     // within the same memory region (== on the same physical hardware). This means
    ///     // that an update to a region-cached value is immediately visible.
    ///     let color = FAVORITE_COLOR.with_cached(|color| color.clone());
    ///     assert_eq!(color, "red");
    /// }).join().unwrap();
    /// ```
    ///
    /// [1]: crate#consistency-guarantees
    fn set_global(&self, value: T);
}

/// Extension trait that adds convenience methods to region-cached static variables
/// in a `region_cached!` block, specifically for `Copy` types.
pub trait RegionCachedCopyExt<T>
where
    T: Copy,
{
    /// Gets a copy of the cached value in the current memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use region_cached::{region_cached, RegionCachedCopyExt};
    ///
    /// region_cached!(static CURRENT_ACCESS_TOKEN: u128 = 0x123100);
    ///
    /// let token = CURRENT_ACCESS_TOKEN.get_cached();
    /// assert_eq!(token, 0x123100);
    /// ```
    fn get_cached(&self) -> T;
}

impl<T> RegionCachedExt<T> for linked::PerThreadStatic<RegionCached<T>>
where
    T: Clone + Send + Sync + 'static,
{
    #[inline]
    fn with_cached<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.with(|inner| inner.with_cached(f))
    }

    #[inline]
    fn set_global(&self, value: T) {
        self.with(|inner| inner.set_global(value));
    }
}

impl<T> RegionCachedCopyExt<T> for linked::PerThreadStatic<RegionCached<T>>
where
    T: Clone + Copy + Send + Sync + 'static,
{
    #[inline]
    fn get_cached(&self) -> T {
        self.with(|inner| inner.get_cached())
    }
}
