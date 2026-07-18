#requires -Version 7

# Argument construction for `just mutants`, the cargo-mutants mutation-testing recipe.
#
# The recipe's fiddly, easy-to-break parts are the long list of platform-dependent path/package
# exclusions and the shard-spec conversion; both live here with Pester coverage so a stray edit to
# the exclusion set (or the exact 1-based -> 0-based shard translation cargo-mutants wants) is
# caught by a test rather than by a wasted CI mutation run. The recipe keeps only the orchestration
# that has real side effects (environment tweaks and the final cargo-mutants invocation).
#
# cargo-mutants can theoretically read exclusions from a config file, but it cannot merge that
# static set with the dynamic, platform-derived set below, so every exclusion is specified as a
# command-line `-e` argument here (see cargo-mutants issue #527).

Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'Sharding.psm1') -Force

function Get-MutantsExcludeArgument {
    # Builds the ordered list of `-e <glob-or-name>` exclusion arguments for cargo-mutants, given
    # the current platform. On non-Windows PowerShell, values that look like wildcard globs are
    # single-quoted so PowerShell's native globbing does not expand them before cargo-mutants sees
    # the literal pattern; on Windows there is no such globbing, so they are passed through as-is.
    # The Windows- and Linux-only source files are excluded on the OTHER platforms (there is no
    # point mutating code that is not even compiled here).
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][bool] $IsWindowsPlatform,
        [Parameter(Mandatory)][bool] $IsLinuxPlatform
    )

    function protect([string] $value) {
        if ($IsWindowsPlatform) {
            return $value
        }
        # Single-quote so PowerShell on Linux does not glob-expand the literal pattern.
        return "'" + $value + "'"
    }

    $exclude = @(
        # Parts of this package require Criterion to work and other parts are currently not tested
        # as there is no public way to simulate a system topology for `many_cpus`.
        '-e', 'many_cpus_benchmarking',

        # Macros are tested via the impl package; mutations in the middle layer might not be detected.
        '-e', 'linked_macros',

        # We do not test facades, as they are just trivial code that forwards calls to real impls.
        '-e', (protect '**/*facade.rs'),
        '-e', 'facade',

        # We have limited coverage of platform bindings because it can be difficult to set up the
        # right scenarios for each, given they are platform-dependent. Instead, we test higher
        # level code using a mock platform.
        '-e', 'bindings',

        # This is just a different type of bindings, skipped for the same reason as `bindings` above.
        '-e', (protect 'packages/many_cpus_impl/src/pal/linux/filesystem/**'),

        # These packages are literally full of synchronization primitives, so 95% of mutations
        # will just cause tests to time out and never complete - pointless to try mutate this stuff.
        '-e', 'events_once',
        '-e', 'awaiter_set',
        '-e', 'events',

        # All this is code only used in tests/benchmarks - we do not test this code itself.
        '-e', (protect 'packages/testing/**'),
        '-e', (protect 'packages/benchmarks/**'),

        # The benchmark faker is a test-support binary (its own `cargo-bench-history-faker`
        # lib+bin package, which the integration tests spawn). It is scaffolding, not shipped
        # code, so mutating it yields no production coverage signal.
        '-e', (protect 'packages/cargo-bench-history-faker/**'),

        # `testing.rs` is an in-workspace test utility (gated behind the `private-test-util`
        # feature, consumed only by the shell crate's tests). It is scaffolding with no public
        # API contract, so mutating it yields no production coverage signal.
        '-e', (protect 'packages/cbh_detect/src/testing.rs'),

        # Some of our systems are single-processor, yet the code may only be meaningfully testable
        # on multi-processor systems. As a "good enough" approximation, we skip mutation testing
        # of code that is only testable in a multi-processor system.
        '-e', (protect 'packages/par_bench/src/resource_usage_ext.rs')
    )

    if (-not $IsWindowsPlatform) {
        $exclude += '-e'
        $exclude += (protect '**/*windows.rs')
        $exclude += '-e'
        $exclude += 'windows'
    }

    if (-not $IsLinuxPlatform) {
        $exclude += '-e'
        $exclude += (protect '**/*linux.rs')
        $exclude += '-e'
        $exclude += 'linux'
    }

    return $exclude
}

function Get-MutantsShardArgument {
    # Converts the shared 1-based "N/M" shard spec into cargo-mutants' native 0-based `--shard`
    # argument (`@('--shard', '0/M') .. @('--shard', '(M-1)/M')`). An empty spec means "no
    # sharding" and yields an empty array so every mutant runs in one job. Throws (via Sharding)
    # for a malformed spec.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $Spec
    )

    if ($Spec -eq '') {
        return @()
    }

    $shard = ConvertFrom-ShardSpec -Spec $Spec
    return @('--shard', ('{0}/{1}' -f ($shard.Index - 1), $shard.Count))
}

Export-ModuleMember -Function Get-MutantsExcludeArgument, Get-MutantsShardArgument
