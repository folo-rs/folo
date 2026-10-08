| Output | How to request it | What it is for | What it carries |
|---|---|---|---|
| Text | the default; `--no-text` suppresses it | Reading a report in a terminal | every finding; omits the per-reason census when findings exist |
| Markdown | `--markdown <path>` | Pasting into a pull request or an issue | every finding and a final Coverage section with the complete per-reason census |
| JSON | `--json <path>` | Automation | the complete census always, and every finding; omits the per-commit chart series, which is presentation rather than data |
| Condensed summary | `--markdown-summary <path>` (`analyze` only) | A size-limited destination, such as a pull request comment or a rolling issue body | at most the 10 findings of greatest magnitude, flattened so the per-set grouping is dropped — lossy by design |
| Outcome | `--outcome <path>` (`analyze` only) | Selecting an automation message without parsing JSON | one stable wire name: `findings`, `clean`, `insufficient_baseline`, `nothing_in_scope`, `partial` |

JSON is the complete machine-readable result: it always carries every finding and the full per-reason census, including the ghost count. It is not an observation archive — the per-commit series behind the charts is obtained with `examine`. Full Markdown always includes the same coverage counts and reasons in its final Coverage section. Text omits the reasons when findings exist. The condensed summary is lossy by design, so it is the one output that must not be automated against: a check reading it cannot distinguish findings that were capped away from findings that were never made. The outcome file repeats only JSON's top-level `outcome` field; it carries no report details.
