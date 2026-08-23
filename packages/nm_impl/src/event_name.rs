use std::borrow::Cow;

/// Identifies an event in reports and registries.
///
/// Typically, event names are `&'static str`, but when the exact set of events is not known
/// in advance, owned strings are also supported via [`Cow`].
pub type EventName = Cow<'static, str>;
