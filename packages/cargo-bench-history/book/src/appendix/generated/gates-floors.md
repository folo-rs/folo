| Metric | Absolute floor | Why |
|---|---|---|
| `wall_time` | 1 ns | A timing engine fits a slope across a run's iterations and resolves far below a clock tick, so this is a judgement about what is worth acting on rather than a resolution limit. |
| `processor_time` | 1 ns | A timing engine fits a slope across a run's iterations and resolves far below a clock tick, so this is a judgement about what is worth acting on rather than a resolution limit. |
| `instruction_count` | 5 instructions | Code layout shifts these counts by a few units between builds of identical source, so a handful of them says nothing about what the code costs. |
| `conditional_branches` | 5 conditional branches | Code layout shifts these counts by a few units between builds of identical source, so a handful of them says nothing about what the code costs. |
| `indirect_branches` | 5 indirect branches | Code layout shifts these counts by a few units between builds of identical source, so a handful of them says nothing about what the code costs. |
| `allocated_bytes` | 1 byte | A fraction of a byte or of an allocation cannot happen; the floor rejects only the sub-unit moves that amortizing across a run's iterations manufactures. |
| `allocation_count` | 1 allocation | A fraction of a byte or of an allocation cannot happen; the floor rejects only the sub-unit moves that amortizing across a run's iterations manufactures. |
