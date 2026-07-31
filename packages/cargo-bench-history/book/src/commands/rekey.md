# rekey

`rekey` migrates stored objects onto the current machine-key format, merging history that an
earlier key format split across several partitions back into continuous series.

```console
# Report what would move, and what merging would do to the numbers.
cargo bench-history rekey --local=./bench-history

# Perform the migration.
cargo bench-history rekey --local=./bench-history --apply
```

## Why history needs rekeying

The [machine key](machine-key.md) is version-tagged, so changing which hardware factors it
hashes forks stored history visibly rather than breaking it silently. The fork is honest but
costly: one machine's series splits into short stretches that the analysis cannot compare, and
short stretches produce poor findings. `rekey` closes the fork.

No benchmark is re-run. Every stored run records the full hardware profile behind its key, so
both the retired and the current key are recomputed from the object itself.

## Copy, never move

Each migrated object is written to its new key and the source is left exactly where it is.
Nothing is deleted, so a second `--apply` finds every destination already present and writes
nothing — the command is safe to re-run, and a bad outcome is undone by deleting the copies. A
destination that already exists holding *different* bytes is a real conflict, and the pass
stops rather than guessing.

One key holds one object per commit and kind. Two partitions that merge can both hold the same
commit, and those two objects are genuinely different — their hardware rendered apart, which is
why they were keyed apart — so neither may claim the shared key. Both stay where they are, the
contested destination is reported, and the merged series carries an ordinary gap at that commit.

### Recovering from an aborted `--apply`

Objects are copied one at a time, so a conflict stops the pass partway and every copy made
before it stands. The destination partition is therefore left **truncated**, not empty — and a
truncated partition reads perfectly well, which is what makes the symptom indirect. If it now
holds fewer commits than the analysis needs, `analyze` in branch mode reports
`TooFewBaseCommits` — silence — rather than an error, and a regression on that machine goes
unjudged.

Because nothing is ever deleted, the source partition is intact. Resolve the conflicting
destination object and re-run the migration: the copy resumes and the partition is restored.

## What moves and what does not

An object moves only when its key's machine segment is the retired hash of the hardware the
object itself records. Anything else is reported and left alone:

- **Override-keyed partitions.** `collect --machine-key github` stores under an operator's
  chosen constant, not a hardware hash, and moving it would destroy a deliberate choice. Such
  a run still records the real host's fingerprint, so recomputation alone cannot spot it — the
  key segment is what gives it away.
- **Already-current partitions**, which have nothing to migrate.
- **Runs that record no hardware**, which cannot be placed. Host hardware is recorded only from
  run schema version 4 onwards, so anything written before it carries nothing to recompute a key
  from. These are counted and listed under `missing_provenance` in the report and stay on the key
  they were written under. Read that section of the dry run before applying: those runs keep
  whatever fragmentation they already have, and the remaining history may be too short to judge
  on its own.
- **Runs whose recorded hardware does not render** under the retired key format — a damaged
  record can describe a machine that could not exist, and no retired key can be computed from
  it. Rendering it anyway would produce a key that may belong to some *other* machine, so the
  run stays put. These are counted and listed under `unrenderable_provenance`. A run already
  under the current key is the exception: that key does not read the histogram at all, so it
  is recognized as already migrated like any other.

Blessing sidecars record no hardware of their own, so they follow the mapping their partition's
runs establish. Clean runs, dirty snapshots, and blessings all migrate together.

Before deciding anything, the pass proves its recomputation against the data. A run records the
machine key its own capture computed, so recomputing both the retired and the current hash of
its recorded hardware must reproduce that fingerprint under one of the two: the retired one for
history captured before the format changed, the current one for history captured after. A
fingerprint matching neither means the reimplemented rendering is not the one that keyed this
store, which invalidates every decision the pass would make, so it abandons the whole run rather
than skipping the object. Hardware that does not render under the retired format produces no
retired hash to compare, so it contradicts nothing and only its own object is held back.

## The merge report

Merging two partitions concatenates two sets of measurements into one series. If both really
are the same machine, their levels agree and the merge is invisible. If they systematically
differ, the merge manufactures a step change at the splice point and the next `analyze` reports
it as a regression.

The report therefore covers, for every pair of partitions that would merge and every
`(benchmark, metric)` both hold:

- the **level offset** between the two groups' medians, absolutely and as a percentage; and
- the **interleaving pattern** over commit order — *interleaved* (the groups share stretches of
  history, as one machine rebooting between runs does) or *time-blocked* (disjoint stretches,
  indistinguishable from a real change at the boundary).

It then reduces each pair to a single **systematic offset**: the median of those relative
offsets, taken over the shared `(benchmark, metric)` series whose move is large enough to say
anything about a level. That one number decides whether the migration proceeds. The
interleaving pattern is stated to sharpen an operator's reading of it, and never blocks on its
own.

The pair is judged rather than the benchmark because only the offset the two partitions share
*across* benchmarks manufactures a visible step. Scatter that points in both directions is
measurement noise; after the merge it is simply noise inside one series, which `analyze`'s own
significance gates already absorb. Judging each benchmark separately would refuse every real
merge instead: a store holds hundreds of shared benchmarks, run-to-run variation is a few
percent, and on a family that large *some* benchmark drifts past any tight threshold by chance.
Taking the median rather than the mean also keeps a handful of benchmarks whose code genuinely
changed during one partition's stretch of history from deciding the fate of all the others.

Individual offsets past the tolerance are still listed and flagged in the report, and the pair
line counts them. They mean "this one series may gain a step at the splice", which is worth
an operator's attention — but they do not block. The pair line also states the verdict itself,
so a report read under `--allow-level-shift` names which pairs would otherwise have refused.
Where a pair shares only one offset large enough to read, the median is that offset, so the
gate is exactly as strict as a per-benchmark rule in the one case where there is no evidence
to average over. Where no shared series moved enough to read, the pair reports no distance at
all and cannot block.

A systematic offset at or beyond the tolerance **refuses the migration**, in the dry run and
under `--apply` alike, so the preview can never disagree with what applying would do. The
threshold is half of the relative practical significance floor the detector itself uses, so a
merge under it sits a comfortable margin below what the detector calls practically significant.
The detector's absolute floors are spent choosing which offsets enter the median rather than
gating the median itself. `--allow-level-shift` proceeds anyway, for an operator who has read
the report and accepts the step it will introduce.

`--verbose` adds the per-object reasoning to standard error: the hardware behind each object,
both keys it hashes to, and why each object was migrated or left.

## No facets

Unlike `analyze`, `list runs`, `prune`, and `examine`, `rekey` takes no discriminant facets. It
is a whole-store maintenance pass whose correctness comes from processing every object; a facet
would migrate half a partition and leave history in a state neither key format describes.
