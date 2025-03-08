Mechanisms for creating families of linked objects that can collaborate across threads while being
internally single-threaded.

The linked object pattern ensures that cross-thread state sharing is always explicit, as well as
cross-thread transfer of linked object instances, facilitated by the mechanisms in this crate. Each
individual instance of a linked object and the mechanisms for obtaining new instances are 
structured in a manner that helps avoid accidental or implicit shared state, by making each instance
thread-local while the entire family can act together to provide a multithreaded API to user code.

More details in the [crate documentation](https://docs.rs/linked/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.