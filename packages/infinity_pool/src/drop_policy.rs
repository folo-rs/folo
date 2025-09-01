/// Determines what happens when a pool is dropped when there are still objects in that pool.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum DropPolicy {
    /// The pool will drop any objects within when the pool is dropped. This is the default.
    #[default]
    MayDropContents,

    /// The pool will panic if it still contains objects when it is dropped.
    ///
    /// This may be valuable if there are external requirements before the objects can be dropped.
    /// For example, it may be known that unsafe code is used to create out of band references
    /// to the objects, with objects only removed after such references have been dropped.
    MustNotDropContents,
}
