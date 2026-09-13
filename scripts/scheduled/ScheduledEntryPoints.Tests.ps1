#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the real check entrypoint and workflow-style invocation against a fixture checker.
# This catches a dropped nonzero exit before GitHub can classify the job.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    $script:fixtureRoot = Join-Path $TestDrive 'entrypoint'
    $script:entryRoot = Join-Path $fixtureRoot 'scripts\scheduled'
    $null = New-Item -ItemType Directory -Path $entryRoot -Force
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Invoke-ScheduledCheck.ps1') -Destination $entryRoot
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'fixtures\execution\CheckExit.psm1') `
        -Destination (Join-Path $entryRoot 'ScheduledExecution.psm1')
}

Describe 'Scheduled check process exit' {
    It 'propagates <code> through <entry>' -ForEach @(
        @{ code = 0; entry = 'script' }, @{ code = 1; entry = 'script' },
        @{ code = 2; entry = 'script' }, @{ code = 3; entry = 'script' },
        # Windows native failures can be signed; they must still make the job fail.
        @{ code = -1073741819; entry = 'script' },
        @{ code = 0; entry = 'workflow' }, @{ code = 2; entry = 'workflow' }
    ) {
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = (Get-Command pwsh).Source
        $start.WorkingDirectory = $fixtureRoot
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.Environment['SCHEDULED_TEST_EXIT'] = [string]$code
        $start.Environment['SCHEDULED_CHECK'] = '{"id":"fixture"}'
        $start.Environment['GITHUB_SHA'] = 'a' * 40
        $capturePath = Join-Path $TestDrive 'entry-invocation.json'
        $start.Environment['SCHEDULED_CAPTURE_PATH'] = $capturePath
        $explicitOutput = Join-Path $TestDrive "runner temp's\scheduled-result"
        $start.Environment['RUNNER_TEMP'] = [IO.Path]::GetDirectoryName($explicitOutput)
        $start.Environment['SCHEDULED_OUTPUT_NAME'] = [IO.Path]::GetFileName($explicitOutput)
        $arguments = if ($entry -eq 'script') {
            @('-NoProfile', '-File', (Join-Path $entryRoot 'Invoke-ScheduledCheck.ps1'))
        } else { @('-NoProfile', '-Command', './scripts/scheduled/Invoke-ScheduledCheck.ps1 -OutputDirectory (Join-Path $env:RUNNER_TEMP $env:SCHEDULED_OUTPUT_NAME); exit $LASTEXITCODE') }
        foreach ($argument in $arguments) { $start.ArgumentList.Add($argument) }
        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $start
        try {
            $null = $process.Start()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            $process.WaitForExit()
            $diagnostic = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
            if ($code -eq 0) { $process.ExitCode | Should -Be 0 -Because $diagnostic }
            else { $process.ExitCode | Should -Not -Be 0 -Because $diagnostic }
            $invocation = Get-Content -LiteralPath $capturePath -Raw | ConvertFrom-Json
            if ($entry -eq 'workflow') {
                $invocation.output | Should -BeExactly $explicitOutput
            } else {
                [IO.Path]::GetDirectoryName($invocation.output) |
                    Should -BeExactly ([IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetTempPath()))
            }
        } finally { $process.Dispose() }
    }
}
