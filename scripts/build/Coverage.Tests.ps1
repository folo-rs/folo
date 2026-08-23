#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for Coverage.psm1. Test-CoverageDataMissing carries the decision that keeps a
# delta-scoped CI run green when it measures a package with no instrumented code, while still
# failing on a real reporting error - including one whose output also mentions an object without
# coverage mappings - so it is exercised directly here. Export-CoverageReport drives real cargo,
# so it is covered by the recipe running in CI rather than unit-tested here.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Coverage.psm1') -Force
}

Describe 'Test-CoverageDataMissing' {
    It 'recognizes the llvm-cov complaint about an object without coverage mappings' {
        # The blank line mirrors the real output, where the captured stream mixes stdout and
        # stderr - an empty element must not trip parameter binding.
        $output = @(
            "error: failed to generate report: process didn't exit successfully",
            '',
            "error: failed to load coverage: 'target/llvm-cov-target/debug/deps/x-1.exe': no coverage data found",
            'error: could not load coverage information'
        )
        Test-CoverageDataMissing -Output $output | Should -BeTrue
    }

    It 'does not recognize an unrelated reporting failure' {
        $output = @(
            'error: failed to generate report: process exited with code 1',
            'error: invalid instrumentation profile data (file header is corrupt)'
        )
        Test-CoverageDataMissing -Output $output | Should -BeFalse
    }

    It 'does not recognize a failure that also names an object without coverage mappings' {
        $output = @(
            "error: failed to generate report: process didn't exit successfully",
            '',
            "error: failed to load coverage: 'target/llvm-cov-target/debug/deps/x-1.exe': no coverage data found",
            "error: failed to load coverage: 'target/llvm-cov-target/y.profdata': invalid instrumentation profile data",
            'error: could not load coverage information'
        )
        Test-CoverageDataMissing -Output $output | Should -BeFalse
    }

    It 'inspects every line of a single multi-line record' {
        # cargo reports a failed llvm-cov invocation as one record quoting the command it ran and
        # the whole captured stderr, so the classification cannot assume one line per element.
        $output = @(
            @(
                "error: failed to generate report: process didn't exit successfully: ``llvm-cov export`` (exit code: 1)",
                '--- stderr',
                "error: failed to load coverage: 'deps/x-1.exe': no coverage data found",
                'error: could not load coverage information'
            ) -join "`n"
        )
        Test-CoverageDataMissing -Output $output | Should -BeTrue
    }

    It 'does not recognize a mixed failure delivered as a single multi-line record' {
        $output = @(
            @(
                "error: failed to generate report: process didn't exit successfully: ``llvm-cov export`` (exit code: 1)",
                '--- stderr',
                "error: failed to load coverage: 'deps/x-1.exe': no coverage data found",
                'error: invalid instrumentation profile data (file header is corrupt)'
            ) -join "`n"
        )
        Test-CoverageDataMissing -Output $output | Should -BeFalse
    }

    It 'does not recognize an empty output (strict-mode safe)' {
        Test-CoverageDataMissing -Output @() | Should -BeFalse
    }
}
