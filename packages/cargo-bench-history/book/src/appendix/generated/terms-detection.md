| Term | What it means |
|---|---|
| **chance level** | How often pure chance alone would produce a pattern at least this strong. |
| **change point** | The commit where a series stops holding one level and starts holding another. |
| **detector** | The procedure that examines one series for one shape of change: a change point, a drift, or a branch comparison. |
| **drift** | A series that moves steadily in one direction rather than stepping between levels. |
| **level** | The value a series sits at over a stretch of commits, ignoring run-to-run scatter. |
| **median** | The middle value of a sample, which a few extreme measurements cannot drag around. |
| **observed range** | The lowest through highest values actually recorded in the current base regime. |
| **one-way-trend check** | A test for whether a series mostly moves in one direction, which counts rises against falls rather than fitting a line. |
| **outlier-resistant slope** | A trend line fitted from the middle of all the pairwise slopes, so a few odd measurements cannot tilt it. |
| **rank comparison** | A test for whether two regimes differ that weighs each measurement by its rank among all of them rather than by its size, so a few extreme values cannot dominate. Tested two-sided. |
| **reference lane** | The other half of the base observations, which the branch is compared against and which never helps choose that comparison. |
| **regime** | A stretch of commits over which a series holds one level. |
| **scatter** | Between-commit variation in a series when nothing has changed. |
| **selector lane** | The half of the base observations allowed to decide where the current regime begins. |
| **split search** | A scan for the single most likely place a series changed level. |
