# Repository-specific release coordination, kept outside general crate-version planning.
# Ref: docs/action-releases.md, "Fast relevance check".
[group('release')]
[script]
action-pairing-needed $REPORT $ACTION $ACTION_MANIFEST='':
    Set-StrictMode -Version Latest
    $ErrorActionPreference = 'Stop'
    $PSNativeCommandUseErrorActionPreference = $true

    $arguments = @('--report', $env:REPORT, '--action', $env:ACTION)
    if ($env:ACTION_MANIFEST) {
        $arguments += @('--action-manifest', $env:ACTION_MANIFEST)
    }
    cargo run --quiet --locked -p action-pairing-check -- @arguments
