use std::borrow::Cow;

/// Any value in this range is a valid magnitude of an event.
///
/// We use integers because they are the fastest data type - floating point math is too slow
/// for high-frequency observations.
///
/// If you are measuring fractional data, scale it up to be representable as integers.
/// For example, instead of counting seconds, count milliseconds or nanoseconds.
pub type Magnitude = i64;

/// The name of an event, used for display and keying purposes.
///
/// Typically event names are `&'static str` but for rare cases when the exact
/// set of events is not known in advance, we also support owned strings via `Cow`.
pub type EventName = Cow<'static, str>;
