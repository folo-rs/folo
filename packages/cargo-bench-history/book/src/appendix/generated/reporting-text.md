**Text** — the default output, printed to standard output.

```text
Analyzed project textproc (history mode)
  commit: 9f2c4a1d3b5e708aab12cd34ef5678901234567a
  runs: 128 (4d17b0c93ea2 → 9f2c4a1d3b5e)  in-scope series judged: 46 of 51  regressions: 2

criterion/x86_64-unknown-linux-gnu/a1b2c3d4
  runs: 128  regressions: 2
  filter: --engine criterion --target-triple x86_64-unknown-linux-gnu --machine-key a1b2c3d4

http_parse/case
  +29.57% wall_time (100% confidence)
    regression via change point · 100.4 → 130.1 · @ commit10
 135 ┤           ╭─╮╭─╮╭╮ 
 126 ┤         ╭─╯ ╰╯ ╰╯╰ 
 116 ┤         │          
 106 ┤ ╭╮ ╭─╮╭─╯          
  97 ┼─╯╰─╯ ╰╯           

index_build/case
  +10.40% wall_time (100% confidence)
    regression via drift · 98.5 → 108.7 · @ commit29
 113 ┤                    ╭╮    ╭── 
 109 ┤          ╭╮╭╮╭─╮   ││ ╭╮╭╯   
 105 ┤ ╭╮     ╭─╯╰╯╰╯ ╰─╮╭╯╰─╯╰╯    
 101 ┼╮│╰─────╯         ╰╯          
  97 ┤╰╯
```
