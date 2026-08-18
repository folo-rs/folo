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

    # mdbook reports a broken include directive (and several other content faults) by logging
    # ERROR and carrying on with a zero exit code, silently dropping the affected page from the
    # output. A book that embeds generated fragments would then publish with holes in it and
    # nothing would fail, so the log is inspected and any ERROR line is made fatal here.
    $output = mdbook build $bookDir --dest-dir $destination 2>&1
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { Write-Host $_ }
    # A non-zero exit (a missing mdbook binary, a hard build failure, or a fault reported under a
    # log prefix other than ERROR) must fail the recipe on its own, independently of the ERROR
    # scan below, which only catches the zero-exit content faults.
    if ($exitCode -ne 0) {
        throw "mdbook build exited with code $exitCode while building '{{ NAME }}'."
    }
    $failures = @($output | Where-Object { "$_" -match "^ERROR" })
    if ($failures.Count -gt 0) {
        throw "mdbook reported $($failures.Count) error(s) while building '{{ NAME }}'; the affected pages are missing from the output."
    }

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

# Regenerates the figures, tables and report excerpts embedded in the cargo-bench-history book's
# data pipeline appendix. The appendix quotes concrete numbers throughout and none of them is typed
# by hand, so this is how a change in analysis behaviour reaches the prose that describes it. Review
# the resulting diff: a figure that moved means the pipeline now behaves differently.
[group('book')]
[script]
book-figures:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo run --package cargo-bench-history-figures --bin cbh-figures -- write

# Verifies the checked-in appendix assets match freshly generated content. Also asserted by a test
# in the cargo-bench-history package, so a stale figure fails `just test` whether or not anyone
# remembers this recipe; it exists as the direct way to ask the question.
[group('book')]
[script]
book-figures-check:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo run --package cargo-bench-history-figures --bin cbh-figures -- check

# Writes a page showing every appendix figure on mdBook's default Light and Navy
# background/text-color pairs, for checking legibility without building and re-theming
# the whole book.
[group('book')]
[script]
book-figures-preview:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo run --package cargo-bench-history-figures --bin cbh-figures -- preview
