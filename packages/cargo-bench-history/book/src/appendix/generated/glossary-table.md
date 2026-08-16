| Term | What it means | Also called | Introduced in |
|---|---|---|---|
| agreement share | The fraction of before-and-after pairs that agree the level moved in the same direction. | probability of superiority | [Noise gates](gates.md) |
| blessing | A recorded decision to treat a change as accepted, so history stops reporting it. |  | [Reconstruction](reconstruction.md) |
| census | The report's account of how many series it judged, and why it did not judge the rest. |  | [Reporting](reporting.md) |
| chance level | How often pure chance alone would produce a pattern at least this strong. | p-value | [Detection](detection.md) |
| change point | The commit where a series stops holding one level and starts holding another. |  | [Detection](detection.md) |
| comparison-base lag | A branch comparison made against base data from several commits back. |  | [Reporting](reporting.md) |
| confidence | How strong the evidence for a finding is, on a scale where higher means chance is a worse explanation. |  | [Detection](detection.md) |
| confidence interval | A range the benchmark engine reports alongside a measurement to say how precisely it pinned it down. |  | [Noise gates](gates.md) |
| dirty run | A measurement taken with uncommitted changes in the working tree. |  | [Collection](collection.md) |
| discriminant set | The engine, target triple, and machine key a run was measured with, which together decide what it may be compared against. |  | [Shape of the data](shape.md) |
| drift | A series that moves steadily in one direction rather than stepping between levels. | monotonic trend | [Detection](detection.md) |
| false-discovery family | Every series that carried enough data to be tested, which is the group a finding has to stand out from. |  | [Multiplicity and coverage](coverage.md) |
| finding | A move that survived detection, every gate, and the group-wide correction. |  | [Reporting](reporting.md) |
| ghost | A benchmark that history remembers but the analyzed commit no longer measures. |  | [Reconstruction](reconstruction.md) |
| group-wide correction | A stricter bar applied when many things are tested at once, so that only a small share of what is reported is expected to be wrong. | Benjamini-Hochberg false discovery rate control | [Multiplicity and coverage](coverage.md) |
| harvest | Reading whatever output the benchmark engines left behind after a run. |  | [Collection](collection.md) |
| level | The value a series sits at over a stretch of commits, ignoring run-to-run wobble. |  | [Detection](detection.md) |
| machine key | A fingerprint of the host hardware, used to keep incomparable results apart. |  | [Collection](collection.md) |
| median | The middle value of a sample, which a few extreme measurements cannot drag around. |  | [Detection](detection.md) |
| merge base | The newest commit a branch and its base still share. |  | [Selection](selection.md) |
| one-way-trend check | A test for whether a series mostly moves in one direction, which counts rises against falls rather than fitting a line. | Mann-Kendall test | [Detection](detection.md) |
| outlier-resistant slope | A trend line fitted from the middle of all the pairwise slopes, so a few odd measurements cannot tilt it. | Theil-Sen estimator | [Detection](detection.md) |
| partition | One discriminant set's slice of the store. |  | [Collection](collection.md) |
| prediction interval | The range a single further measurement is expected to land in, given what the previous ones did. | Student's t prediction interval | [Detection](detection.md) |
| quantum | The smallest step a metric can actually take, such as one whole instruction. |  | [Noise gates](gates.md) |
| rank comparison | Rank-based evidence that the before and after regimes differ, tested two-sided. | Mann-Whitney U test | [Detection](detection.md) |
| regime | A stretch of commits over which a series holds one level. |  | [Detection](detection.md) |
| scatter | Between-commit variation in a series when nothing has changed. |  | [Detection](detection.md) |
| series | One metric of one benchmark, tracked across commits. |  | [Reconstruction](reconstruction.md) |
| split search | A scan for the single most likely place a series changed level. | Pettitt test | [Detection](detection.md) |
| typical residual | How far an ordinary point sits from the level or line fitted to the series. | median absolute residual | [Noise gates](gates.md) |
