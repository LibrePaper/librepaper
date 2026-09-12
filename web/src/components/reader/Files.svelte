<script>
  import PanelHeader from "../PanelHeader.svelte";
  import { tick, onDestroy } from "svelte";
  import { TreeView, createTreeViewCollection, Menu } from "@skeletonlabs/skeleton-svelte";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import Modal from "../Modal.svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import { checkPath, collisionKey } from "../../lib/paths.js";
  import { basename, parentPath, inside, nodeKey, fileTree, folderPaths, topEntries, checkPlacement, copyPath, droppedFiles } from "../../lib/file-manager.js";

  let { files = [], folders = [], open = "", mayEdit = false, rules = {},
    onopen, onadd, onmkdir, onrelocate, ondelete, onduplicate, onmain, onfigure, ontext, ondownload, ondownloaditem } = $props();

  let selected = $state([]);
  let expanded = $state([]);
  let focused = $state("");
  let editing = $state(null);
  let draft = $state("");
  let refusal = $state("");
  let chooser = $state(null);
  let uploadTarget = "";
  let busy = $state(false);
  let hover = $state(null);
  let hoverTimer;
  let dialog = $state(null);
  let destination = $state("");
  let conflict = $state(null);
  let settleConflict;
  const dragType = "application/x-librepaper-files";
  const dragToken = crypto.randomUUID();
  const root = $derived(fileTree(files, folders));
  const collection = $derived(createTreeViewCollection({ rootNode: root, nodeToValue: (node) => node.id, nodeToString: (node) => node.name,
    nodeToChildrenCount: (node) => node.kind === "folder" ? node.children.length : undefined }));
  const directories = $derived(folderPaths(files, folders));
  const entries = $derived([...files, ...directories.map((path) => ({ kind: "folder", id: path, path }))]);
  const chosen = $derived(entries.filter((entry) => selected.includes(nodeKey(entry))));
  const currentFolder = $derived(chosen.length === 1 ? (chosen[0].kind === "folder" ? chosen[0].path : parentPath(chosen[0].path)) : "");
  const deleting = $derived(dialog?.type === "delete" ? files.filter((file) => dialog.entries.some((entry) => entry.path === file.path || entry.kind === "folder" && inside(file.path, entry.path))) : []);
  const protectedSelection = $derived(deleting.some((file) => file.main));
  const openPath = $derived(files.find((file) => file.id === open)?.path);

  onDestroy(() => { clearTimeout(hoverTimer); settleConflict?.(false); });
  // Follow an editor opened elsewhere (including diagnostics) into its folder.
  $effect(() => {
    if (!openPath) return;
    let path = parentPath(openPath);
    const parents = [];
    while (path) { parents.push(`folder:${path}`); path = parentPath(path); }
    // untracked state is read in the scheduled callback to avoid an expansion loop.
    if (parents.length) queueMicrotask(() => { expanded = [...new Set([...expanded, ...parents])]; });
  });

  function entryOf(node) { return node.kind === "folder" ? { kind: "folder", id: node.path, path: node.path } : files.find((file) => file.id === node.fileId && file.kind === node.kind); }
  function selectionFor(node) { return selected.includes(node.id) ? topEntries(chosen) : [entryOf(node)].filter(Boolean); }
  function expand(path) { if (path) expanded = [...new Set([...expanded, `folder:${path}`])]; }
  function reset() { editing = null; draft = ""; refusal = ""; }
  function focusName(element) { tick().then(() => { element.focus(); element.select(); }); }
  // `start` and `choose` are exported for the toolbar's File menu, which
  // offers what this panel's own toolbar does without making a person open
  // the panel first to find it.
  export function start(type, entry = null, parent = currentFolder) {
    if (!mayEdit) return;
    refusal = "";
    editing = { type, entry, parent };
    draft = type === "rename" ? basename(entry.path) : "";
    expand(parent);
  }
  async function commit() {
    if (!editing || !mayEdit) return;
    try {
      const path = [editing.parent, draft.trim()].filter(Boolean).join("/");
      if (!draft.trim()) throw new Error("Enter a name.");
      if (editing.type === "rename") await onrelocate?.([editing.entry], path, true);
      else {
        const kind = editing.type === "folder" ? "folder" : "text";
        const normalized = checkPlacement(rules, { kind, path }, files, folders);
        if (kind === "folder") await onmkdir?.(normalized);
        else await onadd?.(normalized);
      }
      reset();
    } catch (error) { refusal = error.message; }
  }
  function namingKey(event) {
    event.stopPropagation();
    if (event.key === "Enter") { event.preventDefault(); commit(); }
    if (event.key === "Escape") { event.preventDefault(); reset(); }
  }
  function ask(type, targets) {
    if (!mayEdit || !targets.length) return;
    reset();
    dialog = { type, entries: topEntries(targets) };
    destination = "";
  }
  async function confirm() {
    try {
      if (dialog.type === "delete") await ondelete?.(dialog.entries);
      else await onrelocate?.(dialog.entries, destination, false);
      selected = [];
      expand(destination);
      dialog = null;
      refusal = "";
    } catch (error) { refusal = error.message; }
  }
  async function action(value, node) {
    const entry = entryOf(node);
    const targets = selectionFor(node);
    if (!entry) return;
    if (value === "download") { ondownloaditem?.(entry); return; }
    if (!mayEdit) return;
    if (value === "rename") start("rename", entry, parentPath(entry.path));
    if (value === "move" || value === "delete") ask(value, targets);
    if (value === "file" || value === "folder") start(value, null, entry.path);
    if (value === "upload") choose(entry.path);
    if (value === "main") onmain?.(entry);
    if (value === "duplicate") {
      try { await onduplicate?.(entry, copyPath(entry.path, files, folders)); }
      catch (error) { refusal = error.message; }
    }
  }
  function renameOnDoubleClick(event, node) {
    if (!mayEdit || editing || event.target.closest("input, button")) return;
    event.preventDefault();
    event.stopPropagation();
    const entry = entryOf(node);
    if (entry) start("rename", entry, parentPath(entry.path));
  }
  function keyed(event) {
    if (!mayEdit || event.target.closest("input, button, [role=menu]")) return;
    if (event.key === "Escape") { selected = []; return; }
    const targets = chosen.length ? chosen : entries.filter((entry) => nodeKey(entry) === focused);
    if (event.key === "F2" && targets.length === 1) { event.preventDefault(); start("rename", targets[0], parentPath(targets[0].path)); }
    if (event.key === "Delete" && targets.length) { event.preventDefault(); ask("delete", targets); }
  }
  export function choose(path = currentFolder) { uploadTarget = path; chooser?.click(); }
  function resolveConflict(keep) { conflict = null; settleConflict?.(keep); settleConflict = null; }
  async function upload(items, target, incomingFolders = []) {
    if (!mayEdit || busy) return;
    busy = true;
    refusal = "";
    const errors = [];
    try {
      for (const folder of incomingFolders) {
        const path = [target, folder].filter(Boolean).join("/");
        if (!directories.includes(path)) await onmkdir?.(path);
      }
      for (const { file, path: relative } of items) {
        try {
          let path = [target, relative].filter(Boolean).join("/");
          const answer = checkPath(rules, path);
          if (answer.error) throw new Error(answer.error);
          if (entries.some((entry) => collisionKey(entry.path) === collisionKey(path))) {
            conflict = path;
            const keep = await new Promise((resolve) => { settleConflict = resolve; });
            if (!keep) continue;
            path = copyPath(path, files, folders);
          }
          path = checkPlacement(rules, { kind: answer.kind, path }, files, folders);
          if (answer.kind === "asset") await onfigure?.(file, path);
          else await ontext?.(file, path);
          await tick();
        } catch (error) { errors.push(error.message); }
      }
      expand(target);
    } catch (error) { errors.push(error.message); }
    finally { busy = false; refusal = errors.join("; "); }
  }
  export function offer(chosenFiles) {
    return upload(chosenFiles.map((file) => ({ file, path: file.webkitRelativePath || file.name })), "");
  }
  function dragStart(event, node) {
    if (!mayEdit || editing) { event.preventDefault(); return; }
    event.stopPropagation();
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData(dragType, JSON.stringify({ token: dragToken, entries: selectionFor(node) }));
  }
  function dragOver(event, path) {
    if (!mayEdit || busy || ![...event.dataTransfer.types].some((type) => type === dragType || type === "Files")) return;
    event.preventDefault(); event.stopPropagation();
    event.dataTransfer.dropEffect = event.dataTransfer.types.includes(dragType) ? "move" : "copy";
    if (hover !== path) {
      clearTimeout(hoverTimer);
      hover = path;
      hoverTimer = setTimeout(() => expand(path), 650);
    }
  }
  function dragEnd() { hover = null; clearTimeout(hoverTimer); }
  // The six drag-and-drop handlers a row needs are the same whether the row
  // is a folder or a file; only the node dragged and the path dropped onto
  // differ, so a tree row spreads this rather than repeating them.
  function dragAttrs(node, dropPath) {
    return {
      draggable: mayEdit && !editing,
      ondragstart: (event) => dragStart(event, node),
      ondragend: dragEnd,
      ondragover: (event) => dragOver(event, dropPath),
      ondrop: (event) => drop(event, dropPath),
      ondblclick: (event) => renameOnDoubleClick(event, node),
    };
  }
  async function drop(event, path) {
    event.preventDefault(); event.stopPropagation(); dragEnd();
    if (!mayEdit || busy) return;
    try {
      const payload = event.dataTransfer.getData(dragType);
      if (payload) {
        const data = JSON.parse(payload);
        if (data.token !== dragToken) throw new Error("Move files within this project's explorer.");
        await onrelocate?.(data.entries, path, false);
        selected = []; expand(path); refusal = "";
      } else {
        const incoming = await droppedFiles(event.dataTransfer);
        await upload(incoming.files, path, incoming.folders);
      }
    } catch (error) { refusal = error.message; }
  }
</script>

<div class="panel filelist explorer" class:explorer-drop={hover === ""} role="region" aria-label="File manager"
  onpointerdown={(event) => { if (!event.target.closest('[role="treeitem"], button, input, select, header')) selected = []; }}
  ondragover={(event) => dragOver(event, "")} ondrop={(event) => drop(event, "")} ondragleave={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) dragEnd(); }}>
  <PanelHeader title="Files">
    {#snippet actions()}
      <div class="explorer-actions" aria-label="File actions">
        {#if mayEdit}
          <IconButton icon="file-plus" label="New file" tone="plain" size="btn-icon-sm" onclick={() => start("file")} />
          <IconButton icon="folder-plus" label="New folder" tone="plain" size="btn-icon-sm" onclick={() => start("folder")} />
          <IconButton icon="upload" label="Upload files" tone="plain" size="btn-icon-sm" disabled={busy} onclick={() => choose()} />
        {/if}
        <IconButton icon="chevrons-up" label="Collapse all folders" tone="plain" size="btn-icon-sm" disabled={!expanded.length} onclick={() => expanded = []} />
        <IconButton icon="download" label="Download project" tone="plain" size="btn-icon-sm" onclick={() => ondownload?.()} />
      </div>
      {#if mayEdit}<input class="chooser" type="file" multiple bind:this={chooser} aria-label="Choose files to upload"
        onchange={(event) => { const picked = [...event.target.files]; event.target.value = ""; upload(picked.map((file) => ({ file, path: file.name })), uploadTarget); }} />{/if}
    {/snippet}
    {#if chosen.length > 1 && mayEdit}
      <span class="panel-meta">{chosen.length} selected</span>
    {/if}
    {#if busy}<p role="status">Uploading files…</p>{/if}
    {#if refusal && !dialog}<p class="refusal" role="alert">{refusal}</p>{/if}
    {#if editing && editing.type !== "rename"}
      <div class="space-y-1">
        <label for="new-project-entry">New {editing.type} in /{editing.parent}</label>
        <div class="flex items-center gap-1">
          <input id="new-project-entry" class="name" aria-label="New {editing.type} name" bind:value={draft} use:focusName onkeydown={namingKey} />
          <IconButton icon="check" label="Create {editing.type}" onclick={commit} />
          <IconButton icon="x" label="Cancel" onclick={reset} />
        </div>
      </div>
    {/if}
  </PanelHeader>

  <div class="explorer-scroll">
  <TreeView {collection} selectionMode="multiple" selectedValue={selected} expandedValue={expanded}
    onExpandedChange={(event) => { expanded = event.expandedValue; }}
    onFocusChange={(event) => { focused = event.focusedValue; }}
    onSelectionChange={(event) => {
      selected = event.selectedValue;
      if (selected.length === 1 && mayEdit) {
        const entry = entries.find((entry) => nodeKey(entry) === selected[0]);
        if (entry && entry.kind !== "folder") onopen?.(entry);
      }
    }}>
    <TreeView.Label class="sr-only">Project files</TreeView.Label>
    <TreeView.Tree onkeydown={keyed}>
      {#each root.children as node, index (node.id)}{@render branch(node, [index])}{/each}
    </TreeView.Tree>
  </TreeView>
  </div>
</div>

{#snippet row(node)}
  <span class="explorer-chevron" aria-hidden="true">{#if node.kind === "folder"}<Icon name={expanded.includes(node.id) ? "chevron-down" : "chevron-right"} />{/if}</span>
  <Icon name={node.kind === "folder" ? "folder" : node.kind === "asset" ? "image" : "file-text"} />
  {#if editing?.type === "rename" && editing.entry.path === node.path}
    <input class="name" aria-label="Rename {node.path}" bind:value={draft} use:focusName onkeydown={namingKey} onclick={(event) => event.stopPropagation()} />
    <span onclick={(event) => event.stopPropagation()} role="presentation"><IconButton icon="check" label="Save name" onclick={commit} /><IconButton icon="x" label="Cancel rename" onclick={reset} /></span>
  {:else}
    <span class="explorer-name">{node.name}</span>
  {/if}
{/snippet}

{#snippet nodeMenu(node)}
  <ExplorerMenu>
    {#if mayEdit}
      {#if node.kind === "folder"}
        <Menu.Item value="file" class="menuitem">New file</Menu.Item>
        <Menu.Item value="folder" class="menuitem">New folder</Menu.Item>
        <Menu.Item value="upload" class="menuitem" disabled={busy}>Upload files</Menu.Item>
      {/if}
      <Menu.Item value="rename" class="menuitem">Rename <span class="ml-auto text-xs">F2</span></Menu.Item>
      <Menu.Item value="move" class="menuitem">Move to…</Menu.Item>
      {#if node.kind !== "folder"}<Menu.Item value="duplicate" class="menuitem">Duplicate</Menu.Item>{/if}
      {#if node.kind === "text" && !node.main}<Menu.Item value="main" class="menuitem">Set as main file</Menu.Item>{/if}
    {/if}
    <Menu.Item value="download" class="menuitem">Download</Menu.Item>
    {#if mayEdit}<Menu.Item value="delete" class="menuitem" disabled={node.main}>Delete…</Menu.Item>{/if}
  </ExplorerMenu>
{/snippet}

{#snippet branch(node, indexPath)}
  <TreeView.NodeProvider value={{ node, indexPath }}>
    {#if node.kind === "folder"}
      <TreeView.Branch>
        <Menu onSelect={({ value }) => action(value, node)}>
          <Menu.ContextTrigger>
            {#snippet element(attributes)}
              <div {...attributes}>
                <TreeView.BranchControl class="explorer-row {hover === node.path ? 'drop-target' : ''}" title={node.path}
                  {...dragAttrs(node, node.path)}>
                  {@render row(node)}
                </TreeView.BranchControl>
              </div>
            {/snippet}
          </Menu.ContextTrigger>
          {@render nodeMenu(node)}
        </Menu>
        <TreeView.BranchContent>
          {#each node.children as child, index (child.id)}{@render branch(child, [...indexPath, index])}{/each}
        </TreeView.BranchContent>
      </TreeView.Branch>
    {:else}
      <Menu onSelect={({ value }) => action(value, node)}>
        <Menu.ContextTrigger>
          {#snippet element(attributes)}
            <div {...attributes}>
              <TreeView.Item class="explorer-row {open === node.fileId ? 'explorer-open' : ''} {hover === parentPath(node.path) ? 'drop-target' : ''}" title={node.path}
                {...dragAttrs(node, parentPath(node.path))}>
                {@render row(node)}
              </TreeView.Item>
            </div>
          {/snippet}
        </Menu.ContextTrigger>
        {@render nodeMenu(node)}
      </Menu>
    {/if}
  </TreeView.NodeProvider>
{/snippet}

<Modal open={dialog !== null} onclose={() => { dialog = null; refusal = ""; }} title={dialog?.type === "delete" ? "Delete selected items?" : "Move selected items"}>
  {#if dialog?.type === "delete"}
    <p>{dialog.entries.map((entry) => basename(entry.path)).join(", ")}</p>
    <p>{deleting.length} {deleting.length === 1 ? "file" : "files"} will be deleted. This cannot be undone.</p>
    {#if protectedSelection}<p class="refusal">Choose another main file before deleting this file or its folder.</p>{/if}
  {:else if dialog}
    <label for="move-destination">Destination folder</label>
    <select id="move-destination" class="select" bind:value={destination}>
      <option value="">Top level</option>
      {#each directories.filter((path) => !dialog.entries.some((entry) => entry.kind === "folder" && (entry.path === path || inside(path, entry.path)))) as path}
        <option value={path}>{path}</option>
      {/each}
    </select>
    <p class="text-sm text-surface-600-400">References in source files are not changed automatically.</p>
  {/if}
  {#if refusal}<p class="refusal" role="alert">{refusal}</p>{/if}
  {#snippet footer()}
    <button class="btn preset-outlined-surface-300-700" onclick={() => { dialog = null; refusal = ""; }}>Cancel</button>
    <button class="btn {dialog?.type === 'delete' ? 'preset-filled-error-500' : 'preset-filled-primary-500'}" disabled={protectedSelection} onclick={confirm}>{dialog?.type === "delete" ? "Delete" : "Move"}</button>
  {/snippet}
</Modal>
<Modal open={conflict !== null} onclose={() => resolveConflict(false)} title="This name is already in use">
  <p>{conflict}</p><p>Keep both files with a new name, or skip this upload.</p>
  {#snippet footer()}
    <button class="btn preset-outlined-surface-300-700" onclick={() => resolveConflict(false)}>Skip</button>
    <button class="btn preset-filled-primary-500" onclick={() => resolveConflict(true)}>Keep both</button>
  {/snippet}
</Modal>

<style>
  .filelist { display: flex; flex-direction: column; overflow: hidden; }
  .filelist > :global(*) { flex-shrink: 0; }
  .explorer-scroll { flex: 1 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .explorer-actions {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 1);
    flex-wrap: wrap;
  }
</style>
