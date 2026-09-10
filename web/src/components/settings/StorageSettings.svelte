<script>
  // What this browser has downloaded for LaTeX and kept, and how to forget
  // it: the compiler and its packages. Offered only for a LaTeX document; the
  // speech models a browser downloads are listed under Dictation. Nothing
  // here touches a document.
  import SettingRow from "./SettingRow.svelte";
  import * as latex from "../../lib/latex.js";
  import { done } from "../../lib/toast.svelte.js";
  import { megabytes } from "./words.js";

  let cacheSize = $state(null);
  $effect(() => {
    latex.resources
      .size()
      .then((bytes) => (cacheSize = bytes))
      .catch(() => {});
  });

  async function clearCache() {
    await latex.resources.clear();
    cacheSize = await latex.resources.size().catch(() => cacheSize);
    done("Downloaded LaTeX files removed");
  }
</script>

<SettingRow id="storage-latex" title="Downloaded LaTeX files"
            description="{cacheSize === null ? 'Measuring…' : cacheSize < 100_000 ? 'Nothing downloaded yet.' : 'Storage used: ' + megabytes(cacheSize) + '.'} The compiler and packages are kept so later PDF builds are faster; removing them frees the space, and what a build needs downloads again. Your documents are kept.">
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={clearCache}>Remove</button>
</SettingRow>
