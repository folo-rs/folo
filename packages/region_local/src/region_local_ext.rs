use crate::RegionLocal;

/// Extension trait that adds convenience methods to region-local static variables
/// in a `region_local!` block.
pub trait RegionLocalExt<T> {
    /// Executes the provided function with a reference to the value stored
    /// in the current memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use region_local::{region_local, RegionLocalExt};
    ///
    /// region_local!(static FAVORITE_COLOR: String = "blue".to_string());
    ///
    /// let len = FAVORITE_COLOR.with_local(|color| color.len());
    /// assert_eq!(len, 4);
    /// ```
    fn with_local<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R;

    /// Publishes a new value to all threads in the current memory region.
    ///
    /// The update will be applied to all threads in the current memory region
    /// in a [weakly consistent manner][1]. The updated value will not be visible
    /// in other memory regions - storage is independent for each memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use region_local::{region_local, RegionLocalExt};
    ///
    /// region_local!(static FAVORITE_COLOR: String = "blue".to_string());
    ///
    /// FAVORITE_COLOR.set_local("red".to_string());
    /// ```
    ///
    /// Updating the value is [weakly consistent][1]. Do not expect the update to be
    /// immediately visible. Even on the same thread, it is only guaranteed to be
    /// immediately visible if the thread is pinned to a specific memory region.
    ///
    /// ```
    /// use many_cpus::ProcessorSet;
    /// use region_local::{region_local, RegionLocalExt};
    /// use std::num::NonZero;
    ///
    /// region_local!(static FAVORITE_COLOR: String = "blue".to_string());
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
    ///     FAVORITE_COLOR.set_local("red".to_string());
    ///
    ///     // This thread is pinned to a specific processor, so it is guaranteed to stay
    ///     // within the same memory region (== on the same physical hardware). This means
    ///     // that an update to a region-local value is immediately visible.
    ///     let color = FAVORITE_COLOR.with_local(|color| color.clone());
    ///     assert_eq!(color, "red");
    /// }).join().unwrap();
    /// ```
    ///
    /// [1]: crate#consistency-guarantees
    fn set_local(&self, value: T);
}

/// Extension trait that adds convenience methods to region-local static variables
/// in a `region_local!` block, specifically for `Copy` types.
pub trait RegionLocalCopyExt<T>
where
    T: Copy,
{
    /// Gets a copy of the value from the current memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use region_local::{region_local, RegionLocalCopyExt};
    ///
    /// region_local!(static CURRENT_ACCESS_TOKEN: u128 = 0x123100);
    ///
    /// let token = CURRENT_ACCESS_TOKEN.get_local();
    /// assert_eq!(token, 0x123100);
    /// ```
    fn get_local(&self) -> T;
}

impl<T> RegionLocalExt<T> for linked::StaticInstancePerThread<RegionLocal<T>>
where
    T: Clone + Send + Sync + 'static,
{
    #[inline]
    fn with_local<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.with(|inner| inner.with_local(f))
    }

    #[inline]
    fn set_local(&self, value: T) {
        self.with(|inner| inner.set_local(value));
    }
}

impl<T> RegionLocalCopyExt<T> for linked::StaticInstancePerThread<RegionLocal<T>>
where
    T: Clone + Copy + Send + Sync + 'static,
{
    #[inline]
    fn get_local(&self) -> T {
        self.with(|inner| inner.get_local())
    }
}
