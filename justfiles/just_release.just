[group('release')]
audit:
    cargo audit
    
[group('release')]
[script]
prepare-release:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Write-Verbose "Bumping versions and changelogs via 'release-plz update'" -Verbose
    release-plz update

    # Trusted Publishing cannot publish a crate that has never been published before:
    # crates.io only lets you configure a trusted publisher on a crate that already
    # exists, so the very first release of a brand-new crate must be done by hand.
    # Warn about any such crate now, while the release is being prepared, instead of
    # letting the CI publish job discover it and fail. See docs/release-automation.md.
    Write-Verbose "Checking crates.io for never-before-published crates" -Verbose
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json

    # `publish` is null when publishable to any registry and an array otherwise;
    # an empty array means "never publish", a non-empty one names allowed registries.
    $publishable = $metadata.packages |
        Where-Object { $null -eq $_.publish -or $_.publish.Count -gt 0 }

    foreach ($pkg in $publishable) {
        $name = $pkg.name.ToLowerInvariant()

        # crates.io sparse-index path layout, keyed by crate-name length.
        $path = switch ($name.Length) {
            1 { "1/$name" }
            2 { "2/$name" }
            3 { "3/$($name.Substring(0, 1))/$name" }
            default { "$($name.Substring(0, 2))/$($name.Substring(2, 2))/$name" }
        }
        $url = "https://index.crates.io/$path"

        Write-Verbose "Querying $url for '$($pkg.name)'" -Verbose
        $response = Invoke-WebRequest -Uri $url -Method Get -SkipHttpErrorCheck
        switch ($response.StatusCode) {
            200 { }
            404 {
                Write-Warning "$($pkg.name) has never been published. Its first release must be done manually (cargo publish); afterwards configure Trusted Publishing for it on crates.io and re-publish via the GitHub workflow."
            }
            default {
                throw "Unexpected HTTP $($response.StatusCode) checking $url"
            }
        }
    }