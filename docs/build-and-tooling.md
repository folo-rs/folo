# Build and tooling

This chapter covers the day-to-day mechanics of running commands in this workspace:
which command runner to use, how to validate changes, how to work across the two
target operating systems, and the scripting conventions for shell snippets and
recipes.

## Standard commands

We use the [`just`](https://github.com/casey/just) command runner for many common
commands. Look inside `*.just` files in `justfiles/` to see the list of available
commands. Some relevant ones are:

* `just build` - build the entire workspace.
* `just package="cpulist many_cpus" build` - target specific packages with a command.
* `just test` - test the entire workspace; this does **not** run doctests, use
  `just test-docs` for that.
* `just docs` - build API documentation.

The `package` argument must be the first argument to any `just` command, if used.

`just package=foo miri` uses nextest's automatically selected `default-miri`
profile, which reports slow tests and fails tests that exceed the per-test
deadline. See [Miri workload guidance](testing.md#keep-miri-workloads-small) for
the runtime budget and how to reduce or exclude unsuitable workloads.

Avoid running `just bench` (wall-clock Criterion benchmarks) without explicit
confirmation: they take a lot of time, and the numbers are also noisy and
machine-dependent - running them on a shared machine produces results that should
not be acted on. `just validate-local` runs `just test-benches-criterion`
instead, giving every Criterion target one smoke iteration without collecting
measurements. The combined `just test-benches` recipe used by CI also runs
Gungraun targets, which means a normal one-shot Valgrind run on Linux and a no-op
stub elsewhere.

`just bench-cg` (Callgrind / Gungraun) is different: it runs each scenario once
under Valgrind's CPU simulator, so the instruction counts and simulated cache
numbers are deterministic and unaffected by other processes on the machine. It is
safe to run `just bench-cg` (or `just package=foo bench-cg`) any time without
asking - including as a smoke test of a new Callgrind benchmark.

We generally prefer using Just commands over raw Cargo commands if there is a
suitable Just command defined in one of the `*.just` files.

Do **not** execute `just gh-release` — it performs real crates.io publishes and is
a CI-only entry point (driven by the release workflow); never run it manually.

Do **not** use VS Code tasks, relying instead on `just` and, if necessary, `cargo`
commands.

The benchmark caller under `.github/fixtures/bench-history-caller` is a standalone Cargo
workspace. Root-workspace lockfile checks do not cover it. After moving its path dependencies,
refresh its lock with `cargo update --manifest-path .github/fixtures/bench-history-caller/Cargo.toml
--offline --workspace`. `just verify-lockfile` checks both workspaces during local version planning.
After committing, run `just verify-caller-fixture` from the clean checkout.
This uses full locked metadata and rejects dirty inputs rather than repairing them during
collection. The synthetic benchmark emits fixed data, not timed measurements; use
`cargo bench --manifest-path .github/fixtures/bench-history-caller/Cargo.toml --locked --bench synthetic -- --test`
for a local smoke run. The hosted canary checks the actual head and its first parent, so both
must carry consistent fixture locks.

## Development tool installation

`scripts/setup/install-just.ps1` bootstraps the command runner without requiring Just.
It and the `install-tools` / `book-install` recipes share `scripts/setup/CargoTools.psm1`.
Versions come from `constants.env`, read directly during bootstrapping and through
Just's dotenv environment in recipes. The bootstrap runs in PowerShell because the
Rust automation environment and command runner are not yet prepared.

Cargo tools prefer binaries from their publishers, with `cargo install --locked`
as the fallback when no usable archive is found. Exact version pins apply on both
paths, including downgrades; an already matching installation is reused. Git-revision
installs and known source-only tools retain explicit source installation. Platform
exclusions and the Gungraun runner/library and mdBook/preprocessor version pairings
apply equally to binary and source installs.
The cargo-audit invocation supplies its publisher's nested release-tag URL explicitly,
because automatic discovery does not find those archives.
Cargo-sort uses crates.io source because the publisher's archive at the pinned release
reports a different version.

The binstall bootstrap downloads an official archive for the host platform and
verifies a checked-in SHA-256 before executing it. Update the archive digests in
`CargoTools.psm1` together with `CARGO_BINSTALL_VERSION`, obtaining them from the
corresponding GitHub release's asset metadata. Bootstrap uses the executable at
the chosen installation root rather than another copy on PATH.

Development tools installed by binstall are trusted publisher builds, not locally
reproduced builds of the crates.io source. Signatures are verified when the publisher
configures them; unsigned artifacts are allowed. Exact versions do not pin archive
contents, compiler choices or dependency builds. `--locked` constrains source fallback
only. This tradeoff avoids compiling the toolset on every cold setup without requiring
every publisher to support signed releases.

Quickinstall's third-party builds and telemetry are disabled. Binstall does not
discover local GitHub credentials automatically; public downloads work without login.
CI supplies its ephemeral job token to reduce API rate limiting. No PAT or persistent
authentication change is required. Downloads require access to crates.io and publisher
release hosts, normally GitHub. Compilation tools remain prerequisites for workspace
builds and source fallback. Cold setup benefits most; existing caches still matter.

Cargo tools install into `CARGO_INSTALL_ROOT`, then `CARGO_HOME`, then the default
home Cargo directory, in that precedence order. Its `bin` directory must already be
on PATH for subsequent tool use; setup does not edit persistent PATH or execution
policy. Cargo/binstall registration metadata is retained for version reconciliation
and cache restoration. The standalone non-Cargo installers keep their own destinations.
The shared Cargo installer selects its root explicitly rather than reading Cargo's
`install.root` configuration key.

### Windows source-build troubleshooting

Normal setup uses a prebuilt binstall executable and does not need binstall's native
compression or cryptography build dependencies. A manual source installation should
use `cargo install cargo-binstall@<pin> --locked`, with the pin from `constants.env`.
When it fails, inspect the original error before installing additional prerequisites:
dependency-resolution failures and missing native tools require different remedies.
Visual Studio's C++ workload does not imply that its bundled CMake is on the calling
shell's PATH. Check CMake or assembler requirements against the failing dependency
and version; do not install them speculatively or remove the workspace's existing
compiler prerequisites merely because binaries usually avoid tool compilation.

## Validating changes

Validate changes via `just validate-local`. This runs a number of different checks
and will uncover most issues. If you only touched a few packages, scope it to them
via `package="foo bar"`. It also smoke-runs the selected packages' Criterion
targets through `just test-benches-criterion`; see Standard commands for the
distinction between local and combined CI benchmark smoke passes.

`just validate-local` is always shallow validation. Use
`just package="foo bar" validate-deep-local` to run Miri, mutation testing,
many-seed Miri and careful checking on the current platform. These recipes are
independent: deep validation does not implicitly rerun the shallow suite.

The **Standard validation** workflow performs shallow PR/push checks.
**Merge queue validation** runs full-workspace dev Clippy, formatting and version readiness.
**Deep validation** reuses the complete standard suite and runs the full deep suite on merged
`main`, without affected-package or tooling-input selection.
Scheduled checks invoke the same `just miri`, `just miri-harder`, `just mutants`
and `just careful` recipes used locally. Recipes own the toolchains, test runners,
helper preparation and check behavior; scheduling only selects platform, packages
and shards and captures diagnostics.
CI groups related commands into sequential steps in shared jobs and diagnostic-producing
matrix entries rather than running one monolithic local recipe. Failed prerequisites and
cancellation block dependent work, but check failures do not suppress independent checks.
Any failed check fails the job; diagnostic collection and resource cleanup remain available.
Repair authors also run the particular deep checks needed to verify
their repair locally and link the results for review. Scheduling belongs to workflow orchestration, not
to the definitions of the local recipes. To run just mutation testing, use
`just package="foo bar" mutants`. It runs the unmutated baseline before testing
mutants, without assuming shallow validation already ran. The optional final
`OUTPUT` argument selects the native tool-output directory; for example,
`just package=foo mutants 1/8 false mutation-results` saves a shard's output
there. The intervening `CAREFUL` argument retains its normal default.
Mutation timeouts and missed mutations remain
anomalies; changing enforcement cadence does not relax test-quality requirements.

See [scheduled validation](scheduled-validation.md) for readable failure reporting,
GitHub issue triage, repair ownership and reproducible Local App setup. Empty App
scans need only GitHub access; prepare Rust/WSL tooling in repair sessions when
applicable rather than on every polling-session creation.

We operate under a **zero warnings allowed** requirement - fix all warnings that
validation generates.

For asynchronous GitHub results, follow the
[check-waiting policy](git-workflow.md#check-waiting-and-merge-queue-readiness).
Low-signal optional checks do not delay readiness or authorized queue submission;
this does not reduce required validation or relevant local checks.

### Mutation target selection

`just mutants` selects Cargo library unit-test targets with `--lib`.
The same selection applies to the unmutated baseline and each mutant. Integration
tests, doctests, binary targets, examples, and benchmarks are not mutation-test targets; ordinary
testing and coverage retain their own selections. See
[the mutation-testing policy](testing.md#mutation-testing-target-selection).
Unit tests exercise in-process logic; real external interactions belong in
integration targets even if they could technically be compiled by `--lib`.
See [the test boundary](testing.md#unit-tests-stay-inside-the-process).

The selectors live in `.cargo/mutants.toml` under `additional_cargo_args`, so
cargo-mutants applies them to both its test-build and test-execution commands.
`additional_cargo_test_args` alone would still compile integration targets during
the preceding build phase. CLI applications expose their implementation through a
library crate rather than adding binary targets to the mutation run.

Cargo target selection does not restrict cargo-mutants source discovery.
`Get-MutantsExcludeArgument` in `scripts/build/Mutants.psm1` excludes `**/src/main.rs`
and `**/src/bin/**` alongside the platform-specific source exclusions. These paths
contain binary shells and their private helpers, not shared library implementation.
Keep new binary targets in these conventional locations; a custom binary source
layout needs corresponding source-selection handling.

Sharded runs use `--sharding round-robin` to distribute mutations from expensive
source regions across runners rather than assigning consecutive source ranges to
each shard. The shared recipe applies this to both local and CI runs while
preserving the 1-based `N/M` shard argument. Unsharded runs select every mutant.

### Coverage status policy

Codecov requires at least 95% coverage for both the overall project and the
changed lines in a pull request. These are fixed targets, not comparisons against
the base commit, so a decrease that still meets the target does not fail the check.
This allows practical coverage gaps while retaining a meaningful coverage floor.
The PR comment remains advisory and reports coverage changes independently of
whether the checks pass.

The policy lives in `codecov.yml`. Statuses and comments are posted only after all
coverage-producing jobs finish, so they reflect the complete set of reports.

For Codecov verification-key import failures, follow the
[Triage guide](triage.md#codecov-verification-key-import-failures).

### Coverage target selection

`coverage-measure` uses Cargo's `--tests --examples` selection. `--tests` includes
library and binary unit tests and integration tests; `--examples` includes example
test harnesses. This works for library-only, binary-only and mixed package selections
without requiring every selected package to define a library.
See [Cargo target selection](https://doc.rust-lang.org/cargo/commands/cargo-test.html#target-selection).

Benchmark targets stay out of coverage and run through the separate `test-benches`
smoke pass. Do not opt benchmark targets into `test = true`, because Cargo includes
such targets in `--tests`. Doctests retain their separate `test-docs` pass.
The library-only selectors in many-seed Miri and exact library replays are deliberate
scope restrictions, not general test or coverage selection.

### Example execution

`just run-examples` prebuilds the selected examples before starting their runtime
watchdogs. The recipe prepares their environment through
`scripts/build/Examples.psm1`: it resolves Cargo's configured target directory once
with `cargo metadata --no-deps --locked`, exports the resulting absolute
`CARGO_TARGET_DIR`, and enables the examples' `IS_TESTING` smoke paths.

Passing the resolved directory avoids nested Cargo invocations from Criterion's
constructor and from measurement-report output. Criterion's own discovery uses
full-workspace metadata, which can resolve and download unrelated dependencies;
that preparation does not belong inside an example's runtime budget. Cargo remains
the authority for custom output-directory configuration.

Examples should complete within single-digit seconds. Criterion-based examples
use its single-iteration test mode by default rather than collecting benchmark
samples or performing statistical analysis. The watchdog is a last-chance
safeguard, not the expected execution budget. If it fires, the runner retains and
prints partial child output to identify the last completed phase.

## Bicep validation

`just validate-bicep` compiles maintained `.bicep` and `.bicepparam` inputs under `infra/`
and the embedded `cargo-bench-history` Azure bundle. It uses the compiler pinned by
`BICEP_VERSION`, installed through `just install-bicep` (also part of `just install-tools`).
The check neither signs in to Azure nor deploys or queries resources.

Compilation validates syntax and resource types against that compiler's API catalog.
The root `bicepconfig.json` also enables `use-recent-api-versions`. Compiler and linter
warnings fail the check alongside errors; SARIF diagnostics and generated ARM JSON stay
under `target/bicep-validation`, not beside source files.

The API catalog is an offline check, not proof that an API is available in a particular
subscription/region or that deployment permissions and policy allow it. Real deployment
still performs Azure's own validation. Update the compiler pin deliberately when its
catalog needs newer resource definitions.

Standard validation selects this check for changed Bicep inputs, compiler configuration
or invocation code and runs it inside the existing script-validation job. Main and deep
validation retain full scope. See
[workflow implementation](../.github/workflows/implementation.md#bicep-validation).

## Multiplatform codebase

This is a multiplatform codebase. In some packages you will find folders named
`linux` and `windows`, which contain platform-specific code. When modifying files
of one platform, you make the equivalent modifications in the other.

On a typical Windows PC with WSL installed, you can invoke any Linux commands
using the syntax `wsl -e bash -l -c "command"`. For example, to run the
standard validation on both Windows and Linux, execute:

1. `just validate-local`
2. `wsl -e bash -l -c "just validate-local"`

### Platform support and validation

ARM64 is a nice-to-have, minimally supported target. Its validation is best-effort:
authors may skip ARM64 builds, tests and deep checks without separate approval when
the change should logically work on ARM64. Passing checks of the same logic on
other platforms are especially useful evidence. This also applies to scheduled
repairs when the correction addresses the same shared failure on ARM64.

For review and readiness decisions, expected ARM64 success is sufficient in these
cases. Record the skipped scope and the reasoning, not an executed pass. Missing
ARM64 hardware, toolchains or local validation alone does not require installation,
delay the PR or warrant `needs-human`.

Base the expectation on the affected code, not just a green result elsewhere.
Architecture-specific behavior or an unexplained ARM64 failure needs investigation
before assuming the change works. Existing scheduled ARM64 checks remain useful
best-effort coverage; this policy does not change their commands or reporting, turn
observed failures into passes, or bypass required CI checks.

## Automation language and boundaries

For issue classification, discovery scope and repeatable notifications, follow
the [automation guidelines](automation.md).

Prefer **nonpublished Rust utilities** for automation logic, especially structured
configuration parsing, data transformations and policy decisions. Reuse workspace
dependencies and validation conventions; an internal automation task is not a reason
to publish a new crate or introduce another language runtime.

Use PowerShell when executing Rust is impractical at the calling boundary. Examples
include bootstrapping the Rust toolchain, reporting a failed toolchain setup, or
coordinating native App operations before a prepared Rust environment is available.
Explain the actual constraint in the owning implementation guide and script rather
than treating familiarity with shell scripting as justification.

Keep the distinction between logic and process orchestration clear. A thin
PowerShell wrapper can prepare command arguments, invoke a trusted Rust utility and
propagate its outcome. Parsing and semantic decisions belong in the utility when
that environment can execute it.

For example, the failure-reporting job inside `deep-validation.yml` must work even
when Rust setup failed. It uses the runner's existing PowerShell and GitHub CLI
rather than building a reporting executable. Like the check jobs, it runs from the
workflow's main checkout; it simply needs Actions-read and issue-write permissions
to collect diagnostics and file an issue. See
[failure reporting](../.github/workflows/implementation.md#failure-reporting).

## Scripting

You can assume PowerShell 7 (`pwsh`) is available on every operating system and
environment. Where a script is justified, prefer PowerShell 7 commands to Bash
commands.

### Script purpose and decision comments

Every script, module and executable test fixture needs an inline purpose comment
explaining why it exists and how it participates in the repository's workflows.
Name its caller or entry point, the responsibility it owns and any important
authority or lifetime boundary. A filename, a list of exported functions or
"helpers for the workflow" is not an explanation. Test scripts identify the
contract or failure class they protect.

Document non-obvious decisions where they are implemented: why evidence is rejected,
why a state transition retains ownership, why a subprocess is isolated, or why an
operation must precede another. Link the relevant design or implementation heading
instead of duplicating a long rationale. Comments must describe supported behavior,
not development history or merely repeat the following statement.

Apply the same standard to workflow steps and just recipes. A step that implements
a design obligation needs a nearby justification with a link to that document's
relevant heading. Shared comments may explain a cohesive step group; ordinary
boilerplate does not need repeated narration.

### Every PowerShell snippet starts with the standard preamble

Every PowerShell snippet - each `[script]` block in a `.just` file, each `shell: pwsh`
`run:` step in a workflow, and every standalone `.ps1`/`.psm1` under `scripts/` - must start
with the standard preamble:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
```

`Set-StrictMode -Version Latest` turns silent footguns (referencing an uninitialized variable,
reading a non-existent property, indexing out of bounds) into hard errors. The two preference
lines ensure that commands producing nonzero exit codes are treated as errors and fail the
script. (Standalone scripts additionally set `$VerbosePreference = 'Continue'`; module files set
strict mode once at the top rather than per function.)

A workflow `run:` that only invokes a standalone `.ps1` may delegate the preamble to that
script. Keep the preamble inline when the step performs any additional PowerShell logic.

### PowerShell linting

`just validate-scripts` runs [PSScriptAnalyzer](https://github.com/PowerShell/PSScriptAnalyzer)
over everything under `scripts/`, `infra/azure-bench-history-prod/`, and the embedded deployment bundle under
`packages/cargo-bench-history/src/azure_bundle/`, together with that package's native
PowerShell fixtures, gating on Error/Warning findings. The bundle is shipped application
code even though its driver executes in PowerShell.
The rule set lives in
`PSScriptAnalyzerSettings.psd1`, supplemented by repo-local custom rules in
`scripts/analyzer/FoloAnalyzerRules.psm1` - which catch classes the built-in rules (and strict
mode) miss, such as a `foreach` whose loop variable case-insensitively collides with the
collection it enumerates. It runs as part of `just validate-local` and the CI `test-scripts` job.
Silence a genuine false positive with a justified
`[Diagnostics.CodeAnalysis.SuppressMessageAttribute(...)]`, never by relaxing the gate; the tree
is expected to be finding-free.

The wrapper retains runtime/module metadata, full managed/inner exception diagnostics
and the analyzer's file/rule trace under `target/script-analysis/`. Standard validation
uploads those diagnostics even when the analyzer itself fails. Diagnostic collection
does not retry, suppress rules or turn an engine failure into a successful result.
The runner discovers the built-in and custom rules and analyzes the tree in
sequential rule passes. Each pass excludes its peers while retaining the original
settings, including severity, exclusions and opt-in rule configuration. Every
configured rule still runs; only concurrency changes. This trades repeated parsing
for reliable analysis using the analyzer's supported command interface. The custom
module is loaded only for its own passes: loading excluded external rules would
otherwise create an unnecessary runspace pool for every file in every built-in pass.

PSScriptAnalyzer runs script rules concurrently and caches PowerShell `CommandInfo`
objects from a runspace pool. Off-pipeline parameter resolution can return missing
metadata while other rules use that runspace, causing a null reference in
`CommandInfo.ResolveParameter`. This is not an invalid export in the analyzed
module. Initializing a command in advance is insufficient: `ResolveParameter`
requests merged metadata on every call, so even a warmed command-info cache still
accesses runspace state. The pinned analyzer reuses its helper and command cache;
that reuse does not make concurrent metadata access safe.
See [PowerShell/PowerShell#27842](https://github.com/PowerShell/PowerShell/issues/27842)
and [PowerShell/PSScriptAnalyzer#1538](https://github.com/PowerShell/PSScriptAnalyzer/issues/1538).
`scripts/build/tests/CommandMetadataProbe.ps1 -Concurrent` provides a bounded
runtime-level reproducer using real pooled command lookups, exported-command
parameter resolution and dynamic parameters. It reports every observed failure
and fails the invocation; a passing concurrent run does not disprove the race.
Without `-Concurrent` it exercises the serial control. The Pester integration
suite also checks actual built-in/custom findings and settings behavior through
the workspace runner. Repeated fresh-process `just validate-scripts` runs cover
the full repository; retain each invocation's diagnostics when investigating
intermittency rather than retrying a failed gate.

PSScriptAnalyzer can only see `.ps1`/`.psm1` files, so **nontrivial** inline PowerShell
is not linted where it sits. Keep justfile `[script]` blocks and workflow `pwsh`
steps thin. Put automation logic in a nonpublished Rust utility when practical.
When the calling environment requires PowerShell, put nontrivial orchestration in
a module under `scripts/`, covered by Pester (`just test-scripts`) and the analyzer.
The recipe or workflow step then imports and invokes that boundary. A preamble and
a command or two may stay inline.

`just test-scripts` discovers the full PowerShell suite. Pass space-separated script-directory
domains, for example `just test-scripts "book release"`, to run their union. CI selects these
domains from changed inputs and native-helper dependency impact; script analysis and workflow
lint have independent selections. Main pushes retain full validation. See
[non-Cargo change planning](../.github/workflows/implementation.md#non-cargo-change-planning)
for ownership, shared-input rules and the lightweight workflow-lint environment.
