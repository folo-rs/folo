#requires -Version 7

# Shared transient-fault retry helper for the CI tooling.
#
# Network and external-infrastructure operations (crates.io publishes, binary-tool downloads,
# `rustup toolchain install`, read-only `gh` queries) fail intermittently for reasons that have
# nothing to do with the operation itself - a runner disk hiccup, a rate-limit, a dropped TLS
# handshake. Retrying the single failing operation is almost always enough to get past it, so the
# retry lives here once, is exercised by Pester (Retry.Tests.ps1), and is imported by every module
# that drives such an operation rather than each hand-rolling its own loop.
#
# It is deliberately the LOWEST-level seam: callers wrap the smallest possible unit (one download,
# one `rustup` call) so a retry re-runs that unit alone, not the whole recipe. Only clearly
# transient, idempotent operations should be wrapped - see docs in the consuming modules.

Set-StrictMode -Version Latest

function Invoke-WithRetry {
    # Runs $Action, retrying up to $Attempt times when it throws, and rethrows the last failure if
    # every attempt fails. The wait before each retry starts at $DelaySeconds and is multiplied by
    # $BackoffMultiplier after each failure (so a multiplier > 1 gives exponential backoff), capped
    # at $MaxDelaySeconds when that is greater than zero. The defaults (multiplier 1.0, no cap) give
    # a constant delay, so a caller that passes only -Attempt/-DelaySeconds gets simple fixed-delay
    # retries. Returns whatever $Action returns on the first successful attempt.
    #
    # By default it retries on ANY terminating error, so the caller must scope $Action to a
    # genuinely transient, repeatable unit (a retried non-idempotent mutation can double its
    # effect). Pass $RetryOn - a predicate that receives the caught ErrorRecord and returns $true
    # only for retryable failures (e.g. Test-TransientFailure) - to retry selectively and rethrow a
    # deterministic error immediately rather than burning every attempt on it.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][scriptblock] $Action,
        [ValidateRange(1, [int]::MaxValue)][int] $Attempt = 3,
        [ValidateRange(0, [int]::MaxValue)][int] $DelaySeconds = 5,
        [ValidateRange(1.0, [double]::MaxValue)][double] $BackoffMultiplier = 1.0,
        [ValidateRange(0, [int]::MaxValue)][int] $MaxDelaySeconds = 0,
        [scriptblock] $RetryOn
    )

    $delay = $DelaySeconds
    for ($n = 1; $n -le $Attempt; $n++) {
        try {
            return (& $Action)
        } catch {
            $retryable = if ($RetryOn) { [bool] (& $RetryOn $_) } else { $true }
            if ($n -eq $Attempt -or -not $retryable) { throw }

            Write-Warning "Attempt $n of $Attempt failed: $($_.Exception.Message). Retrying in $delay seconds..."
            if ($delay -gt 0) { Start-Sleep -Seconds $delay }

            $delay = [int][math]::Ceiling($delay * $BackoffMultiplier)
            if ($MaxDelaySeconds -gt 0 -and $delay -gt $MaxDelaySeconds) { $delay = $MaxDelaySeconds }
        }
    }
}

function Test-TransientFailure {
    # Heuristic predicate for Invoke-WithRetry's -RetryOn: does $Message look like a transient
    # infrastructure fault worth retrying (server-side 5xx, rate-limiting, a network/TLS blip)
    # rather than a deterministic failure (a 4xx, auth error, or bad request) that will fail
    # identically on every attempt? Deliberately conservative - an unrecognized message is treated
    # as NON-transient so a genuine error surfaces immediately instead of hiding behind slow retries.
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [string] $Message
    )

    if ([string]::IsNullOrWhiteSpace($Message)) { return $false }

    $transient = @(
        'HTTP 5\d\d',
        'rate limit',
        'timed out',
        'timeout',
        'service unavailable',
        'bad gateway',
        'gateway time-?out',
        'temporarily unavailable',
        'connection reset',
        'connection refused',
        'could not resolve host',
        'temporary failure in name resolution',
        'network is unreachable',
        'TLS handshake',
        'unexpected eof'
    ) -join '|'

    return [bool] ($Message -match "(?i)($transient)")
}

Export-ModuleMember -Function `
    Invoke-WithRetry, `
    Test-TransientFailure
