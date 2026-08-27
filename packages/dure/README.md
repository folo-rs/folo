# dure

Detachable Windows console sessions that survive SSH disconnect.

`dure` starts an interactive console process under a per-app supervisor so the
process keeps running after the SSH session that launched it ends. A later SSH
session can attach to the same process. Install with
[`cargo binstall dure`](https://github.com/cargo-bins/cargo-binstall).

Runtime support is Windows only. The crate still compiles on other targets; the
binary exits with an error there.

## Usage

```text
dure run -- <command> [args...]
dure resume [--id <id>]
dure list
dure kill --id <id>
```

`run` always starts a new session in the current directory and attaches immediately.
`resume` attaches to an existing live session (auto-detect by launch directory, or
`--id`). Closing the foreground `dure` process detaches; the supervisor and app
keep running. A new attach displaces any older client.

See `docs/design.md` for the full contract.
