# many_cpus design

`many_cpus` describes the processors and memory regions of the current system so that
callers can place work deliberately on specific hardware. This document covers the
identity that the package attaches to a processor: what a caller can rely on and why the
package draws the distinctions it does.

## Processor identity

A processor is identified by the numeric ID the operating system assigns to it. Every
other property — the memory region it belongs to, its efficiency class, its relative
speed and its model — is descriptive metadata about that processor, not part of its
identity. Two handles to the same processor are the same processor even if the metadata
they carry was gathered differently.

## Processor model

The model is a human-oriented label that identifies the kind of silicon a processor is.
It exists for identification and diagnostics: a caller may show it, log it, or compare
models reported by one version of the package against each other, but its contents are
arbitrary and carry no promised format, so callers must not parse it. What the package
reports for a given processor may change as the package evolves. A fixed placeholder
stands in when the operating system discloses nothing usable.

Operating systems differ in how much they disclose. Some name every logical processor.
Linux names processors on x86-style architectures, but a 64-bit ARM kernel offers a
64-bit process no name at all and describes the processor only through numeric identity
fields — a vendor and a core design. The package therefore reports a platform-provided
name when there is one and otherwise assembles a label out of whatever stable identity
fields the platform does offer, resorting to the placeholder only when nothing
identifying is available at all.

Falling back to the placeholder for an entire architecture would be actively harmful,
because callers use the model to tell hardware apart. Benchmarking tooling, for example,
groups measurement history by the set of models a machine reports; if every machine of an
architecture reports the same placeholder, unrelated hardware collapses into one group
and incomparable measurements are mixed together. Such callers own the consequences of
the model changing between package versions — a change re-groups their data — which is
why the identity fields used are ones the platform derives from the hardware itself
rather than anything that varies with how the machine is configured or observed.

Synthesis deliberately stops at the granularity of a core design. Identity fields that
describe the stepping of an individual chip are excluded, because including them would
draw a finer distinction than a name-reporting platform draws — a processor name carries
no stepping — and would split callers' per-model data sets more finely than the same
hardware would be split on a platform that provides names. Over-partitioning starves each
partition of data, which is a worse outcome than treating two steppings as one model.

Field values are carried through exactly as the platform renders them. The package does
not translate them into vendor or core names: such a mapping is a large table that goes
stale with every hardware release, and a wrong name is worse than a faithful raw
identity, which always distinguishes what needs distinguishing. A synthesized label names
the fields it was built from, so it is traceable back to its source and cannot be
confused with a platform-provided name.
