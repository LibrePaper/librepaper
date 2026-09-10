<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import Modal from "./Modal.svelte";
  import { rankEntries, entryLabel } from "../lib/bibliography.js";
  import { INSERT_ACTIONS, insertionAvailability, buildInsertion } from "../lib/insert.js";

  let { getContext, oninsert, onupload, disabled = false } = $props();

  let dialog = $state(null);
  let dialogOpen = $state(false);
  let captured = $state(null);
  let menuContext = $state(null);
  let draft = $state({});
  let query = $state("");
  let busy = $state(false);
  let error = $state("");

  const groups = [
    ["Structure", ["heading", "abstract", "appendix", "toc"]],
    ["Figures and tables", ["figure", "table"]],
    ["References", ["citation", "bibliography", "cross-reference", "label"]],
    ["Math", ["inline-math", "display-math", "aligned-math", "gather-math", "cases", "matrix"]],
    ["Lists", ["bullet-list", "numbered-list", "description-list"]],
    ["Text blocks", ["quote", "quotation", "code-block", "footnote", "link"]],
    ["Scholarly", ["theorem", "lemma", "proposition", "definition", "proof", "example", "remark"]],
    ["Layout", ["page-break", "horizontal-rule", "columns"]],
    ["Advanced", ["custom-environment"]]
  ];

  const defaults = {
    heading: { text: "", level: "1", numbered: true }, abstract: {}, appendix: {}, toc: {},
    figure: { path: "", caption: "", label: "", width: "" },
    table: { rows: 3, columns: 3, header: true, caption: "", label: "", alignment: "default" },
    citation: { keys: [], locator: "", style: "default" }, bibliography: { file: "" },
    "cross-reference": { target: "" }, label: { label: "" },
    "display-math": { numbered: true, label: "" }, "aligned-math": { numbered: true, label: "" }, "gather-math": { numbered: true, label: "" },
    cases: { rows: 2 }, matrix: { rows: 3, columns: 3, brackets: "parentheses" },
    "code-block": { language: "" }, footnote: { text: "" }, link: { text: "", url: "" },
    theorem: { title: "", label: "" }, lemma: { title: "", label: "" }, proposition: { title: "", label: "" },
    definition: { title: "", label: "" }, proof: { title: "", label: "" }, example: { title: "", label: "" }, remark: { title: "", label: "" },
    columns: { columns: 2, gap: "" }, "custom-environment": { environment: "", title: "" }
  };

  const actionById = (id) => INSERT_ACTIONS?.find((action) => action.id === id) || { id, label: id };
  function contextNow() { return menuContext || getContext?.() || { format: "markdown", path: "", text: "", selection: { from: 0, to: 0, text: "" } }; }
  function availability(id, context) { return insertionAvailability?.(id, context) || { enabled: true }; }
  function actionLabel(id) { return actionById(id).label || id; }
  function startsDialog(id) {
    const context = contextNow();
    captured = context;
    draft = { ...(defaults[id] || {}) };
    query = "";
    dialog = id;
    dialogOpen = true;
    error = "";
    preview = buildInsertion(id, { ...draft, src: draft.path }, captured);
  }
  function choose(id) {
    const context = contextNow();
    const state = availability(id, context);
    if (!state.enabled) return;
    captured = context;
    if (actionById(id).dialog || defaults[id] || id === "citation" || id === "figure" || id === "table") startsDialog(id);
    else void insert(id, {}, captured);
  }
  function insert(id, options, context) {
    const result = buildInsertion(id, options, context);
    if (result?.error) { error = result.error; return; }
    oninsert?.(result, context);
    dialogOpen = false;
    dialog = null;
  }
  async function submit() {
    if (!dialog || busy) return;
    busy = true;
    try { await insert(dialog, { ...draft, citationKeys: draft.keys, src: draft.path }, captured); }
    catch (cause) { error = cause?.message || "Could not generate this insertion."; }
    finally { busy = false; }
  }
  async function upload(event) {
    const file = event.currentTarget.files?.[0];
    if (!file || !onupload) return;
    busy = true;
    try { draft.path = await onupload(file); }
    catch (cause) { error = cause?.message || "Could not upload the image."; }
    finally { busy = false; }
  }
  function entries() { return captured?.bibliography?.entries || captured?.bibliography || []; }
  const filteredEntries = $derived(rankEntries(entries(), query).slice(0, 30));
  const files = $derived((captured?.files || []).map((file) => typeof file === "string" ? file : file.path).filter((path) => /\.bib$/i.test(path || "")));
  const references = $derived(captured?.references || captured?.labels || []);
  const needsText = (id) => ["heading", "footnote", "link", "code-block", "custom-environment"].includes(id);
  $effect(() => {
    if (dialog && captured && dialogOpen) preview = buildInsertion(dialog, { ...draft, src: draft.path }, captured);
  });
</script>

<Menu onOpenChange={(event) => { if (event.open) menuContext = getContext?.() || null; }} onSelect={(event) => choose(event.value)}>
  <Menu.Trigger class="menubar-item" disabled={disabled}>Insert</Menu.Trigger>
  <ExplorerMenu>
    {#each groups as [group, ids], groupIndex}
      {#if groupIndex}<div class="menu-separator" role="separator"></div>{/if}
      <div class="insert-group" role="group" aria-label={group}>{group}</div>
      {#each ids as id}
        {@const state = availability(id, menuContext || {})}
        <Menu.Item value={id} class="menuitem" disabled={!state.enabled}>
          <span>{actionLabel(id)}</span>{#if actionById(id).dialog || defaults[id]}<span class="insert-ellipsis">…</span>{/if}
          {#if state.reason}<span class="insert-reason">{state.reason}</span>{/if}
        </Menu.Item>
      {/each}
    {/each}
  </ExplorerMenu>
</Menu>

{#snippet footer()}
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => { dialogOpen = false; dialog = null; }}>Cancel</button>
  <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={busy} onclick={submit}>Insert</button>
{/snippet}

<Modal bind:open={dialogOpen} title={dialog ? actionLabel(dialog) : "Insert"} description={captured?.format ? `Generated for ${captured.format}` : null} {footer} onclose={() => { dialogOpen = false; dialog = null; }} wide>
  {#if dialog === "citation"}
    <input class="input" type="search" placeholder="Search author, title, year, or key" aria-label="Search bibliography" bind:value={query} autofocus />
    <div class="insert-results" role="listbox" aria-label="Bibliography entries">
      {#each filteredEntries as entry}
        <label class="insert-result"><input type="checkbox" value={entry.key} bind:group={draft.keys} /><span><strong>{entry.key}</strong> {entryLabel(entry)}</span></label>
      {:else}<p class="panel-muted">No matching bibliography entries.</p>{/each}
    </div>
    <label class="label">Page or locator <input class="input" placeholder="optional" bind:value={draft.locator} /></label>
  {:else if dialog === "figure"}
    <label class="label">Project image <select class="select" bind:value={draft.path}><option value="">Choose an image…</option>{#each (captured?.files || []) as file}{@const path = typeof file === "string" ? file : file.path}{#if /\.(png|jpe?g|gif|svg|pdf|webp)$/i.test(path || "")}<option value={path}>{path}</option>{/if}{/each}</select></label>
    <label class="label">Upload image <input type="file" accept="image/*,.pdf" onchange={upload} /></label>
    {#if draft.path}<p class="panel-muted">{draft.path}</p>{/if}
    <label class="label">Caption <input class="input" bind:value={draft.caption} /></label>
    <label class="label">Label <input class="input" placeholder="fig:example" bind:value={draft.label} /></label>
    <label class="label">Width <input class="input" placeholder="e.g. 80% or 0.8\linewidth" bind:value={draft.width} /></label>
  {:else if dialog === "table"}
    <div class="grid grid-cols-2 gap-3"><label class="label">Rows <input class="input" type="number" min="1" max="100" bind:value={draft.rows} /></label><label class="label">Columns <input class="input" type="number" min="1" max="30" bind:value={draft.columns} /></label></div>
    <label class="flex items-center gap-2"><input type="checkbox" bind:checked={draft.header} /> Header row</label>
    <label class="label">Caption <input class="input" bind:value={draft.caption} /></label><label class="label">Label <input class="input" bind:value={draft.label} /></label>
    <label class="label">Alignment <select class="select" bind:value={draft.alignment}><option value="default">Default</option><option value="left">Left</option><option value="center">Center</option><option value="right">Right</option></select></label>
  {:else if dialog === "bibliography"}
    <label class="label">Bibliography file <select class="select" bind:value={draft.file}><option value="">Choose a file…</option>{#each files as file}<option value={file}>{file}</option>{/each}</select></label>
  {:else if dialog === "cross-reference"}
    <label class="label">Reference <select class="select" bind:value={draft.target}><option value="">Choose a heading, figure, table, or equation…</option>{#each references as ref}{@const value = typeof ref === "string" ? ref : (ref.label || ref.id || ref.key)}<option value={value}>{value}</option>{/each}</select></label>
  {:else if dialog === "matrix"}
    <div class="grid grid-cols-2 gap-3"><label class="label">Rows <input class="input" type="number" min="1" max="20" bind:value={draft.rows} /></label><label class="label">Columns <input class="input" type="number" min="1" max="20" bind:value={draft.columns} /></label></div>
    <label class="label">Brackets <select class="select" bind:value={draft.brackets}><option value="parentheses">( )</option><option value="brackets">[ ]</option><option value="braces">{ }</option><option value="none">None</option></select></label>
  {:else if dialog === "cases"}
    <label class="label">Rows <input class="input" type="number" min="1" max="20" bind:value={draft.rows} /></label>
  {:else if dialog === "code-block"}
    <label class="label">Language <input class="input" placeholder="e.g. r, python, bash" bind:value={draft.language} /></label>
  {:else if dialog === "link"}
    <label class="label">Text <input class="input" bind:value={draft.text} /></label><label class="label">URL <input class="input" type="url" placeholder="https://" bind:value={draft.url} /></label>
  {:else if ["theorem", "lemma", "proposition", "definition", "proof", "example", "remark"].includes(dialog)}
    <label class="label">Title <input class="input" placeholder="optional" bind:value={draft.title} /></label><label class="label">Label <input class="input" placeholder="thm:example" bind:value={draft.label} /></label>
  {:else if dialog === "columns"}
    <div class="grid grid-cols-2 gap-3"><label class="label">Columns <input class="input" type="number" min="2" max="6" bind:value={draft.columns} /></label><label class="label">Gap <input class="input" placeholder="optional" bind:value={draft.gap} /></label></div>
  {:else}
    {#if dialog === "custom-environment"}<label class="label">Environment <input class="input" placeholder="e.g. important" bind:value={draft.environment} /></label>{/if}
    {#if needsText(dialog)}<label class="label">Text <input class="input" bind:value={draft.text} /></label>{/if}
    {#if dialog === "heading"}<div class="grid grid-cols-2 gap-3"><label class="label">Level <input class="input" type="number" min="1" max="6" bind:value={draft.level} /></label><label class="flex items-center gap-2 mt-6"><input type="checkbox" bind:checked={draft.numbered} /> Numbered</label></div>{/if}
  {/if}
  {#if preview?.text}
    <details class="insert-preview" open><summary>Source preview</summary><pre>{preview.text}</pre>{#if preview.notes?.length}<ul>{#each preview.notes as note}<li>{note}</li>{/each}</ul>{/if}</details>
  {/if}
</Modal>

{#if error}<p class="insert-error" role="alert">{error}</p>{/if}

<style>
  .insert-group { padding: .35rem .65rem .2rem; color: var(--color-surface-600-400); font-size: .7rem; font-weight: 700; text-transform: uppercase; letter-spacing: .05em; }
  .menu-separator { height: 1px; margin: .2rem .4rem; background: var(--color-divider); }
  .insert-ellipsis { margin-left: auto; color: var(--color-surface-500-500); }
  .insert-reason { display: block; margin-left: auto; max-width: 9rem; overflow: hidden; color: var(--color-surface-500-500); font-size: .7rem; text-overflow: ellipsis; white-space: nowrap; }
  .insert-results { max-height: 16rem; overflow-y: auto; border: 1px solid var(--color-divider); border-radius: var(--radius-base); }
  .insert-result { display: flex; gap: .6rem; padding: .55rem .7rem; cursor: pointer; }
  .insert-result:hover { background: var(--color-surface-100-900); }
  .insert-error { position: fixed; right: 1rem; bottom: 1rem; z-index: 60; max-width: 26rem; padding: .7rem 1rem; color: var(--color-error-700-300); background: var(--color-surface-50-950); border: 1px solid var(--color-error-500-500); border-radius: var(--radius-base); box-shadow: var(--shadow-xl); }
  .insert-preview { margin-top: .4rem; border-top: 1px solid var(--color-divider); padding-top: .6rem; }
  .insert-preview pre { max-height: 11rem; overflow: auto; margin-top: .5rem; padding: .7rem; background: var(--color-surface-100-900); border-radius: var(--radius-base); font-size: .75rem; white-space: pre-wrap; }
  .insert-preview ul { margin: .5rem 0 0 1rem; color: var(--color-warning-700-300); font-size: .8rem; }
</style>
