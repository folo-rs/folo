#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for MockBenchEngine.psm1. Resolve-MockBenchEngineExecutable is pure, so it is fed
# hand-built `cargo build --message-format=json` streams: the happy path, streams that also contain
# dependency artifacts and non-artifact messages (build-finished, which has no target/executable
# and must not throw under strict mode), interleaved rendered-diagnostic noise, the "last match
# wins" ordering, and the "no engine built" failure.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'MockBenchEngine.psm1') -Force

    $script:EngineArtifact = '{"reason":"compiler-artifact","target":{"name":"mock_bench_engine","kind":["bin"]},"executable":"/tmp/target/mock-engine/debug/mock_bench_engine"}'
    $script:DepArtifact = '{"reason":"compiler-artifact","target":{"name":"serde","kind":["lib"]},"executable":null}'
    $script:BuildFinished = '{"reason":"build-finished","success":true}'
}

Describe 'Resolve-MockBenchEngineExecutable' {
    It 'returns the executable path from the engine artifact' {
        $exe = Resolve-MockBenchEngineExecutable -CargoMessage @($script:EngineArtifact)
        $exe | Should -Be '/tmp/target/mock-engine/debug/mock_bench_engine'
    }

    It 'ignores dependency artifacts and non-artifact messages' {
        $messages = @($script:DepArtifact, $script:EngineArtifact, $script:BuildFinished)
        $exe = Resolve-MockBenchEngineExecutable -CargoMessage $messages
        $exe | Should -Be '/tmp/target/mock-engine/debug/mock_bench_engine'
    }

    It 'does not throw on a build-finished message that lacks target/executable (strict-mode safe)' {
        $messages = @($script:BuildFinished, $script:EngineArtifact)
        { Resolve-MockBenchEngineExecutable -CargoMessage $messages } | Should -Not -Throw
    }

    It 'tolerates interleaved non-JSON rendered-diagnostic lines' {
        $messages = @('warning: unused variable', '', $script:EngineArtifact, 'Compiling mock_bench_engine v0.1.0')
        $exe = Resolve-MockBenchEngineExecutable -CargoMessage $messages
        $exe | Should -Be '/tmp/target/mock-engine/debug/mock_bench_engine'
    }

    It 'returns the last matching engine artifact when several are present' {
        $first = '{"reason":"compiler-artifact","target":{"name":"mock_bench_engine","kind":["bin"]},"executable":"/first/mock_bench_engine"}'
        $last = '{"reason":"compiler-artifact","target":{"name":"mock_bench_engine","kind":["bin"]},"executable":"/last/mock_bench_engine"}'
        $exe = Resolve-MockBenchEngineExecutable -CargoMessage @($first, $last)
        $exe | Should -Be '/last/mock_bench_engine'
    }

    It 'ignores an engine artifact whose executable is null' {
        $nullExe = '{"reason":"compiler-artifact","target":{"name":"mock_bench_engine","kind":["lib"]},"executable":null}'
        { Resolve-MockBenchEngineExecutable -CargoMessage @($nullExe) } | Should -Throw '*could not resolve*'
    }

    It 'throws when no engine artifact is present' {
        $messages = @($script:DepArtifact, $script:BuildFinished)
        { Resolve-MockBenchEngineExecutable -CargoMessage $messages } | Should -Throw '*could not resolve the mock_bench_engine executable*'
    }
}
