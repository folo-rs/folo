| Engine | Segments | Note |
|---|---|---|
| `callgrind` | package directory, module path, function, and the case id where one is given | Fully qualified |
| `criterion` | group, function, and the parameter where the benchmark is parameterized | Carries no package name, so identical names in different crates share a series |
| `alloc_tracker` | the operation name alone | Carries no package name, so identical operation names in different crates share a series |
| `all_the_time` | the operation name alone | Carries no package name, so identical operation names in different crates share a series |
