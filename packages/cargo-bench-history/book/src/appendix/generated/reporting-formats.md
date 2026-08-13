| Output | How to request it | What it is for | Carries the whole analysis |
|---|---|---|---|
| Text | the default; `--no-text` suppresses it | Reading a report in a terminal | yes |
| Markdown | `--markdown <path>` | Pasting into a pull request or an issue | yes |
| JSON | `--json <path>` | Automation | yes, except the per-commit chart series, which is presentation rather than data |
| Condensed summary | `--markdown-summary <path>` (`analyze` only) | A size-limited destination, such as a pull request comment or a rolling issue body | no — capped at the 10 findings of greatest magnitude, and flattened so the per-set grouping is dropped |

The condensed summary is lossy by design, so it is the one output that must not be automated against: a check reading it cannot distinguish findings that were capped away from findings that were never made.
