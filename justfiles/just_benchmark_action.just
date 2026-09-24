# Repository-specific release coordination, kept outside general crate-version planning.
# Ref: docs/benchmark-action-releases.md, "Fast relevance check".
[group('release')]
[script]
benchmark-action-pairing-needed $REPORT $ACTION_MANIFEST='':
    Set-StrictMode -Version Latest
    $ErrorActionPreference = 'Stop'
    $PSNativeCommandUseErrorActionPreference = $true

    $arguments = @('--report', $env:REPORT)
    if ($env:ACTION_MANIFEST) {
        $arguments += @('--action-manifest', $env:ACTION_MANIFEST)
    }
    cargo run --quiet --locked -p benchmark-action-check -- @arguments
