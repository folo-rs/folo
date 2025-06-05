[group('release')]
audit:
    cargo audit
    
[group('release')]
prepare-release:
    release-plz update

[group('release')]
[script]
release:
    $env:GIT_TOKEN = gh auth token
    try {
        release-plz release
    } finally {
        $env:GIT_TOKEN = $null
    }