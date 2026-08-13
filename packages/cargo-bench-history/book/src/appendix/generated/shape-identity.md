| Engine | Segments | Note |
|---|---|---|
| `criterion` | group, function, and the parameter where the benchmark is parameterized | Carries no package name, so identical names in different crates share a series |
| `callgrind` | package directory, module path, function, and the case id where one is given | Fully qualified |
| `alloc_tracker` | the operation name | A single segment |
| `all_the_time` | the operation name | A single segment |
