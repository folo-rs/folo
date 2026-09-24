#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the maintained preflight with real Git and Cargo in an isolated standalone
# workspace. Path-version drift must fail without rewriting the historical measurement input.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'CallerFixture.psm1') -Force
}

Describe 'Standalone caller fixture preflight' -Tag Integration {
    BeforeEach {
        $repo = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        $fixture = Join-Path $repo '.github/fixtures/bench-history-caller'
        $dependency = Join-Path $repo 'dependency'
        $null = New-Item -ItemType Directory -Path "$fixture/src", "$dependency/src" -Force
        Set-Content -LiteralPath "$fixture/Cargo.toml" -Value @'
[workspace]
[package]
name = "caller"
version = "0.0.0"
edition = "2024"
[dependencies]
fixture-dependency = { path = "../../../dependency" }
'@
        Set-Content -LiteralPath "$dependency/Cargo.toml" -Value @'
[package]
name = "fixture-dependency"
version = "0.1.0"
edition = "2024"
'@
        Set-Content -LiteralPath "$fixture/src/lib.rs" -Value ''
        Set-Content -LiteralPath "$dependency/src/lib.rs" -Value ''
        git -C $repo init --quiet --initial-branch=main
        git -C $repo config user.name 'Caller fixture'
        git -C $repo config user.email 'fixture@example.invalid'
        git -C $repo config commit.gpgsign false
        cargo generate-lockfile --offline --manifest-path "$fixture/Cargo.toml"
        git -C $repo add --all
        git -C $repo commit --quiet -m 'Clean standalone fixture'
        $script:lock = Join-Path $fixture 'Cargo.lock'
    }

    It 'accepts a clean locked graph without modifying any input' {
        $before = (Get-FileHash -LiteralPath $lock).Hash
        Assert-CallerFixture -Workspace $repo
        (Get-FileHash -LiteralPath $lock).Hash | Should -Be $before
        @(git -C $repo status --porcelain=v1).Count | Should -Be 0
    }

    It 'rejects committed path-version drift that no-deps metadata misses' {
        $path = Join-Path $dependency 'Cargo.toml'
        (Get-Content -LiteralPath $path -Raw).Replace('0.1.0', '0.1.1') |
            Set-Content -LiteralPath $path
        git -C $repo add --all
        git -C $repo commit --quiet -m 'Move path dependency without refreshing nested lock'
        $before = (Get-FileHash -LiteralPath $lock).Hash
        cargo metadata --manifest-path "$fixture/Cargo.toml" --locked --no-deps --format-version 1 |
            Out-Null
        { Assert-CallerFixture -Workspace $repo } | Should -Throw
        (Get-FileHash -LiteralPath $lock).Hash | Should -Be $before
        @(git -C $repo status --porcelain=v1).Count | Should -Be 0
    }

    It 'rejects a missing committed lockfile instead of creating it' {
        git -C $repo rm --quiet -- .github/fixtures/bench-history-caller/Cargo.lock
        git -C $repo commit --quiet -m 'Remove fixture lock'
        { Assert-CallerFixture -Workspace $repo } | Should -Throw
        Test-Path -LiteralPath $lock | Should -BeFalse
    }

    It 'rejects dirty input before collection' -ForEach @('tracked', 'untracked') {
        $path = if ($_ -eq 'tracked') { "$dependency/src/lib.rs" } else { "$repo/untracked.txt" }
        Set-Content -LiteralPath $path -Value '// dirty fixture input'
        { Assert-CallerFixture -Workspace $repo } | Should -Throw
    }

    It 'checks uncommitted version-planning edits without requiring a clean checkout' {
        Set-Content -LiteralPath "$repo/untracked.txt" -Value 'In-progress release work'
        { Assert-CallerFixtureLock -Workspace $repo } | Should -Not -Throw
        $path = Join-Path $dependency 'Cargo.toml'
        (Get-Content -LiteralPath $path -Raw).Replace('0.1.0', '0.1.1') |
            Set-Content -LiteralPath $path
        { Assert-CallerFixtureLock -Workspace $repo } | Should -Throw
        cargo update --manifest-path "$fixture/Cargo.toml" --offline --workspace
        { Assert-CallerFixtureLock -Workspace $repo } | Should -Not -Throw
        { Assert-CallerFixture -Workspace $repo } | Should -Throw
    }
}
