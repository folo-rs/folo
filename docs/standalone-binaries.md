# Standalone binaries

Conventions for standalone binaries — CLI tools, Cargo subcommands, and other
programs a human or an automation step invokes directly. Library crates have
their own rules elsewhere; this chapter is about the behaviour of an executable's
user-facing surface.

## Verbose logging must be explanatory

When a binary offers a verbose / diagnostic mode (typically `--verbose`), each
note it emits must explain the *reasoning* behind a decision, not merely announce
the conclusion. A reader skimming the verbose log should be able to reconstruct
the logic the program followed — which inputs it considered, which ones it
ignored, and why it arrived at the outcome it did. Verbose logs are a debugging
and auditing aid; a log that only states results is nearly useless when the
surprising thing is *why* a result was reached.

Be generous with these notes — cover the major decision points and the inputs
that fed them. A verbose mode that under-explains is a missed opportunity; err on
the side of saying more.

Bad — states only the conclusion, so a surprised reader cannot tell why:

```text
[tool] analysis mode: history
```

Good — names the inputs and the rule, and even calls out what was deliberately
*not* considered:

```text
[tool] analysis mode: history (auto-detected because the target tip is its own
       merge-base with the base branch and no dirty run is recorded on top of it;
       the on-disk working-tree state is deliberately not consulted here)
```

The same principle applies to every derived value a verbose run reports: a
look-back cutoff should say where it came from (an explicit flag vs. a
mode-specific default), an excluded input should say which rule excluded it, and
so on.
