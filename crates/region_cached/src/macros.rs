/// Marks static variables as region-cached.
///
/// The static variables are most conveniently used via extension methods on the
/// [`RegionCachedExt`][1] trait. Import this trait when using region-cached static variables.
///
/// # Example
///
/// ```
/// use region_cached::{RegionCachedExt, region_cached};
///
/// region_cached! {
///     static ALLOWED_KEYS: Vec<String> = vec![
///         "error".to_string(),
///         "panic".to_string()
///     ];
///     static FORBIDDEN_KEYS: Vec<String> = vec![
///         "info".to_string(),
///         "debug".to_string()
///     ];
/// }
/// 
/// let allowed_key_count = ALLOWED_KEYS.with_cached(|keys| keys.len());
/// ```
/// 
/// [1]: crate::RegionCachedExt
#[macro_export]
macro_rules! region_cached {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $initial_value:expr; $($rest:tt)*) => (
        $crate::region_cached!($(#[$attr])* $vis static $NAME: $t = $initial_value);
        $crate::region_cached!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $initial_value:expr) => {
        linked::instance_per_thread! {
            $(#[$attr])* $vis static $NAME: $crate::RegionCached<$t> =
                $crate::RegionCached::new($initial_value);
        }
    };
}
