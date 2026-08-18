# many_cpus implementation

`many_cpus` is a thin published shell over `many_cpus_impl`, which holds the implementation and
carries a platform abstraction layer (see `docs/pal.md`) with one implementation per operating
system plus a fallback for systems we do not describe in detail. This document covers the parts
of that implementation whose reasoning is not evident from the code alone.

## Thread affinity masks

An operating system describes which processors a thread may run on as a bit per processor,
packed into an array of machine words. The width of that array is not fixed by the operating
system: the caller allocates the array and passes its size along on every call. The fixed-size
type that the C library offers for this purpose (`cpu_set_t` on Linux) is a convenience for the
common case and not the limit of what the kernel supports; it describes a machine of a
particular size and nothing larger. Machines larger than that exist and are rented by the hour,
so the package represents a mask as a sequence of machine words whose width it chooses.

The mask keeps as many words inline as the platform's own fixed-size type holds, and spills to
the heap only beyond that. Every machine that the C library could have described therefore
imposes no allocation, and only machines it could not describe at all pay for one.

Counting a mask in words rather than bytes is not a stylistic choice: the operating system
rejects a mask whose size is not a whole number of machine words, so making a word the unit of
width means the requirement cannot be violated. It also makes the buffer's alignment correct by
construction.

### Reading an affinity mask

The operating system refuses to report an affinity mask into a buffer too narrow to describe
every processor it knows of, and it reports that refusal as a plain invalid-argument error
without saying how wide the buffer needs to be. Nor can the required width be computed from the
hardware inventory: the operating system sizes its answer by the processors it could ever have,
which includes processors that are absent, offline, or forbidden to the process. A container
restricted to a handful of processors on a very large host is still answered in the host's
terms.

The only way to learn the required width is therefore to offer wider and wider masks until one
is accepted. The search doubles the width each time so that the number of attempts stays
logarithmic in the size of the machine, and it makes a fixed number of attempts so that it
always ends: the final width describes a machine far larger than operating systems support, so
reaching the end means something other than a large machine is wrong, and the error that the
last attempt produced is reported rather than a conclusion of our own. An invalid-argument error
has causes other than a narrow mask — a sandbox may forbid the call outright — which is why that
error is preserved and why any other error ends the search immediately.

A width that the operating system accepted is remembered and tried first next time. It is a
hint and not a conclusion: a process can outlive a change in the machine it runs on, so a width
that stops working merely sends the search widening again from there.

Writing a mask needs none of this. The operating system accepts a mask of any width when setting
affinity and treats the processors beyond it as absent from the set, so a write is a single call
with a mask wide enough for the processors it names.

### Comparing masks

Two masks that name the same processors are the same set even when one of them is wider, so
mask comparison ignores width. Width is a property of the buffer handed to the operating system,
not of the set. A consequence is that comparing masks says nothing about how wide a mask was —
tests that care about the width of a request must inspect the width itself.
