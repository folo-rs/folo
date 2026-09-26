# Bootstrap release verification

This private executable supports the existing publisher until operational
cutover. It has no independently supported product contract. Candidate source
verification belongs to the owning
[cargo-release-plan design](../../cargo-release-plan/docs/design.md).

The adapter preserves exact candidate and release-line identity, requested
package versions, tracked-input requirements and read-only verification.
It performs no registry publication, GitHub writes or source repair.
