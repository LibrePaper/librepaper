<script>
  // What this browser has downloaded for LaTeX and kept, and how to forget
  // it: the compiler and its packages. Offered only for a LaTeX document. Nothing
  // here touches a document.
  import SettingRow from "./SettingRow.svelte";
  import * as latex from "../../lib/latex.js";
  import { megabytes } from "./words.js";

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

<SettingRow id="storage-latex" title="Downloaded LaTeX files"
            description="{cacheSize === null ? 'Measuring…' : cacheSize < 100_000 ? 'Nothing downloaded yet.' : 'Storage used: ' + megabytes(cacheSize) + '.'} The compiler and packages are kept so later PDF builds are faster; removing them frees the space, and what a build needs downloads again. Your documents are kept.">
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={clearCache}>Remove</button>
</SettingRow>
