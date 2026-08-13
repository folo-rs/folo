| Order | Candidate | Rank | Chance level | Bar to clear | Outcome |
|---|---|---:|---:|---:|---|
| correct, then filter | `checksum` (improvement) | 1 | 0.001 | 0.01 | kept |
| correct, then filter | `tokenize` (regression) | 2 | 0.015 | 0.02 | kept |
| filter, then correct | `tokenize` (regression) | 1 | 0.015 | 0.01 | dropped |

Both orders divide by the same 10 judged series. Correcting first leaves the regression at rank 2, where it is kept; filtering the improvement out first moves it to rank 1, where it is dropped. The display then omits the improvement either way.
