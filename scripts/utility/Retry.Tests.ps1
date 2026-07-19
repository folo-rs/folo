#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for Retry.psm1. Start-Sleep is mocked in the module's scope so the retry timing is
# asserted (attempt counts and the backoff schedule) without the tests actually waiting.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Retry.psm1') -Force
}

Describe 'Invoke-WithRetry' {
    BeforeEach {
        Mock Start-Sleep -ModuleName Retry { }
    }

    It 'runs the action once when it succeeds immediately' {
        $script:calls = 0
        Invoke-WithRetry -Attempt 3 -DelaySeconds 1 -Action { $script:calls++ }
        $script:calls | Should -Be 1
        Should -Invoke Start-Sleep -ModuleName Retry -Times 0 -Exactly
    }

    It 'retries until the action succeeds' {
        $script:calls = 0
        Invoke-WithRetry -Attempt 5 -DelaySeconds 1 -Action {
            $script:calls++
            if ($script:calls -lt 3) { throw 'transient' }
        }
        $script:calls | Should -Be 3
        Should -Invoke Start-Sleep -ModuleName Retry -Times 2 -Exactly
    }

    It 'rethrows after exhausting all attempts' {
        $script:calls = 0
        { Invoke-WithRetry -Attempt 3 -DelaySeconds 1 -Action { $script:calls++; throw 'always' } } |
            Should -Throw
        $script:calls | Should -Be 3
    }

    It 'does not sleep after the final failed attempt' {
        { Invoke-WithRetry -Attempt 2 -DelaySeconds 1 -Action { throw 'always' } } | Should -Throw
        Should -Invoke Start-Sleep -ModuleName Retry -Times 1 -Exactly
    }

    It 'returns the output of the action' {
        $result = Invoke-WithRetry -Attempt 2 -DelaySeconds 1 -Action { 'value' }
        $result | Should -Be 'value'
    }

    It 'does not sleep when the delay is zero' {
        { Invoke-WithRetry -Attempt 2 -DelaySeconds 0 -Action { throw 'always' } } | Should -Throw
        Should -Invoke Start-Sleep -ModuleName Retry -Times 0 -Exactly
    }

    It 'uses a constant delay by default' {
        { Invoke-WithRetry -Attempt 3 -DelaySeconds 7 -Action { throw 'always' } } | Should -Throw
        Should -Invoke Start-Sleep -ModuleName Retry -ParameterFilter { $Seconds -eq 7 } -Times 2 -Exactly
    }

    It 'applies exponential backoff capped at the maximum' {
        { Invoke-WithRetry -Attempt 5 -DelaySeconds 2 -BackoffMultiplier 2 -MaxDelaySeconds 10 -Action { throw 'always' } } |
            Should -Throw
        Should -Invoke Start-Sleep -ModuleName Retry -Times 4 -Exactly
        Should -Invoke Start-Sleep -ModuleName Retry -ParameterFilter { $Seconds -eq 2 } -Times 1 -Exactly
        Should -Invoke Start-Sleep -ModuleName Retry -ParameterFilter { $Seconds -eq 4 } -Times 1 -Exactly
        Should -Invoke Start-Sleep -ModuleName Retry -ParameterFilter { $Seconds -eq 8 } -Times 1 -Exactly
        Should -Invoke Start-Sleep -ModuleName Retry -ParameterFilter { $Seconds -eq 10 } -Times 1 -Exactly
    }

    It 'rethrows immediately without retrying when the RetryOn predicate returns false' {
        $script:calls = 0
        { Invoke-WithRetry -Attempt 4 -DelaySeconds 1 -RetryOn { $false } -Action { $script:calls++; throw 'fatal' } } |
            Should -Throw
        $script:calls | Should -Be 1
        Should -Invoke Start-Sleep -ModuleName Retry -Times 0 -Exactly
    }

    It 'retries while the RetryOn predicate returns true' {
        $script:calls = 0
        { Invoke-WithRetry -Attempt 3 -DelaySeconds 1 -RetryOn { $true } -Action { $script:calls++; throw 'transient' } } |
            Should -Throw
        $script:calls | Should -Be 3
    }

    It 'passes the caught error record to the RetryOn predicate' {
        { Invoke-WithRetry -Attempt 2 -DelaySeconds 1 -RetryOn { param($e) $e.Exception.Message -match 'retry-me' } -Action { throw 'do not retry-me' } } |
            Should -Throw
        Should -Invoke Start-Sleep -ModuleName Retry -Times 1 -Exactly
    }
}

Describe 'Test-TransientFailure' {
    It 'treats a 5xx HTTP status as transient' {
        Test-TransientFailure -Message 'HTTP 503: Service Unavailable' | Should -BeTrue
    }

    It 'treats rate limiting as transient' {
        Test-TransientFailure -Message 'GraphQL: rate limit exceeded' | Should -BeTrue
    }

    It 'treats a dropped connection as transient' {
        Test-TransientFailure -Message 'error: connection reset by peer' | Should -BeTrue
    }

    It 'treats a 404 as non-transient' {
        Test-TransientFailure -Message 'gh: Not Found (HTTP 404)' | Should -BeFalse
    }

    It 'treats an auth failure as non-transient' {
        Test-TransientFailure -Message 'gh: authentication required' | Should -BeFalse
    }

    It 'treats an empty message as non-transient' {
        Test-TransientFailure -Message '' | Should -BeFalse
    }
}
