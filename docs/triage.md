# Triage guide

This guide collects issue-specific procedures for recognizing failures, applying
supported recovery and gathering evidence for further investigation. Match the
observed symptoms to an entry before following its recovery steps; retry permission
for one failure type does not apply to unrelated failures.

For scheduled failure reporting, issue ownership and repair handoffs, see
[scheduled validation](scheduled-validation.md).

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
