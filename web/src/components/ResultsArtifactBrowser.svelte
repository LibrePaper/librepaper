<script>
  import Modal from "./Modal.svelte";
  let { artifact } = $props();
  let opened = $state(false);
  let interactive = $state(false);
  let selected = $state("");
  const path = $derived(artifact.pages.includes(selected) ? selected : artifact.pages[0]);
  const html = $derived(opened ? artifact.page(path, interactive) : "");
</script>

<button class="btn btn-sm preset-tonal-surface" onclick={() => { opened = true; interactive = false; }}>Browse saved pages / widgets</button>
<Modal bind:open={opened} title="Saved artifact browser" wide>
  <label>Page <select class="select" bind:value={selected}>{#each artifact.pages as name}<option value={name}>{name}</option>{/each}</select></label>
  <label><input type="checkbox" bind:checked={interactive} /> Enable document scripts</label>
  <p class="text-sm">Saved render. Scripts run in an isolated frame with network fetches blocked. Widgets requiring a server remain static.</p>
  {#if opened}<iframe title="Saved Quarto page" sandbox="allow-scripts" referrerpolicy="no-referrer" srcdoc={html} class="w-full h-96 border-0"></iframe>{/if}
  {#snippet footer()}<button class="btn btn-sm preset-tonal-surface" onclick={() => opened = false}>Close</button>{/snippet}
</Modal>
