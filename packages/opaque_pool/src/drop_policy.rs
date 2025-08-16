/// Determines item dropping behavior when the container is dropped.
///
/// By default, the container will drop its items when it is dropped.
///
/// # Examples
///
/// ```
/// use std::alloc::Layout;
///
/// use opaque_pool::{DropPolicy, RawOpaquePool};
///
/// // The drop policy is set at pool creation time.
/// let layout = Layout::new::<u32>();
/// let pool = RawOpaquePool::builder()
///     .layout(layout)
///     .drop_policy(DropPolicy::MustNotDropItems)
///     .build();
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum DropPolicy {
    /// The container will drop its items when the container is dropped. This is the default.
    #[default]
    MayDropItems,

    /// The container will panic if it still contains items when it is dropped.
    ///
    /// This may be valuable if there are external requirements before the items can be dropped.
    /// For example, it may be known that unsafe code is used to create out of band references
    /// to the items, with items only removed after such references have been dropped.
    MustNotDropItems,
}
