| Order | Candidate | Rank | Chance level | Threshold at rank | Outcome |
|---|---|---:|---:|---:|---|
| correct, then filter | `checksum` (improvement) | 1 | 0.001 | 0.01 | kept |
| correct, then filter | `tokenize` (regression) | 2 | 0.015 | 0.02 | kept |
| filter, then correct | `tokenize` (regression) | 1 | 0.015 | 0.01 | dropped |

Both orders divide by the same 10 judged series. The tool filters, then corrects: the regression is at rank 1, where it is dropped. Correcting both directions first would leave it at rank 2, where it is kept, before the display would hide the improvement.
