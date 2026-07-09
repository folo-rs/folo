# cbh_git

Implementation crate for [`cargo-bench-history`](https://github.com/folo-rs/folo). Do
not depend on this directly — it carries the process port (launching engine commands and
capturing helper output) and the read-only git-history port, and has no stable public
API. Use the `cargo-bench-history` tool instead.
