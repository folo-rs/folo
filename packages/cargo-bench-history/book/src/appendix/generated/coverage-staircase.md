| Rank | Series | Chance level | Bar to clear | Outcome |
|---:|---|---:|---:|---|
| 1 | `parse_headers` | 0.002 | 0.0083 | kept |
| 2 | `tokenize` | 0.011 | 0.0167 | kept |
| 3 | `index_build` | 0.031 | 0.025 | kept |
| 4 | `flush` | 0.033 | 0.0333 | kept |
| 5 | `compress` | 0.045 | 0.0417 | dropped |

The family is the 12 series this run judged, and the bar at rank *k* is *k* / 12 of the 10.0% the correction is willing to have be wrong.
