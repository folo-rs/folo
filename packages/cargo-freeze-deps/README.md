# cargo-freeze-deps

A Cargo subcommand that rewrites a `Cargo.toml` file in place, freezing every floating
dependency version requirement to its literal `=X.Y.Z` form.

## Usage

Install with [`cargo binstall cargo-freeze-deps`](https://github.com/cargo-bins/cargo-binstall)
to fetch a prebuilt binary on supported targets (transparently building from source
elsewhere), or `cargo install cargo-freeze-deps` to always build from source. Then:

```text
cargo freeze-deps [--path PATH] [--output PATH]
```

### Arguments

* `-p, --path <PATH>`: path to the `Cargo.toml` file to read. Defaults to `./Cargo.toml`
  in the current directory.
* `-o, --output <PATH>`: path to write the rewritten file to. Defaults to the input path
  (rewriting in place).

The tool works on both workspace-level and package-level `Cargo.toml` files — whichever
dependency tables the input file contains are processed.

## What gets rewritten

The following dependency tables are processed:

* `[dependencies]`
* `[dev-dependencies]`
* `[build-dependencies]`
* `[target.<spec>.dependencies]`, `[target.<spec>.dev-dependencies]`,
  `[target.<spec>.build-dependencies]`
* `[workspace.dependencies]`

For each dependency entry that carries a `version` field — whether as a bare string
(`dep = "1.2.3"`), an inline table (`dep = { version = "1.2.3", ... }`), or a detached
table (`[dependencies.dep]` `version = "1.2.3"`) — the version requirement is rewritten
to its frozen literal.

Dependencies without a version field (pure `path`, `git`, or `workspace = true`) are
left untouched.

`[patch]` and `[replace]` tables are not processed.

## Freezing rules

Each version requirement is rewritten to its lowest matching literal `=major.minor.patch`,
zero-filling any missing components per Cargo's expectations.

| Input              | Output                  |
| ------------------ | ----------------------- |
| `1.2.3`            | `=1.2.3`                |
| `1`                | `=1.0.0`                |
| `1.2`              | `=1.2.0`                |
| `=1.2`             | `=1.2.0`                |
| `^1.2.3`           | `=1.2.3`                |
| `~1.2.3`           | `=1.2.3`                |
| `>=1.2.3`          | `=1.2.3`                |
| `>= 1.2.3`         | `=1.2.3`                |
| `<=1.2.3`          | `=1.2.3`                |
| `1.*`              | `=1.0.0`                |
| `1.2.*`            | `=1.2.0`                |
| `1.2.3-rc.1`       | `=1.2.3-rc.1`           |
| `1.2.3+build.42`   | `=1.2.3` (build dropped) |

### Requirements left unchanged

* Strict inequalities without an equals component: `<1.2.3`, `>1.2.3`.
* Bare `*` (no major number to pin to).
* Multi-comparator requirements such as `">=0.5, <1.0"` — freezing these would either
  silently drop part of the original constraint or require knowledge of which versions
  exist on the registry. Hand-freeze them if needed.

## Errors

The tool fails fast — if any single version requirement is malformed, no file is written.

## Formatting

Comments and overall layout are preserved on round-trip via `toml_edit`. Quote style for
the rewritten version string is reset to the `toml_edit` default (double quotes).
