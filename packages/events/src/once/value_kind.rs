/// Represents the kind of value that can be received from an event.
#[derive(Debug)]
pub(crate) enum ValueKind<T> {
    /// The event has been set to a real value.
    Real(T),

    /// The sender has been dropped without ever providing a value.
    Disconnected,
}
