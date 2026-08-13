# Reporting

Everything so far produced findings and a census. This chapter is how they reach you, and how
to read what you get.

## Terms used here

| Term | What it means |
|---|---|
| **finding** | A move that survived detection, every gate, and the group-wide correction. |
| **census** | The report's account of how many series it judged, and why it did not judge the rest. |
| **comparison-base lag** | A branch comparison made against base data from several commits back. |

## Ranking

Findings are ordered by **descending relative move**, then by method, then by a stable
identity tie-break so two runs over the same data produce the same order.

There is deliberately **no severity classification**. Whether a 4% regression in a hot path
matters more than a 40% regression in a rarely-used one is a judgment about your project, and
the tool does not have the information to make it. It sorts by the one quantity it can
measure, and leaves the ranking of importance to you.

Note in particular that findings are **not** sorted by confidence. Every reported finding
already cleared its test, so confidences are uniformly high and would rank almost nothing —
see [Detection](detection.md#confidence-and-what-it-is-not).

## Reading a finding

{{#include generated/reporting-finding-annotated.md}}

The chart is drawn against **topology**, not against the observations. One column per commit,
so a gap is a gap and the trailing stretch after the last observation is visible as one. That
matters: a chart that packed the observations together would hide exactly the sparseness that
[detection cannot see](reconstruction.md#gaps-are-holes-not-zeroes).

Where a series has more commits than the chart has columns, commits are grouped before
plotting. Grouping first is deliberate — the alternative attenuates an isolated observation
surrounded by gaps into nothing.

## The formats

The three canonical formats carry the same information. The **condensed summary** does not: it
is capped, drops the per-set grouping, and is meant for a size-limited destination such as a
pull request comment. Do not automate against it.

**JSON is the machine-readable signal.** It carries each finding self-describingly, plus the
full census — including the reasons the human-readable formats print only on a silent report. It
deliberately omits the per-commit chart series; that is presentation, and
[`examine`](../commands/examine.md) is the way to get the underlying points.

Here is the same analysis in each form:

{{#include generated/reporting-text.md}}

{{#include generated/reporting-json.md}}

## Findings never fail the build

The exit code reflects whether the analysis **ran**. A report full of regressions exits
successfully; a report that could not reach its storage backend does not.

*Why:* a benchmark tool that breaks builds on a measurement gets disabled, and a disabled tool
detects nothing. Findings are advisory by design.

*What this means for automation:* write the JSON report to a file with `--json`. And gate on
**coverage as well as findings** — a run that judged nothing produces an empty findings list,
which a naive check reads as success. See
[Multiplicity and coverage](coverage.md#reading-a-silent-report).

## The coverage line is not decoration

Every report states how many series it judged.

{{#include generated/reporting-census.md}}

This is given the same prominence as the findings on purpose. "No notable changes" means
*nothing changed among the series this run was able to judge*, and without the denominator you
cannot tell that statement apart from "the benchmarks did not run".

Note the asymmetry: the **reasons** a series went unjudged are spelled out only when a report
has no findings to show — otherwise the human-readable formats print the tally alone, on the
grounds that a report with findings has something more urgent to say. The JSON census always
carries the full breakdown, which is another reason to read it rather than the text.

## Comparison-base lag

Branch mode compares your tip against the base's recent commits **within the same discriminant
set**. On a rotating CI pool, the newest base commits may only carry data under a different
machine key — so your runner compares against base data from several commits back.

The report says so, naming how far behind and which of two reasons applies:

{{#include generated/reporting-lag.md}}

It is advisory. It never changes which findings are reported, and never affects the exit code.
What it changes is how much weight you should give a marginal branch finding: a comparison
against a base state ten commits old is a comparison against a base state ten commits old.

## Where output goes

The text report goes to **stdout**. Diagnostics, the effective-selection summary and verbose
notes go to **stderr**.

Warnings are the exception, and it is deliberate: a comparison-base lag notice, or the note that
a dirty run was admitted on the base tip, is rendered **into the report body** rather than onto
stderr. Those qualify the findings printed beside them, so a reader who captured only the report
would otherwise lose the caveat that changes how to read it.

The other formats are written to files rather than to a stream — `--markdown`, `--json` and
`--markdown-summary` each take a path, and `--no-text` suppresses the stdout report when you
want only those.

{{#include generated/reporting-formats.md}}

## What comes next

Nothing — this is the end of the pipeline. If a report left you with a question rather than an
answer, [Insights](insights.md) is the triage chapter.
