# Benchmark history analysis: textproc

1 regression, 1 improvement detected.

Judged 2 of 2 in-scope series.

- Commit: 9f2c4a1d3b5e708aab12cd34ef5678901234567a
- Mode: branch
- Runs analyzed: 128 (4d17b0c93ea2 → 9f2c4a1d3b5e)

## callgrind / x86\_64-unknown-linux-gnu / machine a1b2c3d4

- Runs: 128
- Regressions: 1
- Improvements: 1

**Historical comparison:** None of 5 comparable base commits showed as much out-of-range movement as this branch (2 series compared).

### `textproc/streaming_parser/normalize_unicode_and_validate_nested_records_for_incremental_batches/large_input`
**regression: 126.1 million** instruction count - higher than all 10 current-base observations

current base range: 96.93 million to 103.5 million · excess: +22.6 million / +21.85% · @ 9f2c4a1d3b5e708aab12cd34ef5678901234567a

```text
 126 ┤          ╭ 
 119 ┤          │ 
 111 ┤          │ 
 104 ┼────╮╭────╯ 
  97 ┤    ╰╯     
```

### `textproc/streaming_parser/scan_and_classify_multilingual_text_with_incremental_boundary_checks/large_input`
**improvement: 99.81 million** instruction count - lower than all 10 current-base observations

current base range: 126.1 million to 135.1 million · excess: -26.26 million / -20.83% · @ 9f2c4a1d3b5e708aab12cd34ef5678901234567a

```text
 135 ┤ ╭───╮╭─╮╭╮ 
 126 ┼─╯   ╰╯ ╰╯│ 
 117 ┤          │ 
 109 ┤          │ 
 100 ┤          ╰
```

## Coverage

- State: `full`
- Metric series accounted for: 2
- In scope: 2
- Judged: 2
- Unjudged (including out-of-scope series): 0
