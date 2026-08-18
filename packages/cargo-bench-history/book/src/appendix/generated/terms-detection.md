| Term | What it means |
|---|---|
| **chance level** | How often pure chance alone would produce a pattern at least this strong. |
| **change point** | The commit where a series stops holding one level and starts holding another. |
| **confidence** | How strong the evidence for a finding is, on a scale where higher means chance is a worse explanation. |
| **detector** | The procedure that examines one series for one shape of change: a change point, a drift, or a branch comparison. |
| **drift** | A series that moves steadily in one direction rather than stepping between levels. |
| **level** | The value a series sits at over a stretch of commits, ignoring run-to-run scatter. |
| **median** | The middle value of a sample, which a few extreme measurements cannot drag around. |
| **one-way-trend check** | A test for whether a series mostly moves in one direction, which counts rises against falls rather than fitting a line. |
| **outlier-resistant slope** | A trend line fitted from the middle of all the pairwise slopes, so a few odd measurements cannot tilt it. |
| **prediction interval** | The range a single further measurement is expected to land in, given what the previous ones did. |
| **rank comparison** | A test for whether two regimes differ that weighs each measurement by its rank among all of them rather than by its size, so a few extreme values cannot dominate. Tested two-sided. |
| **regime** | A stretch of commits over which a series holds one level. |
| **scatter** | Between-commit variation in a series when nothing has changed. |
| **split search** | A scan for the single most likely place a series changed level. |
