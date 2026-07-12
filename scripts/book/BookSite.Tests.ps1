#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for BookSite.psm1.
#
# Get-BookInfo walks the filesystem, so tests build a `packages/<pkg>/book/book.toml` fixture under
# TestDrive covering: a book with a title and description, a book missing both (fall back to name),
# a package with no book (skipped), and a stray book.toml outside a `book` directory (ignored).
# New-BookLandingPage renders those into an index.html, so its tests assert the links, the parsed
# titles and HTML-escaping of special characters.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'BookSite.psm1') -Force
}

Describe 'Get-BookInfo' {
    BeforeEach {
        $script:Root = Join-Path $TestDrive 'packages'

        # alpha: a fully specified book.
        $alpha = Join-Path $script:Root 'alpha' 'book'
        New-Item -ItemType Directory -Path $alpha -Force | Out-Null
        Set-Content -Path (Join-Path $alpha 'book.toml') -Value @(
            '[book]'
            'title = "Alpha Guide"'
            'description = "All about alpha."'
        )

        # beta: a book.toml with neither title nor description (falls back to the package name).
        $beta = Join-Path $script:Root 'beta' 'book'
        New-Item -ItemType Directory -Path $beta -Force | Out-Null
        Set-Content -Path (Join-Path $beta 'book.toml') -Value '[book]'

        # gamma: a package with no book at all (must be skipped).
        New-Item -ItemType Directory -Path (Join-Path $script:Root 'gamma') -Force | Out-Null
        Set-Content -Path (Join-Path $script:Root 'gamma' 'Cargo.toml') -Value '[package]'

        # A stray book.toml not inside a `book` directory must be ignored.
        $stray = Join-Path $script:Root 'delta' 'docs'
        New-Item -ItemType Directory -Path $stray -Force | Out-Null
        Set-Content -Path (Join-Path $stray 'book.toml') -Value '[book]'
    }

    It 'discovers only books inside a book directory, sorted by name' {
        $books = @(Get-BookInfo -PackagesRoot $script:Root)
        $books.Count | Should -Be 2
        $books.Name | Should -Be @('alpha', 'beta')
    }

    It 'parses the title and description from book.toml' {
        $alpha = @(Get-BookInfo -PackagesRoot $script:Root) | Where-Object { $_.Name -eq 'alpha' }
        $alpha.Title | Should -Be 'Alpha Guide'
        $alpha.Description | Should -Be 'All about alpha.'
    }

    It 'falls back to the package name when the title is absent' {
        $beta = @(Get-BookInfo -PackagesRoot $script:Root) | Where-Object { $_.Name -eq 'beta' }
        $beta.Title | Should -Be 'beta'
        $beta.Description | Should -Be ''
    }

    It 'returns an empty result when the packages root does not exist' {
        @(Get-BookInfo -PackagesRoot (Join-Path $TestDrive 'nope')).Count | Should -Be 0
    }
}

Describe 'Get-BookMatrixJson' {
    It 'emits a JSON array of names for multiple books' {
        $root = Join-Path $TestDrive 'packages'
        foreach ($name in @('alpha', 'beta')) {
            $book = Join-Path $root $name 'book'
            New-Item -ItemType Directory -Path $book -Force | Out-Null
            Set-Content -Path (Join-Path $book 'book.toml') -Value '[book]'
        }

        Get-BookMatrixJson -PackagesRoot $root | Should -Be '["alpha","beta"]'
    }

    It 'emits a single-element array (not a bare string) for one book' {
        $root = Join-Path $TestDrive 'solo'
        $book = Join-Path $root 'alpha' 'book'
        New-Item -ItemType Directory -Path $book -Force | Out-Null
        Set-Content -Path (Join-Path $book 'book.toml') -Value '[book]'

        Get-BookMatrixJson -PackagesRoot $root | Should -Be '["alpha"]'
    }

    It 'emits an empty array when no books exist' {
        Get-BookMatrixJson -PackagesRoot (Join-Path $TestDrive 'nope') | Should -Be '[]'
    }
}

Describe 'New-BookLandingPage' {
    BeforeEach {
        $script:Root = Join-Path $TestDrive 'packages'
        $book = Join-Path $script:Root 'alpha' 'book'
        New-Item -ItemType Directory -Path $book -Force | Out-Null
        Set-Content -Path (Join-Path $book 'book.toml') -Value @(
            '[book]'
            'title = "Alpha & Friends"'
            'description = "Special <chars>."'
        )
        $script:Out = Join-Path $TestDrive 'site'
    }

    It 'writes an index.html linking to each book' {
        $path = New-BookLandingPage -PackagesRoot $script:Root -OutputPath $script:Out
        Test-Path -LiteralPath $path | Should -BeTrue
        (Get-Content -LiteralPath $path -Raw) | Should -Match 'href="\./alpha/"'
    }

    It 'HTML-escapes titles and descriptions' {
        $path = New-BookLandingPage -PackagesRoot $script:Root -OutputPath $script:Out
        $html = Get-Content -LiteralPath $path -Raw
        $html | Should -Match 'Alpha &amp; Friends'
        $html | Should -Match 'Special &lt;chars&gt;\.'
        $html | Should -Not -Match 'Alpha & Friends'
    }
}
