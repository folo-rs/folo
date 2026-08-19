# many_cpus implementation

`many_cpus` is a thin published shell over `many_cpus_impl`, which holds the implementation and
carries a platform abstraction layer (see `docs/pal.md`) with one implementation per operating
system plus a fallback for systems we do not describe in detail. This document covers the parts
of that implementation whose reasoning is not evident from the code alone.

Each operating system answers the same questions in its own terms, and those answers differ
enough in shape that the reasoning behind an individual platform implementation belongs to that
platform's own document rather than here:

* [Linux](linux.md)