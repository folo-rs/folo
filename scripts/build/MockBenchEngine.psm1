#requires -Version 7

# Resolves the built `mock_bench_engine` test-support binary from `cargo build` JSON output.
#
# The mock benchmark engine lives in its own never-published package (packages/mock_bench_engine),
# so Cargo does not expose it to the cargo-bench-history integration tests via CARGO_BIN_EXE_*.
# A test runner builds it once up front and passes the path via MOCK_BENCH_ENGINE so each per-test
# process skips a redundant freshness check. Picking the executable path out of the streamed
# `--message-format=json` records is easy to get subtly wrong - especially under strict mode, where
# a message that lacks a `target` or `executable` field would throw if accessed blindly - so the
# parsing lives here with Pester coverage. The recipe keeps only the actual `cargo build`.

Set-StrictMode -Version Latest

function Resolve-MockBenchEngineExecutable {
    # Scans the JSON lines emitted by `cargo build --message-format=json` and returns the path to
    # the `mock_bench_engine` binary (the last matching compiler-artifact, mirroring cargo's own
    # "last one wins" ordering). Non-JSON lines (e.g. rendered diagnostics) and messages that carry
    # no target/executable are skipped without error, so it is safe under Set-StrictMode. Throws
    # when no matching executable is found, so a silently-missing build surfaces immediately.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][AllowEmptyString()][string[]] $CargoMessage
    )

    $exe = $null
    foreach ($line in $CargoMessage) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }

        $message = $null
        try {
            $message = $line | ConvertFrom-Json -ErrorAction Stop
        } catch {
            # Tolerate non-JSON lines (rendered diagnostics, blank lines) just as the Rust-side
            # resolver does; only parsed objects flow downstream.
            continue
        }

        if (-not (Test-HasProperty $message 'reason') -or $message.reason -ne 'compiler-artifact') { continue }
        if (-not (Test-HasProperty $message 'executable') -or [string]::IsNullOrEmpty($message.executable)) { continue }
        if (-not (Test-HasProperty $message 'target') -or $null -eq $message.target) { continue }
        if (-not (Test-HasProperty $message.target 'name') -or $message.target.name -ne 'mock_bench_engine') { continue }

        $exe = $message.executable
    }

    if (-not $exe) {
        throw 'could not resolve the mock_bench_engine executable from cargo output'
    }
    return $exe
}

function Test-HasProperty {
    # Strict-mode-safe presence check for a property on a (possibly $null) object, so callers can
    # probe optional fields of heterogeneous cargo messages without tripping StrictMode.
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [AllowNull()] $InputObject,
        [Parameter(Mandatory)][string] $Name
    )

    if ($null -eq $InputObject) { return $false }
    return $InputObject.PSObject.Properties.Name -contains $Name
}

Export-ModuleMember -Function Resolve-MockBenchEngineExecutable
