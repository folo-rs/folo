[group('release')]
prepare-release:
    release-plz update

[group('release')]
release:
    $env:GIT_TOKEN = gh auth token
    try {
        release-plz release --dry-run
    } finally {
        $env:GIT_TOKEN = $null
    }