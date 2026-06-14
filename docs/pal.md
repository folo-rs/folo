# Platform abstraction layer (PAL)

Some packages use a platform abstraction layer (PAL) so that platform-specific
code stays isolated from logic code and can be replaced with mocks during
testing. This chapter describes the layout and the constraints on changing the
PAL boundary.

## Facades and abstractions

Some packages like `many_cpus` use a platform abstraction layer, where an
abstraction like `trait Platform` defined in
`packages/many_cpus/src/pal/abstractions/**` has multiple different
implementations:

1. A Windows implementation (`packages/many_cpus/src/pal/windows/**`).
2. A Linux implementation (`packages/many_cpus/src/pal/linux/**`).
3. A mock implementation (`packages/many_cpus/src/pal/mocks.rs`).

Logic code consumes this abstraction via facade types, which can either call
into the real implementation of the build target platform (Windows or Linux) or
the mock implementation (only when building in test mode). The facades are
defined in `packages/many_cpus/src/pal/facade/**` and only exist to be minimal
pass-through layers to allow swapping in the mock implementation in tests.

When modifying the API of the PAL, you are expected to make the API changes in
the abstraction, facade and implementation types at the same time, as the API
surface must match.

The same pattern may also be used elsewhere (e.g. inside the PAL implementations
as a second layer of abstraction, or in other packages).
