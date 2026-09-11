<script>
  // The settings, as a preferences window rather than a form: a navigation
  // list of categories at the left, one category at a time at the right. It
  // opens from the navbar menu, never from the sidebar, and any entry point
  // can open it on a given category -- the status bar's connect link lands
  // on the local app.
  import { tick } from "svelte";
  import Modal from "../Modal.svelte";
  import { offered, search } from "./registry.js";
  import EditorSettings from "./EditorSettings.svelte";
  import DictationSettings from "./DictationSettings.svelte";
  import StorageSettings from "./StorageSettings.svelte";
  import CompilerSettings from "./CompilerSettings.svelte";
  import RenderingSettings from "./RenderingSettings.svelte";
  import LocalAppSettings from "./LocalAppSettings.svelte";

  let {
    open = $bindable(false),
    category = $bindable("editor"),
    sourceFormat = "",
    mayEdit = false,
    // The editor.
    keys = "default",
    onkeys,
    // The LaTeX project.
    latexSettings = { engine: "auto" },
    onlatexsettings,
    // The Quarto project and the local app it renders on.
    bindingId = "",
    main = "",
    onbindingid,
    options,
    viewing = null,
    onapplyoptions,
  } = $props();

  const context = $derived({ format: sourceFormat, mayEdit });
  const available = $derived(offered(context));
  // The category shown: the one asked for, or the first offered when that is
  // not (the document changed format, or this browser lost the right to edit).
  const shown = $derived(available.find((each) => each.id === category) || available[0]);

  let query = $state("");
  const found = $derived(search(query, context));
  // The navigation: every offered category or, while searching, only those
  // with a matching row.
  const nav = $derived(found ? found.map((match) => match.category) : available);
  const entriesOf = (id) => found?.find((match) => match.category.id === id)?.entries || [];

  let body = $state(null);
  async function go(id, entry = "") {
    category = id;
    if (!entry) return;
    await tick();
    body?.querySelector(`#${entry}`)?.scrollIntoView({ block: "start", behavior: "smooth" });
  }
</script>

<Modal bind:open title="Settings" full>
  <div class="settings">
    <nav class="settings-nav" aria-label="Settings categories">
      <input class="input input-sm settings-search" type="search" placeholder="Search settings" aria-label="Search settings" bind:value={query} />
      {#each nav as item (item.id)}
        <button type="button" class="settings-nav-item" class:current={shown?.id === item.id}
                aria-current={shown?.id === item.id ? "page" : undefined} onclick={() => go(item.id)}>{item.says}</button>
        {#each entriesOf(item.id) as entry (entry.id)}
          <button type="button" class="settings-nav-entry" onclick={() => go(item.id, entry.id)}>{entry.says}</button>
        {/each}
      {/each}
      {#if found && !nav.length}<p class="settings-nav-empty">Nothing matches.</p>{/if}
    </nav>

    <div class="settings-body" bind:this={body}>
      {#if shown}
        <header class="settings-head">
          <h3 class="settings-category">{shown.says}</h3>
          {#if shown.note}<span class="settings-scope">{shown.note}</span>{/if}
        </header>
        {#if shown.id === "editor"}
          <EditorSettings {keys} {onkeys} />
        {:else if shown.id === "dictation"}
          <DictationSettings />
        {:else if shown.id === "storage"}
          <StorageSettings />
        {:else if shown.id === "compiler"}
          <CompilerSettings {latexSettings} {onlatexsettings} />
        {:else if shown.id === "rendering"}
          <RenderingSettings {options} {viewing} {onapplyoptions} />
        {:else if shown.id === "local"}
          <LocalAppSettings {main} {sourceFormat} {bindingId} {onbindingid} />
        {/if}
      {/if}
    </div>
  </div>
</Modal>
