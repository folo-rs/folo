#requires -Version 7

# Coverage report generation for the CI coverage upload (`just coverage-report-ci`).
#
# `cargo llvm-cov report` treats a measured scope in which no object carries coverage mappings as
# a hard error and refuses to export a report at all. A delta-scoped CI run can select exactly
# such a scope - a package that is a pure container for benchmarks, or a deprecated package
# reduced to a documentation shell - where "no coverage" is the correct outcome rather than a
# failure. The decision of whether a failed report run means "nothing to measure" is pure and
# Pester-tested here (Test-CoverageDataMissing); Export-CoverageReport drives the real cargo
# invocation and is covered by CI running the recipe.

Set-StrictMode -Version Latest

function Test-CoverageDataMissing {
    # Decides whether the output of a failed `cargo llvm-cov report` run indicates that the
    # measured scope simply contains no instrumented code, as opposed to a genuine reporting
    # failure (unreadable profile data, a broken toolchain, bad arguments). llvm-cov reports the
    # former as "no coverage data found" for the object file it was asked to export. Every other
    # diagnostic disqualifies the run, so a failure that names both an object without coverage
    # mappings and a real problem stays a failure rather than being waved through.
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][AllowEmptyString()][string[]] $Output
    )

    $missingData = $false

    # Each captured element can itself span several lines: cargo reports a failed llvm-cov
    # invocation as one record that quotes the command and the whole captured stderr.
    foreach ($chunk in $Output) {
        foreach ($line in $chunk -split "`r?`n") {
            $diagnostic = $line.Trim()
            if ($diagnostic -notlike 'error:*') { continue }

            if ($diagnostic -like '*no coverage data found*') {
                $missingData = $true
                continue
            }

            # These two frame the failure without naming a cause of their own. Any other
            # diagnostic describes a different problem, which must not be mistaken for an
            # empty scope.
            if ($diagnostic -like 'error: failed to generate report:*') { continue }
            if ($diagnostic -like 'error: could not load coverage information*') { continue }

            return $false
        }
    }

    return $missingData
}

function Export-CoverageReport {
    # Writes the lcov coverage report the CI upload consumes and returns whether it was written:
    # $true when a report exists at $OutputPath, $false when the measured scope contained no
    # instrumented code and there was nothing to report. Any other failure throws.
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][string] $Toolchain,
        [Parameter(Mandatory)][string] $OutputPath
    )

    # Disable the native-error preference locally so a non-zero exit does not terminate here
    # before we can classify it; we inspect the exit code and output ourselves. 2>&1 merges
    # stderr (where llvm-cov prints its diagnostics) into the captured output.
    $PSNativeCommandUseErrorActionPreference = $false
    $output = @(cargo "+$Toolchain" llvm-cov report --lcov --output-path $OutputPath 2>&1 |
        ForEach-Object { [string] $_ })
    $exitCode = $LASTEXITCODE

    foreach ($line in $output) {
        Write-Host $line
    }

    if ($exitCode -eq 0) {
        return $true
    }

    if (Test-CoverageDataMissing -Output $output) {
        # A report left behind by an earlier run over a different scope would otherwise make the
        # caller believe this scope produced coverage.
        if (Test-Path -LiteralPath $OutputPath) {
            Remove-Item -LiteralPath $OutputPath -Force
        }

        Write-Host 'The measured scope contains no instrumented code - nothing to report.'
        return $false
    }

    throw "cargo llvm-cov report failed with exit code $exitCode."
}

Export-ModuleMember -Function Test-CoverageDataMissing, Export-CoverageReport
