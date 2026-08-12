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
why a synthesized model follows the hardware and nothing else. The identity fields used
are ones the platform derives from the hardware rather than from how the machine is
configured, and each field value is rendered in a form determined by the value alone, so
that the platform's own choice of how to write a value down cannot reach the caller.

Synthesis deliberately stops at the granularity of a core design. Identity fields that
describe the stepping of an individual chip are excluded, because including them would
draw a finer distinction than a name-reporting platform draws — a processor name carries
no stepping — and would split callers' per-model data sets more finely than the same
hardware would be split on a platform that provides names. Over-partitioning starves each
partition of data, which is a worse outcome than treating two steppings as one model.

Field values are reduced to one spelling per value before they enter a label, because how
wide a platform writes a value is a presentation choice it makes no promise about, while
the value itself is the hardware. A spelling that cannot be interpreted is kept as it
came, as it still distinguishes what needs distinguishing. The package does not translate
values into vendor or core names: such a mapping is a large table that goes stale with
every hardware release, and a wrong name is worse than a faithful raw identity. A
synthesized label names the fields it was built from, so it is traceable back to its
source and cannot be confused with a platform-provided name.
