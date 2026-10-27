<script>
  import { basename } from "../../lib/file-manager.js";

  // Counts come from the source by `lib/word-count.js`, which skips markup,
  // math and comments. This component only presents the numbers it is given.
  let { count = null } = $props();

  // "1 word" or "4,321 words"; the caller prints the number.
  function plural(n, noun) {
    return n === 1 ? noun : `${noun}s`;
  }

  // Where the counted document lives: its root, and how many files it includes.
  const scope = $derived.by(() => {
    if (!count) return "";
    const included = count.document.files.length - 1;
    const name = basename(count.document.files[0]?.path || "");
    return included > 0 ? `in ${name} and ${included} included ${plural(included, "file")}` : `in ${name}`;
  });
</script>

{#if count}
  <div class="word-count">
    <p class="word-total"><strong>{count.document.words.toLocaleString()}</strong> {plural(count.document.words, "word")}</p>
    <p class="word-scope">{scope}</p>
    {#if count.document.files.length > 1}
      <ul class="word-files">
        {#each count.document.files as file (file.path)}
          <li title={file.path}>
            <span>{basename(file.path)}</span>
            <span class="word-file-count">{file.words.toLocaleString()}</span>
          </li>
        {/each}
      </ul>
    {/if}
    {#if count.selection !== null}
      <p class="word-selection">{count.selection.toLocaleString()} {plural(count.selection, "word")} selected</p>
    {/if}
  </div>
{/if}

<style>
  .word-count { display: flex; flex-direction: column; gap: calc(var(--spacing) * 1.5); font-size: var(--text-sm); }
  .word-count p { margin: 0; }
  .word-scope { color: var(--color-text-secondary); }
  .word-files {
    display: flex;
    flex-direction: column;
    gap: calc(var(--spacing) * .5);
    margin: 0;
    padding: calc(var(--spacing) * 2) 0 0;
    border-top: 1px solid var(--color-divider);
    list-style: none;
  }
  .word-files li { display: flex; justify-content: space-between; gap: var(--spacing); }
  .word-file-count { font-variant-numeric: tabular-nums; }
</style>
