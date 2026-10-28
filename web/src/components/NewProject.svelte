<script>
  import { untrack } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  // The New project page: three ways to begin, one name. Blank starts empty,
  // From Template starts from a catalog, Upload Files from what the person
  // already has. The desktop app adds a fourth, From a Link, which opens a
  // project someone shared rather than creating one. Nothing here creates a
  // project until Create project is pressed; switching tabs and picking a
  // template only change what the button would make.
  //
  // All state lives in this file, not in the tabs, because a tab that is not
  // showing is not in the page and must still remember what was typed in it.
  import Modal from "./Modal.svelte";
  import IconButton from "./IconButton.svelte";
  import * as localBridge from "../lib/desktop/client.js";
  import { SHELL_HEADERS, upload } from "../lib/api.js";
  import { FORMATS, formatNamed, fillTemplate } from "../lib/starter.js";
  import { BUILT_IN, templateFiles } from "../lib/templates.js";
  import { droppedFiles } from "../lib/file-manager.js";
  import { expandArchives } from "../lib/project-upload.js";
  import { unzip } from "../lib/zip.js";
  import {
    catalog, browse, formatSummary, pickFormat, uniqueName,
    stage, settle, unstage, wrapperOf, finalPaths,
    tree, suggestName, bytes,
  } from "../lib/new-project.js";

  let {
    me = {},
    rules = {},
    templateRows = [],
    takenTitles = [],
    tab: initialTab = "blank",
    initialFormat = "",
    initialTemplate = "",
    ontab = undefined,
    oncancel = undefined,
    ondeletetemplate = undefined,
  } = $props();

  const FORMAT_KEY = "librepaper.template-format";
  const TABS = [
    { id: "blank", label: "Blank" },
    { id: "template", label: "From Template" },
    { id: "upload", label: "Upload Files" },
    ...(localBridge.desktop() ? [{ id: "link", label: "From a Link" }] : []),
  ];

  // A link from this server, share links included, opens here with its key.
  // Links are pasted rather than passed by the system, so a key never leaves the page.
  let linkInput = $state("");
  let linkError = $state("");
  function openLink(event) {
    event.preventDefault();
    linkError = "";
    let url;
    try { url = new URL(linkInput.trim(), location.href); } catch { linkError = "That is not a link."; return; }
    if (url.origin !== location.origin) { linkError = `Paste a link from ${location.host}.`; return; }
    location.assign(url.href);
  }

  function rememberedFormat() {
    try {
      const saved = localStorage.getItem(FORMAT_KEY) ?? "";
      return FORMATS.some((each) => each.id === saved) ? saved : "";
    } catch {
      return "";
    }
  }

  function remember(id) {
    try {
      localStorage.setItem(FORMAT_KEY, id);
    } catch {
      // Not remembering is a convenience lost, not an error.
    }
  }

  let tab = $state(untrack(() => (TABS.some((each) => each.id === initialTab) ? initialTab : "blank")));
  let name = $state("");
  // Once the person has typed in the name, no suggestion replaces it.
  let nameTouched = $state(false);
  let error = $state("");
  let busy = $state(false);

  function changeTab(id) {
    if (id === tab) return;
    tab = id;
    error = "";
    ontab?.(id);
  }

  /* ---------------------------------------------------------- template */

  let query = $state("");
  let formatFilter = $state("");
  let sort = $state("name-asc");
  let selectedId = $state(untrack(() => initialTemplate));
  // The format picked for each template, so going back to one finds it as left.
  // A template opened with a format (from the starter row) starts in that format.
  let chosenFormats = $state(untrack(() => (
    initialTemplate && FORMATS.some((each) => each.id === initialFormat) ? { [initialTemplate]: initialFormat } : {}
  )));

  const templates = $derived(catalog(BUILT_IN, templateRows));
  const shown = $derived(browse(templates, { query, format: formatFilter, sort }));
  const selected = $derived(shown.find((each) => each.id === selectedId) ?? null);
  const templateFormat = $derived(selected
    ? pickFormat(selected, { chosen: chosenFormats[selected.id] ?? "", filter: formatFilter, preferred: rememberedFormat() })
    : "");

  // A template that a search or filter hides is no longer the choice.
  $effect(() => {
    if (selectedId && !shown.some((each) => each.id === selectedId)) selectedId = "";
  });

  function clearFilters() {
    query = "";
    formatFilter = "";
  }

  const countText = $derived(`${shown.length} ${shown.length === 1 ? "template" : "templates"}`);

  /* ------------------------------------------------------------ details */

  let detail = $state(null);
  let detailOpen = $state(false);
  let detailPaths = $state([]);
  const detailFormat = $derived(detail
    ? pickFormat(detail, { chosen: chosenFormats[detail.id] ?? "", filter: formatFilter, preferred: rememberedFormat() })
    : "");

  function showDetail(template) {
    detail = template;
    detailOpen = true;
  }

  // The file list is read only when the dialog is open, so the catalog itself
  // never loads a template's files.
  $effect(() => {
    const template = detailOpen ? detail : null;
    const format = detailFormat;
    detailPaths = [];
    if (!template || template.custom || !format) return;
    let stale = false;
    templateFiles(template.id, format)
      .then((files) => { if (!stale) detailPaths = files.map((file) => file.path).sort(); })
      .catch(() => {});
    return () => { stale = true; };
  });

  /* ------------------------------------------------------------- upload */

  // Entries are { path, file, source }; see lib/new-project.js.
  let staged = $state.raw([]);
  let conflicts = $state.raw([]);
  let unsupported = $state.raw([]);
  let problems = $state.raw([]);
  let notes = $state.raw([]);
  let reading = $state(false);
  let strip = $state(true);
  let progress = $state({ loaded: 0, total: 0 });
  let filesInput = $state(null);
  let folderInput = $state(null);
  const canBrowseFolder = "webkitdirectory" in HTMLInputElement.prototype;

  const archives = $derived([...new Set(staged.map((entry) => entry.source).filter(Boolean))]);
  const wrapper = $derived(wrapperOf(staged));
  const paths = $derived(finalPaths(staged, strip && wrapper !== ""));
  const hasDocument = $derived(paths.some((path) => (rules?.extensions || []).some((ext) => path.toLowerCase().endsWith(ext.toLowerCase()))));
  const totalSize = $derived(staged.reduce((sum, entry) => sum + entry.file.size, 0));
  const stagedTree = $derived(tree(staged.map((entry, index) => ({ path: paths[index], size: entry.file.size }))));
  const stagedPath = $derived(new Map(paths.map((path, index) => [path, staged[index].path])));
  // A folder in the tree is named by its shown path; the staged one carries
  // the wrapper back when it was stripped.
  const stagedFolder = (path) => (strip && wrapper ? `${wrapper}/${path}` : path);

  function resetStaging() {
    staged = [];
    conflicts = [];
    unsupported = [];
    problems = [];
    notes = [];
    strip = true;
  }

  // Zip files become the files inside them, labelled with the archive's name;
  // everything else is staged as it came.
  async function addItems(items) {
    if (!items.length) return;
    reading = true;
    error = "";
    try {
      const incoming = [];
      const fresh = { notes: [], problems: [] };
      for (const item of items) {
        if (!/\.zip$/i.test(item.path)) {
          incoming.push({ path: item.path, file: item.file, source: "" });
          continue;
        }
        const source = item.path.split("/").at(-1);
        try {
          const opened = await expandArchives([item], rules, unzip);
          for (const entry of opened.files) incoming.push({ path: entry.path, file: entry.file, source });
          if (opened.skipped?.length) {
            const count = opened.skipped.length;
            fresh.notes.push(`${count} ${count === 1 ? "file" : "files"} left out of ${source}: rendered output and files LibrePaper does not keep.`);
          }
          for (const root of opened.roots ?? []) {
            if (root) fresh.notes.push(`The archive's top folder ${root} was removed.`);
          }
        } catch (failure) {
          fresh.problems.push(`${source}: ${failure?.message || "that archive could not be read."}`);
        }
      }
      const result = stage(staged, incoming, rules);
      staged = result.staged;
      conflicts = [...conflicts, ...result.conflicts];
      unsupported = [...new Set([...unsupported, ...result.skipped])];
      problems = [...problems, ...fresh.problems, ...result.errors];
      notes = [...notes, ...fresh.notes];
    } catch (failure) {
      problems = [...problems, failure?.message || "Those files could not be added."];
    } finally {
      reading = false;
    }
  }

  function picked(event) {
    const input = event.currentTarget;
    const items = [...input.files].map((file) => ({ file, path: file.webkitRelativePath || file.name }));
    input.value = "";
    void addItems(items);
  }

  function settleConflicts(choice) {
    staged = settle(staged, conflicts, choice);
    conflicts = [];
  }

  /* ------------------------------------------------------ the name field */

  const suggestion = $derived.by(() => {
    const fresh = uniqueName(takenTitles);
    if (tab === "template") return selected ? uniqueName(takenTitles, selected.name) : fresh;
    if (tab === "upload") return suggestName(staged, { archives }) || fresh;
    return fresh;
  });
  $effect(() => {
    if (!nameTouched) name = suggestion;
  });

  /* ------------------------------------------------------------- create */

  function refuse(message, focusId = "") {
    error = message;
    if (focusId) document.getElementById(focusId)?.focus();
  }

  async function finish(response) {
    if (!response.ok) {
      error = (await response.json().catch(() => ({}))).error || "The project could not be created.";
      return false;
    }
    const doc = await response.json();
    // Straight into the project: the point of making one is to work in it.
    location.href = doc.url;
    return true;
  }

  // The same multipart publish a directory uses. The server names the format
  // from the opening file's extension.
  async function publishBuiltIn(id, format, title) {
    const files = fillTemplate(await templateFiles(id, format), format, { title, author: me.name });
    const form = new FormData();
    for (const file of files) form.append("file", new Blob([file.text], { type: "text/plain" }), file.path);
    form.append("title", title);
    return upload(form);
  }

  async function create(event) {
    event.preventDefault();
    if (busy) return;
    const title = name.trim();
    if (!title) return refuse("Give the project a name.", "new-project-name");
    if (tab === "template" && !selected) return refuse("Choose a template.");
    if (tab === "upload" && !hasDocument) return refuse("Add at least one document (.tex, .typ, .qmd, .md or .html).");
    busy = true;
    error = "";
    let navigated = false;
    try {
      let response;
      if (tab === "blank") {
        const form = new FormData();
        form.append("title", title);
        response = await upload(form);
      } else if (tab === "template") {
        if (selected.custom) {
          response = await fetch(`/api/documents/${selected.id}/fork`, {
            method: "POST",
            headers: { ...SHELL_HEADERS, "Content-Type": "application/json" },
            body: JSON.stringify({ title }),
          });
        } else {
          remember(templateFormat);
          response = await publishBuiltIn(selected.id, templateFormat, title);
        }
      } else {
        const form = new FormData();
        staged.forEach((entry, index) => form.append("file", entry.file, paths[index]));
        form.append("title", title);
        progress = { loaded: 0, total: 0 };
        response = await upload(form, { onprogress: (loaded, total) => (progress = { loaded, total }) });
      }
      navigated = await finish(response);
    } catch (failure) {
      error = failure?.message || "The project could not be created.";
    } finally {
      if (!navigated) busy = false;
    }
  }

  /* ----------------------------------------------- dropping onto the page */

  let overlay = $state(false);

  // Files dragged in from the desktop, anywhere on the page. Other drags (text,
  // links, things from within the page) are not ours and are ignored.
  $effect(() => {
    const withFiles = (event) => Array.from(event.dataTransfer?.types ?? []).includes("Files");
    // Enter and leave fire for every element the pointer crosses; counting
    // them keeps the overlay from flickering as it moves over children.
    let depth = 0;
    const enter = (event) => {
      if (!withFiles(event)) return;
      event.preventDefault();
      depth += 1;
      overlay = true;
    };
    const over = (event) => {
      if (!withFiles(event)) return;
      event.preventDefault();
      overlay = true;
    };
    const leave = (event) => {
      if (!withFiles(event)) return;
      depth = Math.max(0, depth - 1);
      if (depth === 0) overlay = false;
    };
    const drop = (event) => {
      if (!withFiles(event)) return;
      // Always, so the browser never navigates to the dropped file.
      event.preventDefault();
      depth = 0;
      overlay = false;
      // The browser releases the drag data once this handler returns, so the
      // entries are captured before anything is awaited.
      const captured = droppedFiles(event.dataTransfer);
      changeTab("upload");
      captured.then((result) => addItems(result.files)).catch(() => {
        problems = [...problems, "The dropped files could not be read."];
      });
    };
    window.addEventListener("dragenter", enter);
    window.addEventListener("dragover", over);
    window.addEventListener("dragleave", leave);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragenter", enter);
      window.removeEventListener("dragover", over);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("drop", drop);
    };
  });
</script>

<section id="new-project" class="new-project">
  <header class="np-head">
    <h1>New project</h1>
    <button type="button" class="btn btn-sm new-project-cancel lp-control-tonal-brand" onclick={() => oncancel?.()}>Cancel</button>
  </header>

  <Tabs value={tab} onValueChange={(event) => changeTab(event.value)}
        ids={{ trigger: (one) => `new-project-tab-${one}`, content: (one) => `new-project-pane-${one}` }}>
    <Tabs.List class="np-tabs" aria-label="How to start the project">
      {#each TABS as one (one.id)}
        <Tabs.Trigger value={one.id}>{one.label}</Tabs.Trigger>
      {/each}
    </Tabs.List>

    <Tabs.Content value="blank">
      {#if tab === "blank"}
        <form id="new-project-form" class="np-form" onsubmit={create}>
          {@render nameField()}
          {@render problem()}
          <div class="np-actions">
            <button type="submit" class="btn btn-sm new-project-create lp-control-brand" disabled={busy}>{busy ? "Creating…" : "Create project"}</button>
          </div>
        </form>
      {/if}
    </Tabs.Content>

    <Tabs.Content value="template">
      {#if tab === "template"}
        {@render templatePicker()}
      {/if}
    </Tabs.Content>

    <Tabs.Content value="upload">
      {#if tab === "upload"}
        {@render uploader()}
      {/if}
    </Tabs.Content>

    <Tabs.Content value="link">
      {#if tab === "link"}
        <form class="np-form open-link" onsubmit={openLink}>
          <p class="np-muted">Open a project someone shared with you. Paste its link from this server.</p>
          <input class="input input-sm" type="url" placeholder="Paste a LibrePaper link, such as a share link" aria-label="LibrePaper link" bind:value={linkInput} />
          <div class="np-actions">
            <button type="submit" class="btn btn-sm lp-control-brand">Open</button>
          </div>
          {#if linkError}<p class="open-link-error" role="alert">{linkError}</p>{/if}
        </form>
      {/if}
    </Tabs.Content>
  </Tabs>
</section>

{#if overlay}
  <div class="drop-overlay" aria-hidden="true"><p>Drop to add to a new project</p></div>
{/if}

<Modal bind:open={detailOpen} title={detail?.name ?? "Template"} wide>
  {#snippet children()}
    {#if detail}
      <div class="np-detail">
        {#if detail.description}<p>{detail.description}</p>{/if}
        <p class="np-muted">Formats: {detail.formats.map((id) => formatNamed(id).name).join(", ")}</p>
        {#if detail.custom}
          <div class="np-detail-actions">
            <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => (location.href = detail.url)}>Open template</button>
            <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => { const gone = detail; detailOpen = false; ondeletetemplate?.(gone); }}>Move to trash</button>
          </div>
        {:else if detailPaths.length}
          <p class="np-muted">Files in {formatNamed(detailFormat).name}:</p>
          <ul class="np-detail-files">
            {#each detailPaths as path}<li>{path}</li>{/each}
          </ul>
        {/if}
      </div>
    {/if}
  {/snippet}
</Modal>

{#snippet nameField()}
  <label class="np-field np-grow" for="new-project-name">
    <span class="np-label">Project name</span>
    <input id="new-project-name" class="input" autocomplete="off" bind:value={name}
           oninput={() => (nameTouched = true)} />
  </label>
{/snippet}

{#snippet problem()}
  {#if error}<p class="new-project-error lp-text-error text-sm" role="alert">{error}</p>{/if}
{/snippet}

{#snippet templatePicker()}
  <div class="np-toolbar">
    <label class="np-field np-search" for="template-search">
      <span class="np-label">Search templates</span>
      <input id="template-search" class="input" type="search" placeholder="Search templates…" bind:value={query} />
    </label>
    <label class="np-field" for="template-format">
      <span class="np-label">Format</span>
      <select id="template-format" class="select" bind:value={formatFilter}>
        <option value="">All formats</option>
        {#each FORMATS as format}<option value={format.id}>{format.name}</option>{/each}
      </select>
    </label>
    <label class="np-field" for="template-sort">
      <span class="np-label">Sort</span>
      <select id="template-sort" class="select" bind:value={sort}>
        <option value="name-asc">Name A to Z</option>
        <option value="name-desc">Name Z to A</option>
      </select>
    </label>
    <p class="template-count np-muted" aria-live="polite">{countText}</p>
  </div>

  {#if shown.length}
    <table class="template-table">
      <thead>
        <tr>
          <th class="col-pick"><span class="sr-only">Choose</span></th>
          <th class="col-name">Name</th>
          <th class="col-say">Description</th>
          <th class="col-formats">Formats</th>
          <th class="col-info"><span class="sr-only">Details</span></th>
        </tr>
      </thead>
      <tbody>
        {#each shown as template (template.id)}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -- the radio in the row is the keyboard path; the row is the larger target -->
          <tr data-template={template.id} class:selected={selectedId === template.id} onclick={() => (selectedId = template.id)}>
            <td class="col-pick">
              <input type="radio" name="template" value={template.id} aria-label="Choose {template.name}"
                     checked={selectedId === template.id} onchange={() => (selectedId = template.id)} />
            </td>
            <td class="col-name">{template.name}</td>
            <td class="col-say" title={template.description}>{template.description || (template.custom ? "One of your templates" : "")}</td>
            <td class="col-formats">{formatSummary(template.formats)}</td>
            <td class="col-info">
              <button type="button" class="template-info" aria-label="Details for {template.name}"
                      onclick={(event) => { event.stopPropagation(); showDetail(template); }}>
                <span aria-hidden="true">i</span>
              </button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <div class="np-empty">
      <p class="template-empty">No templates match.</p>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={clearFilters}>Clear search and filters</button>
    </div>
  {/if}

  {#if selected}
    <form id="new-project-form" class="np-create" onsubmit={create}>
      <p class="np-muted np-selected">Selected: {selected.name}</p>
      {@render nameField()}
      {#if !selected.custom}
        <label class="np-field" for="new-project-format">
          <span class="np-label">Format</span>
          <select id="new-project-format" class="select" value={templateFormat}
                  onchange={(event) => (chosenFormats = { ...chosenFormats, [selected.id]: event.currentTarget.value })}>
            {#each selected.formats as id}<option value={id}>{formatNamed(id).name}</option>{/each}
          </select>
        </label>
      {/if}
      <button type="submit" class="btn btn-sm new-project-create lp-control-brand" disabled={busy}>{busy ? "Creating…" : "Create project"}</button>
      {@render problem()}
    </form>
  {:else}
    {@render problem()}
  {/if}
{/snippet}

{#snippet browseButtons()}
  <button type="button" class="btn btn-sm upload-browse-files lp-control-tonal-brand" onclick={() => filesInput?.click()}>Browse files</button>
  {#if canBrowseFolder}
    <button type="button" class="btn btn-sm upload-browse-folder lp-control-tonal-brand" onclick={() => folderInput?.click()}>Browse folder</button>
  {/if}
{/snippet}

{#snippet treeNode(node)}
  {#if node.children}
    <div class="upload-folder">
      <div class="upload-folder-name">
        {node.name} <span class="np-muted">({node.count} {node.count === 1 ? "file" : "files"})</span>
        <IconButton icon="x" label="Remove the folder {node.path}" tone="plain" size="btn-icon-sm"
                    onclick={() => { staged = unstage(staged, stagedFolder(node.path)); }} />
      </div>
      <ul>
        {#each node.children as child (child.path)}<li>{@render treeNode(child)}</li>{/each}
      </ul>
    </div>
  {:else}
    <div class="upload-file">
      <span class="upload-file-name">{node.name}</span>
      <span class="np-muted">{bytes(node.size)}</span>
      <IconButton icon="x" label="Remove {node.path}" tone="plain" size="btn-icon-sm"
                  onclick={() => (staged = unstage(staged, stagedPath.get(node.path) ?? node.path))} />
    </div>
  {/if}
{/snippet}

{#snippet uploader()}
  <input id="upload-files" type="file" multiple hidden onchange={picked} bind:this={filesInput} />
  <input id="upload-folder" type="file" webkitdirectory hidden onchange={picked} bind:this={folderInput} />

  {#if !staged.length}
    <div class="upload-drop">
      <p>Drop files, folders, or a ZIP archive here</p>
      <div class="np-row">{@render browseButtons()}</div>
    </div>
  {:else}
    <div class="upload-more">
      <span>Add more files</span>
      <div class="np-row">{@render browseButtons()}</div>
    </div>
  {/if}

  {#if reading}<p class="np-muted" role="status">Reading files…</p>{/if}

  {#if conflicts.length}
    <div class="np-conflicts" role="group" aria-label="Files with the same path">
      <p>{conflicts.length} {conflicts.length === 1 ? "file is" : "files are"} already staged with the same path: {conflicts.map((one) => one.path).join(", ")}</p>
      <div class="np-row">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => settleConflicts("replace")}>Replace</button>
        <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => settleConflicts("keep-both")}>Keep both</button>
        <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => settleConflicts("skip")}>Skip</button>
      </div>
    </div>
  {/if}
  {#if unsupported.length}
    <p class="np-muted">Not supported, so left out: {unsupported.join(", ")}</p>
  {/if}
  {#each notes as note}<p class="np-muted">{note}</p>{/each}
  {#each problems as one}<p class="new-project-error lp-text-error text-sm" role="alert">{one}</p>{/each}

  {#if staged.length}
    <form id="new-project-form" class="np-preview" onsubmit={create}>
      <div class="np-form">
        {@render nameField()}
      </div>

      <p class="np-muted">
        {staged.length} {staged.length === 1 ? "file" : "files"}, {bytes(totalSize)}
        {#if archives.length}from {archives.join(", ")}{/if}
      </p>

      {#if wrapper}
        <label class="np-check">
          <input id="upload-strip" type="checkbox" bind:checked={strip} />
          <span>Put the files at the top level instead of inside "{wrapper}"</span>
        </label>
      {/if}

      <div class="np-treehead">
        <span class="np-label">Files</span>
        <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={resetStaging}>Clear all</button>
      </div>
      <ul class="upload-tree">
        {#each stagedTree as node (node.path)}<li>{@render treeNode(node)}</li>{/each}
      </ul>

      {#if !hasDocument}
        <p class="new-project-error lp-text-error text-sm" role="alert">Add at least one document (.tex, .typ, .qmd, .md or .html).</p>
      {/if}
      {@render problem()}

      {#if busy}
        <div class="np-progress">
          <progress class="upload-progress" value={progress.loaded} max={progress.total || 1}></progress>
          <p class="np-muted" aria-live="polite">Uploading {bytes(progress.loaded)} of {bytes(progress.total || totalSize)}</p>
        </div>
      {/if}

      <div class="np-actions">
        <button type="submit" class="btn btn-sm new-project-create lp-control-brand" disabled={busy || !hasDocument}>{busy ? "Creating…" : "Create project"}</button>
      </div>
    </form>
  {/if}
{/snippet}

<style>
  .new-project { max-width: 1440px; margin-inline: auto; padding: calc(var(--spacing) * 8); display: flex; flex-direction: column; gap: calc(var(--spacing) * 4); }
  .np-head { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 3); }
  .np-head h1 { font-size: var(--text-2xl); font-weight: 600; margin: 0; }
  .np-muted { color: var(--color-text-secondary); font-size: var(--text-sm); margin: 0; }

  /* An underline in the brand colour and nothing else, as in the panels. */
  :global(.np-tabs) { display: flex; gap: calc(var(--spacing) * 6); border-bottom: 1px solid var(--color-pane-edge); margin-bottom: calc(var(--spacing) * 5); }
  :global(.np-tabs > [role="tab"]) { padding: calc(var(--spacing) * 2) calc(var(--spacing)); cursor: pointer; color: var(--color-text-secondary); border-bottom: 2px solid transparent; margin-bottom: -1px; }
  :global(.np-tabs > [role="tab"][data-selected]) { color: var(--color-link); border-color: var(--color-link); }
  :global(.np-tabs > [role="tab"]:focus-visible) { outline: 2px solid var(--color-focus); outline-offset: 2px; }

  .np-form { display: flex; flex-wrap: wrap; align-items: flex-end; gap: calc(var(--spacing) * 4); max-width: 48rem; }
  .np-field { display: flex; flex-direction: column; gap: calc(var(--spacing)); min-width: 0; }
  .np-grow { flex: 1 1 16rem; }
  .np-label { font-size: var(--text-sm); color: var(--color-text-secondary); }
  .np-actions { display: flex; justify-content: flex-end; }
  .np-preview .np-actions { width: 100%; }
  .open-link p { flex-basis: 100%; }
  .open-link input { flex: 1; min-width: 12rem; max-width: 32rem; }
  .open-link-error { color: var(--color-error-text); font-size: var(--text-sm); }
  .np-form :global(.new-project-error) { flex-basis: 100%; order: 1; margin: 0; }
  .np-row { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 2); }

  .np-toolbar { display: flex; flex-wrap: wrap; align-items: flex-end; gap: calc(var(--spacing) * 3); margin-bottom: calc(var(--spacing) * 3); }
  .np-search { flex: 1 1 18rem; max-width: 28rem; }
  .template-count { margin-left: auto; align-self: center; }

  .template-table { width: 100%; table-layout: fixed; border-collapse: collapse; }
  .template-table th { text-align: left; font-size: var(--text-xs); text-transform: uppercase; letter-spacing: .04em; color: var(--color-text-secondary); font-weight: 600; padding: calc(var(--spacing) * 2); }
  .template-table td { height: 3rem; padding: 0 calc(var(--spacing) * 2); border-top: 1px solid var(--color-pane-edge); vertical-align: middle; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .template-table tbody tr { cursor: pointer; }
  .template-table tbody tr:hover { background: var(--color-row-hover); }
  .template-table tbody tr.selected { background: var(--color-row-selected); }
  .col-pick { width: 2.5rem; }
  .col-info { width: 3rem; text-align: right; }
  .col-name { width: 24%; font-weight: 500; }
  .col-say { width: 50%; color: var(--color-text-secondary); }
  .col-formats { font-size: var(--text-sm); color: var(--color-text-secondary); }
  .col-pick input { margin: 0; cursor: pointer; accent-color: var(--color-focus); }
  .col-pick input:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  .template-info { display: inline-grid; place-items: center; width: 1.5rem; height: 1.5rem; border: 1px solid var(--color-border); border-radius: 50%; background: transparent; color: var(--color-text-secondary); font-size: var(--text-xs); font-style: italic; font-weight: 600; cursor: pointer; }
  .template-info:hover { background: var(--color-row-hover); }
  .template-info:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }

  .np-empty { display: flex; flex-direction: column; align-items: flex-start; gap: calc(var(--spacing) * 3); padding-block: calc(var(--spacing) * 6); }
  .template-empty { margin: 0; color: var(--color-text-secondary); }

  /* Stays in reach on a long catalog without covering any row: it sits in the
     flow after the table and only sticks once the table runs past the window. */
  .np-create { position: sticky; bottom: 0; display: flex; flex-wrap: wrap; align-items: flex-end; gap: calc(var(--spacing) * 4); padding-block: calc(var(--spacing) * 3); background: var(--color-shell); border-top: 1px solid var(--color-pane-edge); }
  .np-selected { align-self: center; }
  .np-create :global(.new-project-create) { margin-left: auto; }
  .np-create :global(.new-project-error) { flex-basis: 100%; margin: 0; }

  .np-detail { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .np-detail p { margin: 0; }
  .np-detail-actions { display: flex; gap: calc(var(--spacing) * 2); }
  .np-detail-files { margin: 0; padding-left: calc(var(--spacing) * 5); font-size: var(--text-sm); list-style: disc; }

  .upload-drop { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: calc(var(--spacing) * 4); min-height: 14rem; padding: calc(var(--spacing) * 6); border: 2px dashed var(--color-border); border-radius: 12px; text-align: center; }
  .upload-drop p { margin: 0; font-size: var(--text-lg); }
  .upload-more { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 3); padding: calc(var(--spacing) * 3) calc(var(--spacing) * 4); border: 1px dashed var(--color-border); border-radius: 8px; }
  .np-conflicts { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 3); }
  .np-conflicts p { margin: 0; font-size: var(--text-sm); overflow-wrap: anywhere; }
  .np-preview { display: flex; flex-direction: column; gap: calc(var(--spacing) * 3); margin-top: calc(var(--spacing) * 4); }
  .np-check { display: flex; align-items: center; gap: calc(var(--spacing) * 2); font-size: var(--text-sm); }
  .np-treehead { display: flex; align-items: center; justify-content: space-between; }

  .upload-tree, .upload-tree ul { list-style: none; margin: 0; padding: 0; }
  .upload-tree ul { padding-left: calc(var(--spacing) * 4); }
  .upload-tree .upload-folder-name { display: flex; align-items: center; gap: var(--spacing); padding-block: calc(var(--spacing)); font-weight: 500; }
  .upload-file { display: flex; align-items: center; gap: calc(var(--spacing) * 3); min-height: 2rem; border-top: 1px solid var(--color-pane-edge); }
  .upload-file-name { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
  .upload-progress { width: 100%; accent-color: var(--color-focus); }
  .np-progress { display: flex; flex-direction: column; gap: calc(var(--spacing)); }

  .drop-overlay { position: fixed; inset: 0; z-index: 60; display: grid; place-items: center; background: color-mix(in srgb, var(--color-shell) 85%, transparent); border: 3px dashed var(--color-brand); pointer-events: none; }
  .drop-overlay p { margin: 0; font-size: var(--text-xl); font-weight: 600; }

  @media (max-width: 760px) {
    .new-project { padding: calc(var(--spacing) * 4); }
    .np-search { flex-basis: 100%; max-width: none; }
    .np-toolbar .np-field:not(.np-search) { flex: 1 1 8rem; }
    .np-toolbar .np-field :global(.select) { width: 100%; }
    .template-count { margin-left: 0; width: 100%; }

    /* The same rows as a list: name over description over formats, so nothing
       needs a sideways scroll. */
    .template-table, .template-table tbody { display: block; }
    .template-table thead { display: none; }
    .template-table tbody tr { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; grid-template-areas: "pick name info" "pick say info" "pick formats info"; column-gap: calc(var(--spacing) * 2); align-items: center; padding-block: calc(var(--spacing) * 2); border-top: 1px solid var(--color-pane-edge); }
    .template-table td { display: block; width: auto; height: auto; padding: 0; border: 0; }
    .template-table .col-pick { grid-area: pick; width: auto; }
    .template-table .col-name { grid-area: name; width: auto; }
    .template-table .col-say { grid-area: say; width: auto; }
    .template-table .col-formats { grid-area: formats; width: auto; font-size: var(--text-xs); }
    .template-table .col-info { grid-area: info; width: auto; }

    .np-create { flex-direction: column; align-items: stretch; }
    .np-create :global(.new-project-create) { margin-left: 0; }
    .np-form { max-width: none; }
    .np-actions { justify-content: stretch; width: 100%; }
    .np-actions :global(.new-project-create) { width: 100%; }
  }
</style>
