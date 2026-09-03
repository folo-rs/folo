# dure

Detachable Windows console sessions that outlive the terminal.

`dure` starts an interactive console process under a per-app supervisor, so the
process keeps running after the terminal that launched it goes away — a closed
window, a dropped SSH connection, or a killed foreground process. A later
terminal attaches to the same process.

`dure` runs only on Windows. Other targets build, so the workspace stays
buildable everywhere, but the resulting binary does nothing except report that
the platform is unsupported.

```text
cargo binstall dure
```

## Usage

```text
dure run -- <command> [args...]
dure resume [<id>]
dure list
dure kill <id>
```

`run` always starts a new session in the current directory and attaches immediately.
`resume` attaches to an existing live session (auto-detect by launch directory, or
an explicit `<id>` argument). Closing the foreground `dure` process detaches; the
supervisor and app keep running. A new attach displaces any older client.

See `docs/design.md` for the full contract.
