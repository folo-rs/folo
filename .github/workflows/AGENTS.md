# GitHub workflows — agent instructions

Instructions for editing the workflows in this directory. For the design and its rationale,
see [design.md](design.md). Keep this file limited to actionable instructions; put
high-level design in `design.md` and per-job mechanics in inline YAML comments.

## When you change a workflow

- Update [design.md](design.md) when you change the design; do not record design or history
  here.
- Validate before pushing with `just validate-workflows` (actionlint, which delegates to
  ShellCheck for embedded shell).

## Shell

- Every `run:` step uses `shell: pwsh`; prefer PowerShell over Bash. The `setup-environment`
  composite is the only exception — it bootstraps PowerShell itself.
- Every `run: pwsh` step opens with the standard preamble (`Set-StrictMode -Version Latest` plus
  the two error-preference lines); see `docs/build-and-tooling.md`.
- Keep steps thin: put non-trivial logic in a PowerShell `[script]` `just` recipe the step
  calls, so it runs and is tested locally. Logic worth unit-testing goes one level deeper
  into a module under `scripts/` covered by a Pester suite (`just test-scripts`); see
  `scripts/release/ReleaseAutomation.psm1`. This is also what makes the logic visible to
  `just validate-scripts` (PSScriptAnalyzer) — inline YAML is invisible to both it and Pester.

## Toolchain versions

- Never hardcode toolchain versions. They are defined in `constants.env` and
  `rust-toolchain.toml`; call `just install-tools` / `just <command>` so versions flow
  through automatically.

## Job gating

- A job whose inputs are not Cargo packages (the workflow files, or anything under
  `scripts/`) must run unconditionally — do not gate it on the `delta` job or `skip_all`, or
  a change touching only those files would be validated by nothing. Package-scoped jobs gate
  on `delta`.
