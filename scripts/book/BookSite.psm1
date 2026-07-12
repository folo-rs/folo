#requires -Version 7

# Backs the book recipes (justfiles/just_book.just) and the book workflow
# (.github/workflows/book.yml): discovers the Markdown books published to GitHub Pages and
# generates the multi-book landing page.
#
# Each published book lives at `<PackagesRoot>/<name>/book/book.toml`. Two things are worth testing
# in isolation: discovering the book set (it drives both the workflow's build matrix and the landing
# cards), and rendering the landing HTML (the title and description are parsed straight out of each
# book.toml, and must be HTML-escaped before they land in the page).

Set-StrictMode -Version Latest

function Get-BookInfo {
    # Returns one object per discovered book - properties Name (the package directory name), Title
    # and Description (parsed from book.toml, falling back to Name / empty) and BookRoot (the
    # directory holding book.toml) - sorted by Name for deterministic ordering. A book.toml that is
    # not directly inside a `book` directory is ignored, so unrelated manifests never leak in.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)][string] $PackagesRoot
    )

    if (-not (Test-Path -LiteralPath $PackagesRoot)) {
        return @()
    }

    $books = [System.Collections.Generic.List[pscustomobject]]::new()
    $manifests = Get-ChildItem -LiteralPath $PackagesRoot -Filter 'book.toml' -Recurse -File |
        Where-Object { $_.Directory.Name -eq 'book' }

    foreach ($manifest in ($manifests | Sort-Object FullName)) {
        $name = $manifest.Directory.Parent.Name
        $text = Get-Content -LiteralPath $manifest.FullName -Raw

        $title = if ($text -match '(?m)^\s*title\s*=\s*"([^"]+)"') { $Matches[1] } else { $name }
        $description = if ($text -match '(?m)^\s*description\s*=\s*"([^"]+)"') { $Matches[1] } else { '' }

        $books.Add([pscustomobject]@{
                Name        = $name
                Title       = $title
                Description = $description
                BookRoot    = $manifest.Directory.FullName
            })
    }

    return @($books | Sort-Object Name)
}

function Get-BookMatrixJson {
    # Returns a compact JSON array of discovered book names, for a GitHub Actions build matrix
    # (`fromJSON(...)`). Always emits an array - even for zero or one book - because ConvertTo-Json
    # would otherwise unwrap a single element to a bare string and break the matrix shape.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][string] $PackagesRoot
    )

    $books = @(Get-BookInfo -PackagesRoot $PackagesRoot)
    $items = $books | ForEach-Object { ConvertTo-Json -InputObject $_.Name -Compress }
    return '[' + ($items -join ',') + ']'
}

function New-BookLandingPage {
    # Writes a self-contained landing index.html into $OutputPath with one card per discovered book,
    # each linking to `./<Name>/`. Titles and descriptions are HTML-escaped. Returns the path to the
    # written file. SupportsShouldProcess because writing the file is state-changing, so `-WhatIf`
    # reports the write without performing it (matches the other file-writing helpers under scripts/).
    [CmdletBinding(SupportsShouldProcess)]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][string] $PackagesRoot,
        [Parameter(Mandatory)][string] $OutputPath
    )

    $books = @(Get-BookInfo -PackagesRoot $PackagesRoot)

    $cards = foreach ($book in $books) {
        $title = [System.Net.WebUtility]::HtmlEncode($book.Title)
        $description = [System.Net.WebUtility]::HtmlEncode($book.Description)
        $href = [uri]::EscapeDataString($book.Name)
        "    <a class=""card"" href=""./$href/""><h2>$title</h2><p>$description</p></a>"
    }

    $html = @"
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Folo books</title>
<style>
  body { font-family: system-ui, sans-serif; max-width: 48rem; margin: 4rem auto; padding: 0 1rem; line-height: 1.5; }
  h1 { font-size: 2rem; }
  .card { display: block; border: 1px solid #ddd; border-radius: 8px; padding: 1rem 1.25rem; margin: 1rem 0; text-decoration: none; color: inherit; }
  .card:hover { border-color: #888; }
  .card h2 { margin: 0 0 0.25rem; font-size: 1.15rem; }
  .card p { margin: 0; color: #555; }
</style>
</head>
<body>
<h1>Folo books</h1>
<p>User guides published from the <a href="https://github.com/folo-rs/folo">folo-rs/folo</a> repository.</p>
$($cards -join "`n")
</body>
</html>
"@

    $indexPath = Join-Path $OutputPath 'index.html'
    if ($PSCmdlet.ShouldProcess($indexPath, 'Write landing page')) {
        New-Item -ItemType Directory -Force -Path $OutputPath | Out-Null
        Set-Content -LiteralPath $indexPath -Value $html -Encoding utf8
    }
    return $indexPath
}

Export-ModuleMember -Function Get-BookInfo, Get-BookMatrixJson, New-BookLandingPage
