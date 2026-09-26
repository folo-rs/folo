#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises cargo-mutants source discovery with the real recipe exclusions. The build-domain
# script suite verifies that library-only testing excludes binary source without losing CLI
# library mutations. Listing does not compile or execute mutations.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Mutants.psm1') -Force
    $script:repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))

    function Get-DiscoveredMutant {
        param(
            [string[]] $Exclusions,
            [string[]] $PackageArguments = @('--workspace')
        )

        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = (Get-Command cargo -CommandType Application | Select-Object -First 1).Source
        $start.WorkingDirectory = $repositoryRoot
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        foreach ($argument in (@('mutants', '--list', '--json') + $PackageArguments + $Exclusions)) {
            $start.ArgumentList.Add($argument)
        }
        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $start
        try {
            $null = $process.Start()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            $process.WaitForExit()
            $output = $stdout.GetAwaiter().GetResult()
            $diagnostic = $stderr.GetAwaiter().GetResult()
            if ($process.ExitCode -ne 0) { throw "Mutation discovery failed: $diagnostic" }
            if ($diagnostic -match '(?i)\bWARN(?:ING)?\b') { throw $diagnostic }
            return @($output | ConvertFrom-Json)
        } finally { $process.Dispose() }
    }
}

# cargo-mutants is not installed on ARM64 by the workspace tool setup. Mutation validation
# targets x64; the same source-selection policy is architecture-independent.
Describe 'Library-only mutation discovery' -Skip:([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') {
    It 'removes binary source mutations and preserves every selected library mutation' {
        $exclusions = @(Get-MutantsExcludeArgument -IsWindowsPlatform $IsWindows -IsLinuxPlatform $IsLinux)
        # Compare against the same platform/package policy without the binary-source exclusions.
        $withoutBinaryExclusions = @()
        for ($index = 0; $index -lt $exclusions.Count; $index += 2) {
            if ($exclusions[$index + 1] -notin @('**/src/main.rs', '**/src/bin/**')) {
                $withoutBinaryExclusions += $exclusions[$index], $exclusions[$index + 1]
            }
        }
        # Use all workspace sources to detect collateral loss in other library packages, too.
        $before = @(Get-DiscoveredMutant -Exclusions $withoutBinaryExclusions)
        $after = @(Get-DiscoveredMutant -Exclusions $exclusions)
        $binaryPattern = '(^|/)src/(main\.rs$|bin/)'
        $binaryMutants = @($before | Where-Object { $_.file -match $binaryPattern })
        $libraryMutants = @($before | Where-Object { $_.file -notmatch $binaryPattern })

        # Nonempty witnesses prevent a no-op exclusion or empty discovery from passing.
        $binaryMutants.Count | Should -BeGreaterThan 0
        $libraryMutants.Count | Should -BeGreaterThan 0
        @($after | Where-Object { $_.file -match $binaryPattern }).Count | Should -Be 0
        @($after.name | Sort-Object) | Should -Be @($libraryMutants.name | Sort-Object)

        $packages = @('cargo-bench-history-stress', 'cargo-release-plan')
        if ($IsWindows) { $packages += 'dure' }
        foreach ($packageName in $packages) {
            @($binaryMutants | Where-Object { $_.package -eq $packageName }).Count | Should -BeGreaterThan 0
            @($after | Where-Object { $_.package -eq $packageName }).Count | Should -BeGreaterThan 0
        }
        foreach ($packageName in @('crp_diag', 'crp_workspace', 'crp_versioning', 'crp_native', 'crp_publication')) {
            @($after | Where-Object { $_.package -eq $packageName }).Count | Should -BeGreaterThan 0
        }
    }

    It 'excludes a src/bin entry point without excluding its library' {
        # The figure generator supplies a real src/bin layout. Isolate the binary-path
        # exclusions from its unrelated whole-package skip to exercise that layout, too.
        $exclusions = @(Get-MutantsExcludeArgument -IsWindowsPlatform $IsWindows -IsLinuxPlatform $IsLinux)
        $binaryExclusions = @()
        for ($index = 0; $index -lt $exclusions.Count; $index += 2) {
            if ($exclusions[$index + 1] -in @('**/src/main.rs', '**/src/bin/**')) {
                $binaryExclusions += $exclusions[$index], $exclusions[$index + 1]
            }
        }
        $packageArguments = @('-p', 'cargo-bench-history-figures')
        $before = @(Get-DiscoveredMutant -Exclusions @() -PackageArguments $packageArguments)
        $after = @(Get-DiscoveredMutant -Exclusions $binaryExclusions -PackageArguments $packageArguments)
        @($before | Where-Object { $_.file -match '/src/bin/' }).Count | Should -BeGreaterThan 0
        $expected = @($before | Where-Object { $_.file -notmatch '/src/bin/' })
        $expected.Count | Should -BeGreaterThan 0
        @($after.name | Sort-Object) | Should -Be @($expected.name | Sort-Object)
    }
}
