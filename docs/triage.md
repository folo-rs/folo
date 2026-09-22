# Triage guide

This guide collects issue-specific procedures for recognizing failures, applying
supported recovery and gathering evidence for further investigation. Match the
observed symptoms to an entry before following its recovery steps; retry permission
for one failure type does not apply to unrelated failures.

For scheduled failure reporting, issue ownership and repair handoffs, see
[scheduled validation](scheduled-validation.md).

## Codecov verification-key import failures

### Recognizing the failure

The Codecov action imports a public verification key before verifying and running
its downloaded CLI. A failure containing `gpg: no valid OpenPGP data found`,
`Total number processed: 0` and `Could not import GPG verification key` is a known
intermittent bootstrap symptom. See [#722](https://github.com/folo-rs/folo/issues/722)
and [codecov/codecov-action#1876](https://github.com/codecov/codecov-action/issues/1876).
The symptom alone does not establish whether key retrieval, import or the runner
environment caused it; it is not evidence of inadequate coverage or a code defect.
The recovery policy applies to report uploads and the `coverage-notify` job's
notification release on any platform. A failed notification release can leave
Codecov statuses absent even when `required-checks` passes.

### Recovery and investigation

Agents handling CI may perform the following bounded recovery without asking the
user to authorize a routine retry:

1. Inspect the original attempt's logs and confirm the failure is at verification-key
   import. For a coverage upload, coverage generation must have succeeded. Handle
   unrelated failures separately; do not apply this policy to `BAD signature`,
   checksum mismatches, authentication failures, missing reports, coverage-target
   failures or failing tests.
2. Check the workflow run's attempt history and that its commit is still the intended
   revision. If a replacement run for the same ref is queued or running, inspect or
   await it instead of restarting a superseded run. Retain the original run/job/attempt
   links, resolved action revision, wrapper/CLI versions and diagnostics. Allow at most
   one recovery rerun of each affected job per workflow run, across all attempts and
   agent sessions. Inspect earlier attempts' failed steps, not just the attempt counter,
   to distinguish recovery for this symptom from unrelated reruns. If another agent or
   operator already requested recovery, inspect or await it instead. This permits
   recovery on a fresh hosted runner without retrying a persistent fault until it passes.
3. Rerun only affected jobs and their dependent jobs, with no source or verification
   policy changes. Obtain each affected job's `databaseId` from the failed attempt
   before rerunning, rather than assuming a browser URL contains the CLI's job identifier:

   ```text
   gh run view "{{RUN_ID}}" --repo folo-rs/folo --attempt "{{ATTEMPT}}" --json headSha,attempt,jobs
   gh run rerun --repo folo-rs/folo --job "{{JOB_DATABASE_ID}}"
   ```

   Use `gh run rerun "{{RUN_ID}}" --repo folo-rs/folo --failed` only when every failed
   job is affected by this symptom or is a failed dependent, and none has exhausted
   its recovery budget.
4. Inspect the recovery attempt. Require successful GPG signature and checksum
   verification (`CLI integrity verified`) and the intended Codecov operation's success
   (report upload or notification release), not just a green step: advisory test-result
   uploads can report success despite uploader errors. For notification recovery,
   confirm the expected Codecov statuses appear. Apply the normal required-check policy
   to the resulting run. Record the recovery, both attempt links and any action or CLI
   version differences in the existing issue or handoff; a rerun request is not a
   passing check, and a passing retry is a mitigation, not a root-cause fix.
5. If recovery fails, stop recovery retries and investigate. Preserve the resolved
   action revision, wrapper/CLI versions, runner image and both attempts' logs.
   When possible, capture the public key fetch's HTTP status, response metadata,
   visible curl errors and GPG import diagnostics on the affected runner using an
   isolated keyring. Do not include tokens or other credentials. Check upstream
   reports and attach new evidence to the existing issue rather than filing a
   duplicate or changing unrelated Rust code.

### Safeguards

Keep signature verification, checksum verification, `fail_ci_if_error: true` on
required uploads and the coverage targets intact. Do not use `skip_validation`,
`use_pypi`, an unverified predownloaded `binary`, `continue-on-error` or a relaxed
coverage target to bypass this failure. The action already retries key import
internally; the bounded job rerun is for failures that exhaust that recovery,
not a general retry wrapper around validation.
