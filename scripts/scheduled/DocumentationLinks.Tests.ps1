#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

# The scheduled script-test domain verifies that relative links in the App setup
# documentation resolve to repository files, without running an agent or contacting GitHub.
BeforeAll {
    $root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
    $documents = @(
        (Join-Path $root '.github\prompts\setup-scheduled-remediation.prompt.md'),
        (Join-Path $root 'docs\scheduled-validation.md')
    )
    foreach ($role in @('scheduled-triage', 'scheduled-intake', 'scheduled-repair')) {
        $documents += Join-Path $root ".github\skills\$role\SKILL.md"
    }
}

Describe 'Scheduled documentation links' {
    It 'resolves relative links to existing repository documents' {
        foreach ($document in $documents) {
            $text = Get-Content -LiteralPath $document -Raw
            foreach ($link in [regex]::Matches($text, '\]\(([^)]+)\)')) {
                $target = ($link.Groups[1].Value -split '#', 2)[0]
                if ($target -eq '' -or $target -match '^[a-z]+:') { continue }
                $path = Join-Path (Split-Path $document -Parent) $target.Replace('/', '\')
                Test-Path -LiteralPath $path -PathType Leaf | Should -BeTrue -Because $link.Value
            }
        }
    }
}
