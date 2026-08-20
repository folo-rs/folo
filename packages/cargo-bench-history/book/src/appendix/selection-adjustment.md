# Selection adjustment

The change-point detector finds where a series stepped by **trying every interior split and
keeping the most convincing one**. That search is the source of a subtle dishonesty, and this
chapter is how the tool corrects for it before any gate or family-wide correction sees the number.

> **Two unrelated things are called "selection" in this book.** The
> [Selection](selection.md) chapter is about *which stored runs are eligible* for an analysis.
> This chapter is about a statistical effect — the detector *selecting* the most striking split
> out of many — and has nothing to do with run eligibility. It is also distinct from the
> [family-wide correction](coverage.md), which accounts for how many *series* were tested; this
> one accounts for how many *splits within one series* were tried.

## In plain terms: searching finds a pattern in noise

Give the detector a completely flat, unchanging series and it will still try dozens of places to
cut it in two, then report the single most lopsided cut it found. With enough places to look,
one of them looks surprising by chance alone. The p-value the detector reports at that chosen
split — this book calls it the **tainted p** — is therefore optimistic: it answers "how
surprising is *this* split?" while ignoring that the split was the winner of a search.

Read literally, an uncorrected tainted p would let far more unchanged series through the
significance gate than its threshold claims — and mostly the recent, short-regime series the tool
most wants to judge well. So history mode replaces the tainted p with an **honest,
selection-adjusted chance level** before anything downstream consumes it. Branch mode makes one
predetermined comparison, searches no split, and needs no adjustment.

## The honest number is a property of the series length

Under the null hypothesis — no real step — a series' values matter only through their order, and
every order is equally likely. So the honest adjusted chance level is a **mathematical constant
of the series length `n`**: the distribution, over all `n!` orderings, of the tainted p the
detector's whole procedure would report. It is not fitted to your data or ours; it is derived
once and looked up.

That procedure, evaluated on one ordering, is exactly what production does: locate the split with
Pettitt over every interior position, reject it unless the shorter side reaches the minimum
regime length, otherwise score it with the two-sided Mann–Whitney p-value at that split. The
committed table is the tabulated distribution of that procedure's output, one row per length.

## Two bounds, and the tool keeps the smaller

The adjusted chance level is the smaller of two independently valid **upper bounds** on the honest
number. Each is at least the truth, so their minimum is at least the truth too — still an honest
p-value, never more significant than it should be.

- **A calibrated table.** Per series length, it records where the honest chance level sits for any
  tainted p. It is tight where it matters most — right around the decision boundary, where a
  finding is won or lost.
- **A union bound.** If the detector could have reported any of `k` splits, then reporting the most
  extreme of them can inflate the apparent significance by at most a factor of `k`, so
  `k × tainted p` is a valid honest level. (`k` is the count of interior splits that leave a full
  regime on each side.) This is the plain "multiply by how many chances you took" argument.

**Why keep both?** The table is built by sampling (below), so it cannot resolve chance levels
below its sampling margin — roughly one in a thousand — no matter how strong the evidence: every
row bottoms out at that floor. Handed straight to the [family-wide correction](coverage.md), that
floor would silently bury obvious regressions in any suite of more than a couple of dozen series.
The union bound has no floor — it scales straight down with the tainted p — so it carries the deep
tail the table cannot reach, while the table stays tighter than the union bound near the boundary.
Each covers the other's weakness.

## Then both detectors are accounted for

History runs two detectors — the change-point detector and the drift detector — and reports
whichever fits the data better. That is a second selection, on top of the split search, and it
inflates the false-alarm rate about twofold. So the adjusted chance level of **each** detector is
doubled before the significance gate. It is a deliberately blunt, defensible factor rather than a
fitted one.

The result of all this is the honest per-series chance level that the significance gate and the
family-wide correction both consume. You can see the step in a [gate ladder](gates.md#reading-a-gate-ladder):
it corrects the level and never declines a candidate on its own.

## Exact significance for short series

The tainted p itself is computed **exactly** wherever a split is lopsided enough to count exactly —
whenever the *smaller* of the two sides is small, however long the whole series is. Rather than
lean on the normal approximation, the tool enumerates the permutation tail directly: it counts the
orderings at least as extreme as the observed split and divides. The calibration models this same
exact procedure, so the honest number and the tainted number describe the same computation.

The choice is made split by split, and it matters most when one side is short and its values repeat
— routine for integer instruction counts. There the normal approximation can report a p-value far
**smaller** than the exact permutation count supports, overstating how significant a repeated-value
split is; enumerating the short side reins that back to the honest value. Only the near-balanced
splits of a long series exceed what a double-precision integer can count exactly, and there the
approximation is kept — safely, because a series that long already has an honest smallest p-value
below the reporting floor, so no verdict turns on it.

## Why you can trust the numbers

The whole point of a committed table is that a reader can check it rather than take it on faith.

- **Short rows are exact.** For the shortest series every one of the `n!` orderings is enumerated,
  so those rows carry no sampling error at all.
- **Longer rows are sampled, and honest about it.** Above the exact range the distribution is
  estimated by Monte Carlo, and each such row carries a Dvoretzky–Kiefer–Wolfowitz margin: a
  stated, family-wide confidence that the committed critical values err toward reporting *fewer*
  findings, not more.
- **It is reproducible bit-for-bit.** The derivation is fully deterministic — the same fixed seeds
  produce the same table every time. Anyone can re-derive it from scratch and confirm it matches
  the committed copy, and a freshness test fails the build if the two ever drift apart.

The derivation and every constant behind it live in the `cargo-bench-history-calibration` generator
crate; this chapter is the account of *what* it produces and *why*, not the code.

## What bounds all of this

The correction is defined for every length the pipeline can present, because analysis caps each
series at its most recent points (a low four-figure ceiling) and drops anything older. That ceiling
is what guarantees no series is ever longer than the table's last row. It sits comfortably above the
range the tool is built for — dozens to a few hundred points per series — where the honest
correction is most needed and most accurate.

## What this stage hands on

A single honest chance level per series, in place of the tainted one. It flows unchanged into the
[significance gate](gates.md) and then the [family-wide correction](coverage.md), so every later
stage reasons about a number that already tells the truth about the search that produced it.
