/// Determines container behavior when the container is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropPolicy {
    /// The container will drop its items when the container is dropped.
    MayDropItems,

    /// The contains will panic if it still contains items when it is dropped.
    ///
    /// This may be valuable if there are external requirements before the items can be dropped.
    /// For example, it may be known that unsafe code is used to create out of band references
    /// to the items, with items only removed after such references have been dropped.
    MustNotDropItems,
}
