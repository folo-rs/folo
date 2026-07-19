# Recipes for building and previewing the Markdown user-guide book(s) published to GitHub Pages.
# Each published book lives at `packages/<crate>/book/` (a `book.toml` next to a `src/` tree).
# The book workflow (.github/workflows/book.yml) assembles every discovered book into one Pages
# site under `/<book-name>/`, with a generated landing index at the root.

# Installs mdbook and the mermaid preprocessor, pinned to the versions in constants.env. These are
# deliberately kept out of `just install-tools` because only book building needs them.
[group('book')]
[script]
book-install:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo install mdbook@$env:BOOK_MDBOOK_VERSION mdbook-mermaid@$env:BOOK_MDBOOK_MERMAID_VERSION --locked

# Builds a single book into OUT/<NAME>. NAME is the package directory name (the book lives at
# `packages/<NAME>/book`). Used per-book by the build matrix in the book workflow.
[group('book')]
[script]
book-build-one NAME OUT="target/book-site":
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $bookDir = Join-Path "packages" (Join-Path "{{ NAME }}" "book")
    if (-not (Test-Path (Join-Path $bookDir "book.toml"))) {
        throw "No book found at $bookDir (expected a book.toml there)."
    }

    # Regenerate the git-ignored mermaid assets referenced by book.toml. The mermaid config is
    # already committed in book.toml, so this only copies mermaid.min.js / mermaid-init.js.
    mdbook-mermaid install $bookDir

    # mdbook resolves a relative --dest-dir against the book root, so pass an absolute path to
    # land the output under the workspace-rooted OUT directory instead.
    $destination = Join-Path (Get-Location).Path (Join-Path "{{ OUT }}" "{{ NAME }}")
    mdbook build $bookDir --dest-dir $destination
    Write-Host "Built book '{{ NAME }}' into $destination"

# Generates the multi-book landing page (index.html) into OUT, one card per discovered book. The
# card titles and descriptions are read from each book's book.toml (see BookSite.psm1), so adding a
# book needs no edit here.
[group('book')]
[script]
book-landing OUT="target/book-site":
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/book/BookSite.psm1" -Force
    $path = New-BookLandingPage -PackagesRoot "packages" -OutputPath "{{ OUT }}"
    Write-Host "Wrote landing page to $path"

# Builds every discovered book plus the landing page into OUT - the full site, for local preview
# and as the single-command entry point.
[group('book')]
[script]
book-build OUT="target/book-site":
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/book/BookSite.psm1" -Force

    if (Test-Path "{{ OUT }}") { Remove-Item -Recurse -Force "{{ OUT }}" }

    foreach ($book in @(Get-BookInfo -PackagesRoot "packages")) {
        just book-build-one $book.Name "{{ OUT }}"
    }

    just book-landing "{{ OUT }}"
    Write-Host "Assembled site into {{ OUT }} (open {{ OUT }}/index.html)."

# Serves a single book with live reload for local authoring, e.g. `just book-serve cargo-bench-history`.
[group('book')]
[script]
book-serve NAME:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $bookDir = Join-Path "packages" (Join-Path "{{ NAME }}" "book")
    if (-not (Test-Path (Join-Path $bookDir "book.toml"))) {
        throw "No book found at $bookDir (expected a book.toml there)."
    }

    mdbook-mermaid install $bookDir
    mdbook serve $bookDir --open
