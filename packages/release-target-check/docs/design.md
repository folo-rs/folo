# Bootstrap release verification

This private executable supports the existing publisher until operational
cutover. It has no independently supported product contract. Candidate source
verification belongs to the owning
[cargo-release-plan design](../../cargo-release-plan/docs/design.md).

The **candidate** is the clean source commit being verified. The **release line**
is the frozen branch-tip commit whose first-parent history must contain that
candidate. They have distinct roles and need not identify the same commit.

The adapter preserves exact candidate and release-line identity, requested
package versions, tracked-input requirements and read-only verification.
It performs no registry publication, GitHub writes or source repair.
