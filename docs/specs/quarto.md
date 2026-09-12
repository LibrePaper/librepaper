# SPEC: Quarto

Work nobody has asked for yet, and limitations kept on purpose. Each item
says which.

Source preview through the paired local app, capture, binding, sync, and
isolation all shipped. What did not:

## Left open

- The draft passes `:::` divs through verbatim. Callouts, columns, tabsets,
  margin content, conditional content (`when-format`, profiles), and
  shortcodes other than `{{< include >}}` render as their source. The
  parser in `web/src/lib/engines/quarto.js` already tracks fences and
  spans; the missing part is a rendering for each family.
- The draft applies none of `echo`, `include`, `output`, `eval` or
  `code-fold`: every cell shows verbatim. That was chosen over guessing.
- Quarto preview is HTML only. `QuartoWatch::kind()` in
  `crates/librepaper/src/local/preview/quarto.rs` is hard-wired to HTML;
  a `--to pdf` preview would use the existing PDF reader, as Calepin's does.
- Website and book projects are collected and tested on the capture side
  but the live preview accepts document scope only.
- `librepaper local doctor` reports Quarto but never the Calepin tool.
- No declared Quarto version support matrix, and the real-toolchain R and
  Python tests are all `#[ignore]`d with no job that runs them. The
  end-to-end gate (pair, live repaint, disconnect, reconnect, R then Python)
  has no automated run.
- `web/tools/quarto-benchmark.mjs` prints draft timings but asserts no
  budget and has no large fixture.
- Confinement (`bwrap`, `sandbox-exec`, none) is detected and reported, but
  there is no model for granting a render access to data or the network.

## References

- [Quarto draft parser](../../web/src/lib/engines/quarto.js) -- tracks fences and spans; the div renderings would go here.
- [Quarto preview](../../crates/librepaper/src/local/preview/quarto.rs) -- `QuartoWatch::kind()`, hard-wired to HTML.
- [Quarto benchmark](../../web/tools/quarto-benchmark.mjs) -- timings without a budget.
- [Local CLI](../../crates/librepaper/src/local/cli.rs) -- `doctor`, which does not report Calepin.
