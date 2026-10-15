## Benchmark history: textproc

1 regression, 1 improvement detected.
All in-scope series judged.

### `textproc/streaming_parser/normalize_unicode_and_validate_nested_records_for_incremental_batches/large_input`

**Regression · +21.85% excess**  
instruction count · Linux x86\_64 · callgrind  
Current: **126.1 million**

```text
base [=]-------* current
```


### `textproc/streaming_parser/scan_and_classify_multilingual_text_with_incremental_boundary_checks/large_input`

**Improvement · -20.83% excess**  
instruction count · Linux x86\_64 · callgrind  
Current: **99.81 million**

```text
current *------[==] base
```


Analyzed commit: `9f2c4a1d3b5e`.
