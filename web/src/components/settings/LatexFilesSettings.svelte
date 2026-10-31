<script>
  // What this browser has downloaded for LaTeX and kept, and how to forget
  // it: the compiler and its packages. Offered only for a LaTeX document. Nothing
  // here touches a document.
  import SettingRow from "./SettingRow.svelte";
  import * as latex from "../../lib/latex.js";
  import { megabytes } from "./words.js";
  import { m } from "../../lib/paraglide/messages.js";

  let cacheSize = $state(null);
  $effect(() => {
    latex.resources
      .size()
      .then((bytes) => (cacheSize = bytes))
      .catch(() => {});
  });

  // Nothing is said when this finishes: the row's own description is
  // re-measured and goes back to "Nothing downloaded yet.", which is the
  // same news in the place the reader is already looking.
  async function clearCache() {
    await latex.resources.clear();
    cacheSize = await latex.resources.size().catch(() => cacheSize);
  }
</script>

<SettingRow id="render-latex-files" title={m.latexfilessettings_title()}
            description={cacheSize === null ? m.latexfilessettings_measuring() : cacheSize < 100_000 ? m.latexfilessettings_empty() : m.latexfilessettings_used({ size: megabytes(cacheSize) })}>
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={clearCache}>{m.latexfilessettings_remove()}</button>
</SettingRow>
