/// Marks static variables as region-local.
///
/// The static variables are most conveniently used via extension methods on the
/// [`RegionLocalExt`][1] trait. Import this trait when using region-local static variables.
///
/// # Example
///
/// ```
/// use region_local::{RegionLocalExt, region_local};
///
/// region_local! {
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
/// let allowed_key_count = ALLOWED_KEYS.with_local(|keys| keys.len());
/// ```
///
/// [1]: crate::RegionLocalExt
#[macro_export]
macro_rules! region_local {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $initial_value:expr; $($rest:tt)*) => (
        $crate::region_local!($(#[$attr])* $vis static $NAME: $t = $initial_value);
        $crate::region_local!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $initial_value:expr) => {
        $crate::__private::linked::thread_local_rc! {
            $(#[$attr])* $vis static $NAME: $crate::RegionLocal<$t> =
                $crate::RegionLocal::new(|| $initial_value);
        }
    };
}
