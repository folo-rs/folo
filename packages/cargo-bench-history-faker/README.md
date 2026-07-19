# cargo-bench-history-faker

A synthetic benchmark-output generator that validates
[`cargo-bench-history`](../cargo-bench-history) end to end.

> **Unsupported.** This crate exists to test `cargo-bench-history`. Its library
> API and its command line may change in any release without a semver major bump.
> Do not depend on either as a stable contract.

The faker imitates the only parts of a real benchmark engine that
`cargo-bench-history` observes: it writes criterion / Callgrind-summary /
`alloc_tracker` / `all_the_time` output files into a cargo target tree and exits
with a caller-chosen code. It runs no real benchmarks. Paired with the hidden
`cargo bench-history import` command it lets any repository in the organization
exercise the full `cargo-bench-history` storage and analysis pipeline against
curated engine output, using only published binaries — no vendoring and no
private API access.

## Usage

Install with
[`cargo binstall cargo-bench-history-faker`](https://github.com/cargo-bins/cargo-binstall)
to fetch a prebuilt binary on supported targets (transparently building from
source elsewhere), or `cargo install cargo-bench-history-faker` to always build
from source. Then run it in place of a benchmark engine:

```text
cargo-bench-history-faker [--exit-code N]
                          [--callgrind GROUP|MODULE|FUNCTION[|ID[|PACKAGE_DIR]]=IR/BC/BI]...
                          [--criterion GROUP|FUNCTION|VALUE=NANOS[@STDDEV/CILOW:CIHIGH]]...
                          [--alloc-tracker OPERATION=BYTES/COUNT[@BLOW:BHIGH/CLOW:CHIGH]]...
                          [--all-the-time OPERATION=NANOS[@LOW:HIGH]]...
                          [--fail-if-exists PATH] [--chdir DIR]
```

Each `--callgrind`/`--criterion`/`--alloc-tracker`/`--all-the-time` flag may be
repeated to emit several cases, and every value each case records comes straight
from its argument — the faker invents nothing. Output is written under the target
root, honoring `CARGO_TARGET_DIR` exactly as a real harvest does. To feed the
output into a history, point `cargo bench-history import` at the same target
directory.

See the crate documentation for the meaning of each flag.
