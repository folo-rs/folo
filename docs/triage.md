# Triage guide

This guide collects issue-specific procedures for recognizing failures, applying
supported recovery and gathering evidence for further investigation. Match the
observed symptoms to an entry before following its recovery steps; retry permission
for one failure type does not apply to unrelated failures.

For scheduled failure reporting, issue ownership and repair handoffs, see
[scheduled validation](scheduled-validation.md).

## Benchmark history findings

Read the full report and the actual per-commit observations before assigning a
cause. A history finding's post-split median is not necessarily its tip value.
Allocator-sensitive Callgrind cases follow the
[workload-matched warmup pattern](callgrind-benchmarks.md#warm-the-allocator-with-the-actual-workload).
Additional scenarios in the same binary can change inlining of shared code even
when the measured case and library source are unchanged; compare generated call
graphs before treating an instruction-count change as a library regression.

For allocation-sensitive workloads, compare allocator selection and memory
locality as well as instruction counts. Untimed allocation can affect a later
read-only traversal through the placement of its data.

### Separate operation cost from measurement context

Equal instruction sequences do not guarantee equal native latency. An allocator
change can alter data placement or the code surrounding a measured loop without
adding work to the loop itself. Function alignment does not fix the alignment of
every basic block within that function. Compare generated code and use controlled
layout experiments before treating such a finding as extra algorithmic work.
An alignment experiment is diagnostic evidence, not by itself a reason to change
the entire suite's build policy.

Inspect the number of simultaneously live inputs in batched benchmarks. Criterion's
adaptive batch policies can retain different working sets when an allocator changes
untimed setup throughput. Using the same `BatchSize` variant does not establish that
those working sets are equal. A fixed input-count control can separate this effect
while preserving the operation, fresh logical state and cleanup boundary. In a
delta-export benchmark, every positive-delta input must still contain a positive
delta; repeatedly exporting the same updated state is not an equivalent control.

Use native counters and component controls to narrow a cause, not to claim more
than they establish. Simulated cache results do not establish native latency,
and a no-op provider removes real backend work rather than repairing it.
Reproduction on one processor model does not establish the exact contribution
on another. Conversely, failure to reproduce locally does not resolve a finding
on the reported hardware.

Track the benchmark-boundary changes and representative-hardware follow-up for
the allocator, notification, export and regional-read investigation in
[#726](https://github.com/folo-rs/folo/issues/726).

### Scope an accepted change by its cause

A blessing records acceptance of an expected behavior change, not the location
where a finding happened to be observed. Select the narrow benchmark-id prefixes
whose behavior changed, then choose discriminant restrictions only where the
expected behavior actually differs.

For a source-wide benchmark change, omit engine, target-triple and machine-key
filters. The acceptance must cover every matching partition, including existing
partitions without measurements at the anchor commit and partitions discovered
later. Hardware-specific investigation evidence does not make an accepted
source-wide change hardware-specific. Conversely, an intentional change limited
to one engine, target or hardware class warrants that explicit restriction.
Leave the other axes unrestricted.

Do not use `bless --all` merely to reach every machine: that switch accepts every
benchmark identity, including future identities. Benchmark identity selection
and discriminant scope are independent. Use the supported `bless` command rather
than editing storage objects; its
[command guide](../packages/cargo-bench-history/book/src/commands/bless.md)
defines persistent scope and revocation.

Verify the **effective persisted scope**, not just the invocation or a successful
exit. Inspect `list blessings --context <anchor> --engine all --target-triple all
--machine-key all`, optionally with `--json <path>`. Confirm the anchor, exact
prefixes and unrestricted axes in the stored record view. A successful analysis
on the investigating machine alone cannot establish that other partitions are
covered. Use the current scoped-blessing-capable executable for both the write
and the audit.

## Codecov verification-key import failures

### Workflow policy

Every Codecov action invocation sets `skip_validation: true` to avoid intermittent
verification-key import failures tracked in [#722](https://github.com/folo-rs/folo/issues/722)
and [codecov/codecov-action#1876](https://github.com/codecov/codecov-action/issues/1876).
This applies to coverage uploads, test-result uploads and the `coverage-notify` job's
notification release on every platform.

The workflows trust the upstream action and HTTPS CLI download without the wrapper's
GPG signature or checksum verification. This setting bypasses executable verification,
not coverage validation, upload authentication or upload-error handling.

### Recognizing and investigating the failure

With executable verification enabled, the action imports a public verification key
before verifying and running its downloaded CLI. Messages containing
`gpg: no valid OpenPGP data found`, `Total number processed: 0` and
`Could not import GPG verification key` identify the bootstrap symptom.
The symptom alone does not establish whether key retrieval, import or the runner
environment caused it; it is not evidence of inadequate coverage or a code defect.

If a run reports this symptom, inspect its exact workflow revision, resolved action
revision and effective inputs to confirm that the failing invocation receives
`skip_validation: true`. Preserve the run/job/attempt links and wrapper/CLI diagnostics
when investigating an invocation that ignores the setting. Do not include credentials.

Require the intended Codecov operation's success, not just a green step: advisory
test-result uploads can report success despite uploader errors. For notification
release, confirm the expected Codecov statuses appear; a failed release can leave them
absent even when `required-checks` passes.

### Safeguards

Keep `fail_ci_if_error: true` on required uploads and notification release, together
with the coverage targets and complete-upload notification gate. Do not use
`continue-on-error`, relaxed coverage targets or general retry wrappers to conceal
download, authentication, report or upload failures.
