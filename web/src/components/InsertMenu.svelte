<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import { onDestroy } from "svelte";
  import MenubarMenu from "./MenubarMenu.svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import Icon from "./Icon.svelte";
  import { menubar } from "../lib/menubar.svelte.js";
  import Modal from "./Modal.svelte";
  import { rankEntries, entryLabel } from "../lib/bibliography.js";
  import { INSERT_ACTIONS, insertionAvailability, buildInsertion, gatherInsertEnvironments, gatherInsertTargets, insertEnvironmentFields } from "../lib/insert.js";

  /** @typedef {import("../lib/insert.js").InsertContext} InsertContext */
  /** @typedef {import("../lib/insert.js").InsertOptions} InsertOptions */
  /** @typedef {import("../lib/insert.js").InsertionDraft} InsertionDraft */
  /** @typedef {import("../lib/insert.js").InsertionResult} InsertionResult */
  /** @typedef {import("../lib/insert.js").FinalInsertion} FinalInsertion */
  /** @typedef {{key?: string; [field: string]: unknown}} InsertBibliographyEntry */
  /** @typedef {InsertBibliographyEntry & {zotero_item: string}} ZoteroEntry */
  /** @typedef {InsertContext & {bibliography?: InsertBibliographyEntry[] | {entries: InsertBibliographyEntry[]}; targetId?: string}} InsertMenuContext */
  /** @typedef {{
   * getContext?: () => InsertMenuContext | null;
   * oninsert?: (result: FinalInsertion, context: InsertMenuContext) => unknown;
   * onupload?: (file: File) => Promise<string>;
   * onpreview?: (path: string) => Promise<string | null | undefined>;
   * onpalette?: (id: "symbols" | "math") => void;
   * onfocus?: () => void;
   * oncancel?: (context: InsertMenuContext | null) => void;
   * onzoterosearch?: ((query: string) => Promise<ZoteroEntry[]>) | null;
   * onzoteroimport?: (items: ZoteroEntry[]) => Promise<string[]>;
   * keys?: Record<string, string>;
   * disabled?: boolean;
   * }} InsertMenuProps */
  /** @type {InsertMenuProps} */
  let { getContext, oninsert, onupload, onpreview, onpalette, onfocus, oncancel, onzoterosearch = null, onzoteroimport, keys = {}, disabled = false } = $props();

  /** @type {string | null} */
  let dialog = $state(null);
  let dialogOpen = $state(false);
  /** @type {InsertMenuContext | null} */
  let captured = $state(null);
  /** @type {InsertMenuContext | null} */
  let menuContext = $state(null);

  onDestroy(() => {
    clearTimeout(zoteroTimer);
    if (menuContext) oncancel?.(menuContext);
    if (dialogOpen && captured && captured !== menuContext) oncancel?.(captured);
  });
  /** @type {InsertionDraft} */
  let draft = $state({});
  let query = $state("");
  let busy = $state(false);
  let error = $state("");
  let preview = $state(null);
  let dialogGeneration = 0;
  let source = $state(/** @type {"bibtex" | "zotero"} */ ("bibtex"));
  /** @type {ZoteroEntry[]} */
  let zoteroEntries = $state([]);
  /** @type {ZoteroEntry[]} */
  let zoteroSelected = $state([]);
  let zoteroReady = $state(false);
  let zoteroError = $state("");
  let zoteroSearchCount = 0;
  let zoteroTimer = 0;

  /** @typedef {string | {label: string; ids: string[]}} InsertMenuEntry */
  // The common actions stay at the top. Everything else sits in a sub-menu
  // named after the section it used to head.
  /** @type {InsertMenuEntry[]} */
  const menuTree = [
    "figure", "table", "citation", "cross-reference", "footnote", "link", "heading",
    "inline-math", "display-math",
    { label: "Lists", ids: ["bulleted-list", "numbered-list", "description-list"] },
    { label: "Theorems", ids: ["theorem", "lemma", "proposition", "definition", "proof", "example", "remark"] },
    { label: "Blocks", ids: ["quote", "quotation", "code-block"] },
    { label: "Document", ids: ["abstract", "appendix", "toc", "bibliography", "label"] },
    { label: "Layout", ids: ["page-break", "horizontal-rule", "columns", "custom-environment"] }
  ];

  /** @type {Record<string, InsertionDraft>} */
  const defaults = {
    heading: { text: "", level: "1", numbered: true, label: "" }, abstract: {}, appendix: {}, toc: {},
    figure: { path: "", caption: "", label: "", width: "" },
    table: { rows: 3, columns: 3, header: true, caption: "", label: "", alignment: "default" },
    citation: { keys: [], locator: "", style: "default" }, bibliography: { file: "" },
    "cross-reference": { target: "" }, label: { label: "" },
    "display-math": { numbered: true, label: "" },
    "code-block": { language: "" }, footnote: { text: "" }, link: { text: "", url: "" },
    theorem: { title: "", label: "" }, lemma: { title: "", label: "" }, proposition: { title: "", label: "" },
    definition: { title: "", label: "" }, proof: { title: "", label: "" }, example: { title: "", label: "" }, remark: { title: "", label: "" },
    columns: { columns: 2, gap: "" }, "custom-environment": { environment: "", title: "" }
  };

  const actionById = (id) => INSERT_ACTIONS.find((action) => action.id === id);
  /** @returns {InsertMenuContext} */
  function contextNow() { return menuContext || getContext?.() || { format: "markdown", path: "", text: "", selection: { from: 0, to: 0, text: "" } }; }
  function availability(id, context) { return insertionAvailability?.(id, context) || { enabled: true }; }
  function actionLabel(id) { return actionById(id)?.label || id; }
  function closeDialog() {
    const target = captured;
    clearTimeout(zoteroTimer);
    dialogGeneration += 1;
    busy = false;
    dialogOpen = false;
    dialog = null;
    oncancel?.(target);
  }
  /** @returns {InsertionDraft} */
  function insertionOptions() { return { ...draft, title: draft.title || draft.text, src: draft.path, arguments: draft.argumentsText ? draft.argumentsText.split("\n") : [] }; }
  /** @param {unknown} cause @param {string} fallback @returns {string} */
  function errorMessage(cause, fallback) {
    return cause && typeof cause === "object" && "message" in cause ? String(cause.message) : fallback;
  }
  /** @param {string} id @param {InsertionDraft} options @param {InsertMenuContext} context @returns {InsertionResult} */
  function safeBuild(id, options, context) {
    try { error = ""; return buildInsertion(id, options, context); }
    catch (cause) { const message = errorMessage(cause, "This insertion is not valid yet."); error = message; return { text: "", notes: [message], additionalEdits: [] }; }
  }
  /** @param {string} id */
  function startsDialog(id) {
    const context = contextNow();
    dialogGeneration += 1;
    const generation = dialogGeneration;
    captured = context;
    draft = { ...(defaults[id] || {}) };
    query = "";
    error = "";
    source = "bibtex";
    zoteroEntries = [];
    zoteroSelected = [];
    zoteroReady = false;
    zoteroError = "";
    clearTimeout(zoteroTimer);
    // Skeleton closes the menu after dispatching onSelect. Opening the modal
    // on the next task avoids that close event immediately dismissing it.
    setTimeout(() => {
      if (generation !== dialogGeneration) return;
      dialog = id;
      dialogOpen = true;
      preview = safeBuild(id, insertionOptions(), context);
      if (id === "citation" && onzoterosearch) void openZotero(generation);
    }, 0);
  }
  /** @param {string} id */
  function choose(id) {
    // A palette opener is answered by the reader, which owns the overlays. It
    // is handed on after the menu has closed, so the menu's focus restore does
    // not take the focus back from the dialog.
    if (id.startsWith("palette:")) {
      const paletteId = /** @type {"symbols" | "math"} */ (id.slice("palette:".length));
      setTimeout(() => onpalette?.(paletteId), 0);
      return;
    }
    const context = contextNow();
    const state = availability(id, context);
    if (!state.enabled) return;
    captured = context;
    const needsDialog = actionById(id)?.dialog;
    if (needsDialog) startsDialog(id);
    else {
      const result = safeBuild(id, {}, context);
      if (!result.text || result.notes?.length || result.additionalEdits?.length) {
        draft = {};
        const generation = ++dialogGeneration;
        setTimeout(() => {
          if (generation !== dialogGeneration) return;
          dialog = id;
          dialogOpen = true;
          preview = result;
        }, 0);
      } else void insert(id, {}, context);
    }
  }
  /** @param {string} id @param {InsertionDraft} options @param {InsertMenuContext} context */
  function insert(id, options, context) {
    let result;
    try { result = buildInsertion(id, options, context); }
    catch (cause) { error = errorMessage(cause, "This insertion is not valid yet."); return; }
    try { oninsert?.(result, context); }
    catch (cause) { error = errorMessage(cause, "The insertion target changed."); dialog = id; dialogOpen = true; return; }
    dialogGeneration += 1;
    dialogOpen = false;
    dialog = null;
    setTimeout(() => onfocus?.(), 0);
  }
  async function submit() {
    if (!dialog || !captured || busy) return;
    if (dialog === "citation" && source === "zotero") return submitZotero();
    busy = true;
    try { await insert(dialog, insertionOptions(), captured); }
    catch (cause) { error = errorMessage(cause, "Could not generate this insertion."); }
    finally { busy = false; }
  }
  // Importing writes the items into the project bibliography, so the context
  // captured when the dialog opened is stale afterwards. A fresh one is taken.
  async function submitZotero() {
    const generation = dialogGeneration;
    busy = true;
    try {
      const keys = await onzoteroimport?.(zoteroSelected);
      if (generation !== dialogGeneration) return;
      const stale = captured;
      oncancel?.(stale);
      if (menuContext === stale) menuContext = null;
      captured = getContext?.() ?? null;
      if (!captured) {
        error = "The document changed. Close this dialog and choose the insertion point again.";
        return;
      }
      insert("citation", { ...insertionOptions(), keys }, captured);
    }
    catch (cause) { error = errorMessage(cause, "Could not import the Zotero items."); }
    finally { busy = false; }
  }
  /** @param {Event & {currentTarget: HTMLInputElement}} event */
  async function upload(event) {
    const file = event.currentTarget.files?.[0];
    if (!file || !onupload) return;
    const generation = dialogGeneration;
    busy = true;
    try {
      const path = await onupload(file);
      if (generation === dialogGeneration && dialog === "figure") draft.path = path;
    }
    catch (cause) { error = errorMessage(cause, "Could not upload the image."); }
    finally { if (generation === dialogGeneration && dialog === "figure") busy = false; }
  }
  /** @returns {InsertBibliographyEntry[]} */
  function entries() { return Array.isArray(captured?.bibliography) ? captured.bibliography : captured?.bibliography?.entries || []; }
  const filteredEntries = $derived(rankEntries(entries(), query).slice(0, 30));
  const files = $derived((captured?.files || []).map((file) => typeof file === "string" ? file : file.path).filter((path) => (captured?.format === "latex" ? /\.bib$/i : captured?.format === "typst" ? /\.(bib|ya?ml)$/i : /\.(bib|json|ya?ml)$/i).test(path || "")));
  const selectedImage = $derived((captured?.files || []).find((file) => (typeof file === "string" ? file : file.path) === draft.path));
  const selectedImagePreview = $derived(selectedImage && typeof selectedImage !== "string" ? selectedImage.url : undefined);
  const references = $derived(gatherInsertTargets(captured || {}));
  const environmentFields = $derived(insertEnvironmentFields(draft.environment || "", captured || {}));
  const environments = $derived(gatherInsertEnvironments(captured || {}));
  /** @param {string | null} id */
  const needsText = (id) => ["heading", "link"].includes(id || "");
  const zoteroMode = $derived(dialog === "citation" && source === "zotero");
  // Zotero mode needs a selection. The preview may be empty until the import.
  const insertReady = $derived(zoteroMode ? zoteroSelected.length > 0 : Boolean(preview?.text || preview?.additionalEdits?.length));
  /** @returns {InsertionResult | null} */
  function zoteroPreview() {
    if (!zoteroSelected.length || !captured) return null;
    try { return buildInsertion("citation", { ...insertionOptions(), keys: zoteroSelected.map((item) => item.key) }, captured); }
    catch { return null; }
  }
  /** @returns {string} */
  function zoteroReason() {
    if (!onzoterosearch) return "desktop app only";
    if (zoteroReady) return "";
    return zoteroError ? "not running" : "checking";
  }
  /** @param {string} text */
  async function searchZotero(text) {
    const generation = dialogGeneration;
    const search = ++zoteroSearchCount;
    let found;
    try { found = await onzoterosearch?.(text); }
    catch (cause) {
      if (generation === dialogGeneration && search === zoteroSearchCount) {
        zoteroReady = false;
        zoteroError = errorMessage(cause, "Zotero is not running.");
        zoteroEntries = [];
      }
      return;
    }
    if (generation !== dialogGeneration || search !== zoteroSearchCount) return;
    zoteroReady = true;
    zoteroError = "";
    zoteroEntries = found || [];
  }
  /** @param {number} generation */
  async function openZotero(generation) {
    await searchZotero("");
    if (generation !== dialogGeneration || entries().length || !zoteroReady) return;
    source = "zotero";
    error = "";
  }
  // Typing in Zotero mode searches after a pause, so each keystroke does not
  // ask Zotero a question.
  function scheduleZotero() {
    if (source !== "zotero") return;
    clearTimeout(zoteroTimer);
    const generation = dialogGeneration;
    const text = query;
    zoteroTimer = setTimeout(() => { if (generation === dialogGeneration) void searchZotero(text); }, 200);
  }
  /** @param {string} value */
  function chooseSource(value) {
    source = value === "zotero" ? "zotero" : "bibtex";
    error = "";
    if (source === "zotero") scheduleZotero();
  }
  /** @param {ZoteroEntry} entry @param {boolean} selected */
  function toggleZotero(entry, selected) {
    const others = zoteroSelected.filter((item) => item.zotero_item !== entry.zotero_item);
    zoteroSelected = selected ? [...others, entry] : others;
  }
  $effect(() => {
    if (dialog && captured && dialogOpen) preview = zoteroMode ? zoteroPreview() : safeBuild(dialog, insertionOptions(), captured);
  });
  // Zag closes a sub-menu only through its own trigger, so a sub-menu left
  // open when the bar moves on would stay on screen. Clearing the open group
  // whenever the bar stops showing Insert closes it with its parent.
  /** @type {string | null} */
  let openGroup = $state(null);
  $effect(() => { if (menubar.opened !== "insert") openGroup = null; });
</script>

{#snippet insertItem(id)}
  {@const state = availability(id, menuContext || {})}
  <Menu.Item value={id} class="menuitem" disabled={!state.enabled}>
    <span class="menuitem-label">{actionLabel(id)}{#if actionById(id)?.dialog}…{/if}</span>
    {#if state.reason}<span class="insert-reason">{state.reason}</span>{/if}
  </Menu.Item>
{/snippet}

{#snippet subMenu(entry)}
  {@const allDisabled = entry.ids.every((id) => !availability(id, menuContext || {}).enabled)}
  <!-- The child menu reports its own selections, so it gets the same handler
       as the bar's menu; Zag does not pass a sub-menu's select up to its parent. -->
  <Menu open={openGroup === entry.label}
        onOpenChange={(event) => { if (event.open) openGroup = entry.label; else if (openGroup === entry.label) openGroup = null; }}
        onSelect={(chosen) => choose(chosen.value)}>
    <Menu.TriggerItem value={"submenu:" + entry.label} class="menuitem" disabled={allDisabled}>
      <span class="menuitem-label">{entry.label}</span>
      <span class="menuitem-chevron" aria-hidden="true"><Icon name="chevron-right" size="1rem" /></span>
    </Menu.TriggerItem>
    <ExplorerMenu>
      {#each entry.ids as id}{@render insertItem(id)}{/each}
    </ExplorerMenu>
  </Menu>
{/snippet}

<MenubarMenu id="insert" label="Insert" {disabled}
             onopen={() => {
               // The context from the last opening is released, unless a dialog took it.
               if (menuContext && !(dialogOpen && captured === menuContext)) oncancel?.(menuContext);
               menuContext = getContext?.() || null;
             }}
             onselect={choose}>
  <div class="insert-scroll">
  {#each menuTree as entry}
    {#if typeof entry === "string"}{@render insertItem(entry)}{/if}
  {/each}
  <div class="menu-separator" role="separator"></div>
  <Menu.Item value="palette:symbols" class="menuitem"><span class="menuitem-label">Symbols…</span>{#if keys.symbols}<span class="menuitem-keys">{keys.symbols}</span>{/if}</Menu.Item>
  <Menu.Item value="palette:math" class="menuitem"><span class="menuitem-label">Math…</span>{#if keys.math}<span class="menuitem-keys">{keys.math}</span>{/if}</Menu.Item>
  <div class="menu-separator" role="separator"></div>
  {#each menuTree as entry}
    {#if typeof entry !== "string"}{@render subMenu(entry)}{/if}
  {/each}
  </div>
</MenubarMenu>

<Modal bind:open={dialogOpen} title={dialog ? actionLabel(dialog) : "Insert"} description={captured?.format ? `Generated for ${captured.format}` : null} onclose={closeDialog} wide
  confirm={{
    form: "insert-dialog",
    label: "Insert",
    disabled: busy || Boolean(error) || !insertReady,
    oncancel: closeDialog,
  }}>
  <!-- A form, so Enter in any of these fields inserts, the way Enter in the
       name field of the rename dialog renames. -->
  <form id="insert-dialog" class="flex min-h-0 flex-col gap-3" onsubmit={(event) => { event.preventDefault(); void submit(); }}>
  {#if dialog === "citation"}
    <label class="label">Source
      <select class="select" value={source} onchange={(event) => chooseSource(event.currentTarget.value)}>
        <option value="bibtex" disabled={!entries().length}>{entries().length ? "BibTeX" : "BibTeX (no bibliography)"}</option>
        <option value="zotero" disabled={!zoteroReady}>{zoteroReason() ? `Zotero (${zoteroReason()})` : "Zotero"}</option>
      </select>
    </label>
    <input class="input" type="search" placeholder="Search author, title, year, or key" aria-label="Search bibliography" value={query}
           oninput={(event) => { query = event.currentTarget.value; scheduleZotero(); }} />
    <div class="insert-results" role="group" aria-label={source === "zotero" ? "Zotero items" : "Bibliography entries"}>
      {#if source === "zotero"}
        {#each zoteroEntries as entry}
          <label class="insert-result"><input type="checkbox" value={entry.zotero_item} checked={zoteroSelected.some((item) => item.zotero_item === entry.zotero_item)} onchange={(event) => toggleZotero(entry, event.currentTarget.checked)} /><span><strong>{entry.key}</strong> {entryLabel(entry)}</span></label>
        {:else}<p class="panel-muted">No matching Zotero items.</p>{/each}
      {:else}
        {#each filteredEntries as entry}
          <label class="insert-result"><input type="checkbox" value={entry.key} bind:group={draft.keys} /><span><strong>{entry.key}</strong> {entryLabel(entry)}</span></label>
        {:else}<p class="panel-muted">No matching bibliography entries.</p>{/each}
      {/if}
    </div>
    <label class="label">Citation style <select class="select" bind:value={draft.style}><option value="default">Default</option><option value="narrative">Narrative / textual</option><option value="parenthetical">Parenthetical</option></select></label>
    <label class="label">Page or locator (optional) <input class="input" bind:value={draft.locator} /></label>
  {:else if dialog === "figure"}
    <label class="label">Project image <select class="select" bind:value={draft.path}><option value="">Choose an image…</option>{#if draft.path && !selectedImage}<option value={draft.path}>{draft.path}</option>{/if}{#each (captured?.files || []) as file}{@const path = typeof file === "string" ? file : file.path}{#if /\.(png|jpe?g|gif|svg|pdf|webp)$/i.test(path || "")}<option value={path}>{path}</option>{/if}{/each}</select></label>
    <label class="label">Upload image <input type="file" accept="image/*,.pdf" onchange={upload} /></label>
    {#if draft.path}<p class="panel-muted">{draft.path}</p>{/if}
    {#if draft.path && onpreview}
      {#await onpreview(draft.path) then url}{#if url}<img class="insert-image-preview" src={url} alt="Selected project asset" />{/if}{/await}
    {:else if selectedImagePreview}<img class="insert-image-preview" src={selectedImagePreview} alt="Selected project asset" />{/if}
    <label class="label">Caption <input class="input" bind:value={draft.caption} /></label>
    <label class="label">Label <input class="input" bind:value={draft.label} /></label>
    <label class="label">Width <input class="input" bind:value={draft.width} /></label>
  {:else if dialog === "table"}
    <div class="grid grid-cols-2 gap-3"><label class="label">Rows <input class="input" type="number" min="1" max="100" bind:value={draft.rows} /></label><label class="label">Columns <input class="input" type="number" min="1" max="30" bind:value={draft.columns} /></label></div>
    <label class="flex items-center gap-2"><input type="checkbox" bind:checked={draft.header} /> Header row</label>
    <label class="label">Caption <input class="input" bind:value={draft.caption} /></label><label class="label">Label <input class="input" bind:value={draft.label} /></label>
    <label class="label">Alignment <select class="select" bind:value={draft.alignment}><option value="default">Default</option><option value="left">Left</option><option value="center">Center</option><option value="right">Right</option></select></label>
  {:else if dialog === "bibliography"}
    <label class="label">Bibliography file <select class="select" bind:value={draft.file}><option value="">Choose a file…</option>{#each files as file}<option value={file}>{file}</option>{/each}</select></label>
  {:else if dialog === "cross-reference"}
    <label class="label">Reference <select class="select" bind:value={draft.target}><option value="">Choose a heading, figure, table, or equation…</option>{#each references as ref}{@const value = ref.id || ref.label}<option value={value}>{value}</option>{/each}</select></label>
  {:else if dialog === "display-math"}
    <label class="flex items-center gap-2"><input type="checkbox" bind:checked={draft.numbered} disabled={captured?.format === "markdown"} /> Numbered</label>
    <label class="label">Label <input class="input" bind:value={draft.label} /></label>
  {:else if dialog === "code-block"}
    <label class="label">Language <input class="input" bind:value={draft.language} /></label>
  {:else if dialog === "link"}
    <label class="label">Text <input class="input" bind:value={draft.text} /></label><label class="label">URL <input class="input" type="url" bind:value={draft.url} /></label>
  {:else if ["theorem", "lemma", "proposition", "definition", "proof", "example", "remark"].includes(dialog)}
    <label class="label">Title (optional) <input class="input" bind:value={draft.title} /></label><label class="label">Label <input class="input" bind:value={draft.label} /></label>
  {:else if dialog === "columns"}
    <div class="grid grid-cols-2 gap-3"><label class="label">Columns <input class="input" type="number" min="2" max="6" bind:value={draft.columns} /></label>{#if captured?.format !== "quarto"}<label class="label">Gap (optional) <input class="input" bind:value={draft.gap} /></label>{/if}</div>
  {:else}
    {#if dialog === "custom-environment"}<label class="label">Environment <input class="input" bind:value={draft.environment} />{#if environments.length}<select class="select mt-2" aria-label="Defined environments" onchange={(event) => draft.environment = event.currentTarget.value}><option value="">Choose a defined environment…</option>{#each environments as name}<option value={name}>{name}</option>{/each}</select>{/if}</label>{/if}
    {#if dialog === "custom-environment" && environmentFields.length}<label class="label">Arguments (one per line)<textarea class="textarea" rows={environmentFields.length} bind:value={draft.argumentsText}></textarea><span class="panel-muted">Enter source expressions in this order: {environmentFields.map(field => field.label).join(", ")}.</span></label>{/if}
    {#if needsText(dialog)}<label class="label">Text <input class="input" bind:value={draft.text} /></label>{/if}
    {#if dialog === "heading"}<div class="grid grid-cols-2 gap-3"><label class="label">Level <input class="input" type="number" min="1" max={captured?.format === "latex" ? 5 : 6} bind:value={draft.level} /></label><label class="flex items-center gap-2 mt-6"><input type="checkbox" bind:checked={draft.numbered} /> Numbered</label></div><label class="label">Label <input class="input" bind:value={draft.label} /></label>{/if}
    {#if dialog === "label"}<label class="label">Label <input class="input" bind:value={draft.label} /></label>{/if}
  {/if}
  {#if preview}
    <details class="insert-preview" open><summary>Source preview</summary>{#if preview.text}<pre>{preview.text}</pre>{:else}<p class="panel-muted">No source will be inserted.</p>{/if}{#if preview.notes?.length}<ul>{#each preview.notes as note}<li>{note}</li>{/each}</ul>{/if}{#if preview.additionalEdits?.length}<p class="panel-muted">Additional source edits:</p>{#each preview.additionalEdits as edit}<pre>{edit.path ? `${edit.path}: ` : ""}{edit.insert || "(delete)"}</pre>{/each}{/if}</details>
  {/if}
  {#if error}<p class="insert-error" role="alert">{error}</p>{/if}
  </form>
</Modal>

<style>
  .menuitem-chevron { display: inline-flex; margin-left: auto; padding-left: calc(var(--spacing) * 2); }
  .menu-separator { height: 1px; margin: .2rem .4rem; background: var(--color-divider); }
  .insert-reason { flex: 0 0 auto; margin-left: auto; max-width: 9rem; overflow: hidden; color: var(--color-text-secondary); font-size: .7rem; text-overflow: ellipsis; white-space: nowrap; }
  .insert-results { max-height: 16rem; overflow-y: auto; border: 1px solid var(--color-divider); border-radius: var(--radius-base); }
  .insert-result { display: flex; gap: .6rem; padding: .55rem .7rem; cursor: pointer; }
  .insert-result:hover { background: var(--color-subtle); }
  .insert-error { padding: .7rem 1rem; color: var(--color-error-text); background: var(--color-raised); border: 1px solid var(--color-error-solid); border-radius: var(--radius-base); box-shadow: var(--shadow-xl); }
  .insert-preview { margin-top: .4rem; border-top: 1px solid var(--color-divider); padding-top: .6rem; }
  .insert-preview pre { max-height: 11rem; overflow: auto; margin-top: .5rem; padding: .7rem; background: var(--color-subtle); border-radius: var(--radius-base); font-size: .75rem; white-space: pre-wrap; }
  .insert-preview ul { margin: .5rem 0 0 1rem; color: var(--color-warning-text); font-size: .8rem; }
  .insert-image-preview { max-height: 10rem; max-width: 100%; object-fit: contain; border: 1px solid var(--color-divider); border-radius: var(--radius-base); }
  .insert-scroll { max-height: min(70vh, 34rem); overflow-y: auto; }
</style>
