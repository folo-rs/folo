# Pool storage and growth

Object pools support repeated insertion and removal while keeping live objects at
stable memory addresses. Insertion grows capacity as needed without moving existing
objects. Removing an object makes its storage available for reuse.

Pools are intended to be long-lived. Steady-state insertion and removal take
priority over first-insertion and teardown cost. Typed, layout-based and
heterogeneous pools offer reference-counted or manually managed object lifetimes;
the access model does not change the storage stability or growth behavior.

Raw handles require callers to uphold object lifetimes and exclusive-access rules.
Internal diagnostic assertions are not additional public API guarantees.
