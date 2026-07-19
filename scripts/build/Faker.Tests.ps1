#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for Faker.psm1. Resolve-FakerExecutable is pure, so it is fed hand-built
# `cargo build --message-format=json` streams: the happy path, streams that also contain
# dependency artifacts and non-artifact messages (build-finished, which has no target/executable
# and must not throw under strict mode), interleaved rendered-diagnostic noise, the "last match
# wins" ordering, and the "no faker built" failure.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Faker.psm1') -Force

    $script:FakerArtifact = '{"reason":"compiler-artifact","target":{"name":"cargo-bench-history-faker","kind":["bin"]},"executable":"/tmp/target/faker/debug/cargo-bench-history-faker"}'
    $script:DepArtifact = '{"reason":"compiler-artifact","target":{"name":"serde","kind":["lib"]},"executable":null}'
    $script:BuildFinished = '{"reason":"build-finished","success":true}'
}

Describe 'Resolve-FakerExecutable' {
    It 'returns the executable path from the faker artifact' {
        $exe = Resolve-FakerExecutable -CargoMessage @($script:FakerArtifact)
        $exe | Should -Be '/tmp/target/faker/debug/cargo-bench-history-faker'
    }

    It 'ignores dependency artifacts and non-artifact messages' {
        $messages = @($script:DepArtifact, $script:FakerArtifact, $script:BuildFinished)
        $exe = Resolve-FakerExecutable -CargoMessage $messages
        $exe | Should -Be '/tmp/target/faker/debug/cargo-bench-history-faker'
    }

    It 'does not throw on a build-finished message that lacks target/executable (strict-mode safe)' {
        $messages = @($script:BuildFinished, $script:FakerArtifact)
        { Resolve-FakerExecutable -CargoMessage $messages } | Should -Not -Throw
    }

    It 'tolerates interleaved non-JSON rendered-diagnostic lines' {
        $messages = @('warning: unused variable', '', $script:FakerArtifact, 'Compiling cargo-bench-history-faker v0.0.5')
        $exe = Resolve-FakerExecutable -CargoMessage $messages
        $exe | Should -Be '/tmp/target/faker/debug/cargo-bench-history-faker'
    }

    It 'returns the last matching faker artifact when several are present' {
        $first = '{"reason":"compiler-artifact","target":{"name":"cargo-bench-history-faker","kind":["bin"]},"executable":"/first/cargo-bench-history-faker"}'
        $last = '{"reason":"compiler-artifact","target":{"name":"cargo-bench-history-faker","kind":["bin"]},"executable":"/last/cargo-bench-history-faker"}'
        $exe = Resolve-FakerExecutable -CargoMessage @($first, $last)
        $exe | Should -Be '/last/cargo-bench-history-faker'
    }

    It 'ignores a faker artifact whose executable is null' {
        $nullExe = '{"reason":"compiler-artifact","target":{"name":"cargo-bench-history-faker","kind":["lib"]},"executable":null}'
        { Resolve-FakerExecutable -CargoMessage @($nullExe) } | Should -Throw '*could not resolve*'
    }

    It 'throws when no faker artifact is present' {
        $messages = @($script:DepArtifact, $script:BuildFinished)
        { Resolve-FakerExecutable -CargoMessage $messages } | Should -Throw '*could not resolve the cargo-bench-history-faker executable*'
    }
}
