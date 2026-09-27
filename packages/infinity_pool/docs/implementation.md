# Pool storage

This guide covers the shared storage layer and its insertion invariants.
The storage layer implements the stable-address and reuse behavior described in
[the design](design.md). Layout-based pools own independently allocated slabs and
track which slabs have vacant slots. Typed and heterogeneous wrappers share this
storage layer rather than implementing separate allocation algorithms.

Each slab initializes its slot metadata when allocated. Vacant slots form an
intrusive freelist; occupied slots own the object's drop operation. An occupied
count supports fullness reporting, while the freelist selects insertion storage.
The exhausted freelist has an out-of-bounds sentinel index.

## Slab insertion invariants

Pool vacancy tracking uses the slab's fullness predicate to retire filled slabs
from insertion selection. Before creating references into slot storage, slab
insertion independently checks in debug builds that the freelist head is in
bounds. Reusing the fullness predicate for this guard would let one predicate
defect disable both tracking and detection, permitting undefined behavior instead
of a deterministic diagnostic.

The guard precedes object initialization and metadata changes. Rejection therefore
leaves existing objects and the freelist untouched. Release builds retain the
unchecked insertion contract and its caller-enforced vacancy requirement.

Boundary tests use small slabs to cover growth, live-object preservation and
vacancy reuse. Guard tests also exercise inconsistent occupied counts, ensuring
diagnostics do not depend on fullness reporting. Mutation testing retains the
fullness predicate substitutions and runs the ordinary library tests without
test-local watchdogs.
