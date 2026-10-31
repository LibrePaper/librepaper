<script>
  import { m } from "../../lib/paraglide/messages.js";
  import PanelHeader from "../PanelHeader.svelte";
  import { tick, onDestroy, untrack } from "svelte";
  import { TreeView, createTreeViewCollection, Menu } from "@skeletonlabs/skeleton-svelte";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import Modal from "../Modal.svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import { checkDirectoryPath, checkPath, collisionKey } from "../../lib/paths.js";
  import { basename, parentPath, inside, nodeKey, fileTree, folderPaths, topEntries, checkPlacement, copyPath, droppedFiles } from "../../lib/file-manager.js";
  import { expandArchives } from "../../lib/project-upload.js";
  import { unzip } from "../../lib/zip.js";
  import { retargetElementAttributes } from "../element-attributes.js";

  let { files = [], folders = [], open = "", preview = "", locked = "", mayEdit = false, rules = {},
    onopen, onadd, onmkdir, onrelocate, ondelete, onduplicate, onfigure, ontext, ondownload, ondownloaditem,
    previewable = () => false, onlock = undefined, onunlock = undefined,
    // The demo cannot store files on the server, so uploads are greyed out.
    demo = false } = $props();

  let selected = $state([]);
  let expanded = $state([]);
  let focused = $state("");
  let editing = $state(null);
  let draft = $state("");
  let refusal = $state("");
  let uploadWarning = $state("");
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
  const allFolders = $derived(directories.map((path) => `folder:${path}`));
  const entries = $derived([...files, ...directories.map((path) => ({ kind: "folder", id: path, path }))]);
  const chosen = $derived(entries.filter((entry) => selected.includes(nodeKey(entry))));
  const currentFolder = $derived(chosen.length === 1 ? (chosen[0].kind === "folder" ? chosen[0].path : parentPath(chosen[0].path)) : "");
  const deleting = $derived(dialog?.type === "delete" ? files.filter((file) => dialog.entries.some((entry) => entry.path === file.path || entry.kind === "folder" && inside(file.path, entry.path))) : []);
  const openPath = $derived(files.find((file) => file.id === open)?.path);
  // Project files are text or asset formats, plus ZIP archives that unpack into them.
  const allowedExtensions = $derived([...new Set([...(rules.text_extensions || []), ...(rules.asset_extensions || []), ".zip"])].sort());

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
  // The selection follows a file opened elsewhere too (the outline, a
  // diagnostic), so choosing the file that was open before is still a change
  // the tree reports. It moves only when the open file changes, and a
  // selection of several items is left alone.
  $effect(() => {
    const id = open;
    untrack(() => {
      const file = files.find((entry) => entry.id === id);
      if (!file) return;
      const key = nodeKey(file);
      if (selected.length <= 1 && selected[0] !== key) selected = [key];
    });
  });

  function entryOf(node) { return node.kind === "folder" ? { kind: "folder", id: node.path, path: node.path } : files.find((file) => file.id === node.fileId && file.kind === node.kind); }
  function selectionFor(node) { return selected.includes(node.id) ? topEntries(chosen) : [entryOf(node)].filter(Boolean); }
  function expand(path) { if (path) expanded = [...new Set([...expanded, `folder:${path}`])]; }
  function reset() { editing = null; draft = ""; refusal = ""; uploadWarning = ""; }
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
      if (!draft.trim()) throw new Error(m.files_enter_name());
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
    if (value === "duplicate") {
      try { await onduplicate?.(entry, copyPath(entry.path, files, folders)); }
      catch (error) { refusal = error.message; }
    }
  }
  function renameOnDoubleClick(event, node) {
    if (!(event.target instanceof Element) || !mayEdit || editing || event.target.closest("input, button")) return;
    event.preventDefault();
    event.stopPropagation();
    const entry = entryOf(node);
    if (entry) start("rename", entry, parentPath(entry.path));
  }
  function keyed(event) {
    if (!(event.target instanceof Element) || !mayEdit || event.target.closest("input, button, [role=menu]")) return;
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
    uploadWarning = "";
    const errors = [];
    const skipped = [];
    try {
      // A ZIP is the files inside it. Expanding before anything is written
      // means a broken archive refuses the whole import rather than leaving
      // half of one behind, and an archive dropped on a folder unpacks there.
      if (items.some((item) => /\.zip$/i.test(item.path))) {
        const opened = await expandArchives(items, rules, unzip);
        items = opened.files;
        incomingFolders = [...incomingFolders, ...opened.folders];
      }
      for (const folder of incomingFolders) {
        const path = [target, folder].filter(Boolean).join("/");
        if (!directories.includes(path)) await onmkdir?.(path);
      }
      for (const { file, path: relative } of items) {
        try {
          let path = [target, relative].filter(Boolean).join("/");
          const answer = checkPath(rules, path);
          if (answer.error) {
            // Invalid paths remain errors. A valid path with an unsupported
            // extension is a skipped upload and should not block its siblings.
            if (!checkDirectoryPath(rules, path)) { skipped.push(relative); continue; }
            throw new Error(answer.error);
          }
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
    finally {
      busy = false;
      refusal = errors.join("; ");
      if (skipped.length) {
        uploadWarning = m.files_skipped({ count: skipped.length, names: skipped.join(", "), extensions: allowedExtensions.join(", ") || m.common_none_inline() });
      }
    }
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
        if (data.token !== dragToken) throw new Error(m.files_move_within_explorer());
        await onrelocate?.(data.entries, path, false);
        selected = []; expand(path); refusal = "";
      } else {
        const incoming = await droppedFiles(event.dataTransfer);
        await upload(incoming.files, path, incoming.folders);
      }
    } catch (error) { refusal = error.message; }
  }
</script>

<div class="panel filelist explorer" class:explorer-drop={hover === ""} role="region" aria-label={m.files_manager_label()}
  onpointerdown={(event) => { if (!(event.target instanceof Element) || !event.target.closest('[role="treeitem"], button, input, select, header')) selected = []; }}
  ondragover={(event) => dragOver(event, "")} ondrop={(event) => drop(event, "")} ondragleave={(event) => { if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) dragEnd(); }}>
  <PanelHeader title={m.common_files()}>
    {#snippet actions()}
      <!-- One run of controls, left to right: what makes something in the
           tree, what folds it, then what moves files across its edge. They
           are all the same kind of thing to press, so they sit in one group
           rather than a pair pushed off to the far end of the header. -->
      <div class="explorer-actions" aria-label={m.files_actions_label()}>
        {#if mayEdit}
          <IconButton icon="file-plus" label={m.common_new_file()} tone="plain" size="btn-icon-sm" onclick={() => start("file")} />
          <IconButton icon="folder-plus" label={m.common_new_folder()} tone="plain" size="btn-icon-sm" onclick={() => start("folder")} />
        {/if}
        <IconButton icon="chevrons-down" label={m.files_expand_all()} tone="plain" size="btn-icon-sm" disabled={allFolders.every((value) => expanded.includes(value))} onclick={() => expanded = allFolders} />
        <IconButton icon="chevrons-up" label={m.files_collapse_all()} tone="plain" size="btn-icon-sm" disabled={!expanded.length} onclick={() => expanded = []} />
        {#if mayEdit}
          <IconButton icon="upload" label={demo ? m.files_upload_demo() : m.common_upload_files()} tone="plain" size="btn-icon-sm" disabled={busy || demo} onclick={() => choose()} />
        {/if}
        {#if mayEdit}<IconButton icon="download" label={m.common_download_project()} tone="plain" size="btn-icon-sm" onclick={() => ondownload?.()} />{/if}
      </div>
      {#if mayEdit}<input class="chooser" type="file" multiple bind:this={chooser} aria-label={m.files_choose_upload()}
        accept={allowedExtensions.join(",")}
      onchange={(event) => { const input = event.currentTarget; const picked = [...(input.files || [])]; input.value = ""; upload(picked.map((file) => ({ file, path: file.name })), uploadTarget); }} />{/if}
    {/snippet}
    {#if chosen.length > 1 && mayEdit}
      <span class="panel-meta">{m.files_selected_count({ count: chosen.length })}</span>
    {/if}
    {#if busy}<p role="status">{m.files_uploading()}</p>{/if}
    {#if uploadWarning}<p class="upload-warning" role="status">{uploadWarning}</p>{/if}
    {#if refusal && !dialog}<p class="refusal" role="alert">{refusal}</p>{/if}
    {#if editing && editing.type !== "rename"}
      <div class="space-y-1">
        <label for="new-project-entry">{editing.type === "folder" ? m.files_new_folder_in({ parent: editing.parent }) : m.files_new_file_in({ parent: editing.parent })}</label>
        <div class="flex items-center gap-1">
          <input id="new-project-entry" class="name" aria-label={editing.type === "folder" ? m.files_new_folder_name() : m.files_new_file_name()} bind:value={draft} use:focusName onkeydown={namingKey} />
          <IconButton icon="check" label={editing.type === "folder" ? m.files_create_folder() : m.files_create_file()} onclick={commit} />
          <IconButton icon="x" label={m.common_cancel()} onclick={reset} />
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
      if (selected.length === 1) {
        const entry = entries.find((entry) => nodeKey(entry) === selected[0]);
        if (entry && entry.kind !== "folder") onopen?.(entry);
      }
    }}>
    <TreeView.Label class="sr-only">{m.files_project_files_label()}</TreeView.Label>
    <TreeView.Tree onkeydown={keyed}>
      {#each root.children as node, index (node.id)}{@render branch(node, [index])}{/each}
    </TreeView.Tree>
  </TreeView>
  </div>
</div>

<!-- A row is the same shape whether it opens or not, but a folder's chevron
     and name are Skeleton's parts rather than ours: the chevron reads
     `data-state` off the machine instead of off a second copy of the
     expansion kept here, which could disagree with it. There is no exposed
     part for a leaf's text, so a file's name stays a span. -->
{#snippet row(node)}
  {#if node.kind === "folder"}
    <TreeView.BranchIndicator class="explorer-chevron"><Icon name="chevron-right" /></TreeView.BranchIndicator>
  {:else}
    <span class="explorer-chevron" aria-hidden="true"></span>
  {/if}
  <Icon name={node.kind === "folder" ? "folder" : node.kind === "asset" ? "image" : "file-text"} />
  {#if editing?.type === "rename" && editing.entry.path === node.path}
    <input class="name" aria-label={m.files_rename_path({ path: node.path })} bind:value={draft} use:focusName onkeydown={namingKey} onclick={(event) => event.stopPropagation()} />
    <span onclick={(event) => event.stopPropagation()} role="presentation"><IconButton icon="check" label={m.files_save_name()} onclick={commit} /><IconButton icon="x" label={m.files_cancel_rename()} onclick={reset} /></span>
  {:else if node.kind === "folder"}
    <TreeView.BranchText class="explorer-name">{node.name}</TreeView.BranchText>
  {:else}
    <span class="explorer-name">{node.name}</span>
  {/if}
  <!-- The eye is the lock. Only the locked file shows it solid, and pressing
       it unlocks. Every other file that renders on its own offers the faint
       eye, which locks the preview there without opening the file. The file
       being previewed without a lock is the open file, which its row already
       marks. -->
  {#if node.kind !== "folder" && locked && node.path === locked}
    <button type="button" class="explorer-preview shown" aria-label={m.common_unlock_the_preview()} title={m.common_unlock_the_preview()}
      onclick={(event) => { event.stopPropagation(); onunlock?.(); }}
      ondblclick={(event) => event.stopPropagation()}
      onkeydown={(event) => event.stopPropagation()}><Icon name="eye" /></button>
  {:else if node.kind === "text" && previewable(node.path)}
    <button type="button" class="explorer-preview" aria-label={m.common_lock_preview_on({ name: node.name })} title={m.common_lock_preview_on_file()}
      onclick={(event) => { event.stopPropagation(); const entry = entryOf(node); if (entry) onlock?.(entry); }}
      ondblclick={(event) => event.stopPropagation()}
      onkeydown={(event) => event.stopPropagation()}><Icon name="eye" /></button>
  {/if}
{/snippet}

{#snippet nodeMenu(node)}
  <ExplorerMenu>
    {#if mayEdit}
      {#if node.kind === "folder"}
        <Menu.Item value="file" class="menuitem">{m.common_new_file()}</Menu.Item>
        <Menu.Item value="folder" class="menuitem">{m.common_new_folder()}</Menu.Item>
        <Menu.Item value="upload" class="menuitem" disabled={busy || demo}>{demo ? m.files_upload_demo() : m.common_upload_files()}</Menu.Item>
      {/if}
      <Menu.Item value="rename" class="menuitem">{m.common_rename()} <span class="ml-auto text-xs">F2</span></Menu.Item>
      <Menu.Item value="move" class="menuitem">{m.files_move_to()}</Menu.Item>
      {#if node.kind !== "folder"}<Menu.Item value="duplicate" class="menuitem">{m.files_duplicate()}</Menu.Item>{/if}
    {/if}
    <Menu.Item value="download" class="menuitem">{m.common_download()}</Menu.Item>
    {#if mayEdit}<Menu.Item value="delete" class="menuitem">{m.files_delete_menu()}</Menu.Item>{/if}
  </ExplorerMenu>
{/snippet}

{#snippet branch(node, indexPath)}
  <TreeView.NodeProvider value={{ node, indexPath }}>
    {#if node.kind === "folder"}
      <TreeView.Branch>
        <Menu onSelect={({ value }) => action(value, node)}>
          <Menu.ContextTrigger>
            {#snippet element(attributes)}
              <div {...retargetElementAttributes(attributes)}>
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
          <!-- The line down the side of an open folder. Zag gives it the
               depth it is at, so it lands under that folder's chevron
               without this having to count the nesting again. -->
          <TreeView.BranchIndentGuide class="explorer-guide" />
          {#each node.children as child, index (child.id)}{@render branch(child, [...indexPath, index])}{/each}
        </TreeView.BranchContent>
      </TreeView.Branch>
    {:else}
      <Menu onSelect={({ value }) => action(value, node)}>
        <Menu.ContextTrigger>
          {#snippet element(attributes)}
            <div {...retargetElementAttributes(attributes)}>
              <TreeView.Item class="explorer-row {open === node.fileId ? 'explorer-open' : ''} {hover === parentPath(node.path) ? 'drop-target' : ''}" title={node.path}
                data-previewed={node.path === preview ? "" : undefined}
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

<Modal open={dialog !== null} onclose={() => { dialog = null; refusal = ""; }} title={dialog?.type === "delete" ? m.files_dialog_delete_title() : m.files_dialog_move_title()}
  confirm={{
    label: dialog?.type === "delete" ? m.common_delete() : m.files_move(),
    tone: dialog?.type === "delete" ? "error" : "primary",
    onclick: confirm,
    oncancel: () => { dialog = null; refusal = ""; },
  }}>
  {#if dialog?.type === "delete"}
    <p>{dialog.entries.map((entry) => basename(entry.path)).join(", ")}</p>
    <p>{m.files_will_be_deleted({ count: deleting.length })}</p>
  {:else if dialog}
    <label for="move-destination">{m.files_destination_folder()}</label>
    <select id="move-destination" class="select" bind:value={destination}>
      <option value="">{m.files_top_level()}</option>
      {#each directories.filter((path) => !dialog.entries.some((entry) => entry.kind === "folder" && (entry.path === path || inside(path, entry.path)))) as path}
        <option value={path}>{path}</option>
      {/each}
    </select>
    <p class="text-sm lp-text-secondary">{m.files_references_unchanged()}</p>
  {/if}
  {#if refusal}<p class="refusal" role="alert">{refusal}</p>{/if}
</Modal>
<Modal open={conflict !== null} onclose={() => resolveConflict(false)} title={m.files_name_in_use()}
  cancelLabel={m.common_skip()}
  confirm={{ label: m.common_keep_both(), onclick: () => resolveConflict(true), oncancel: () => resolveConflict(false) }}>
  <p>{conflict}</p><p>{m.files_keep_both_note()}</p>
</Modal>

<style>
  .filelist { display: flex; flex-direction: column; overflow: hidden; }
  .filelist > :global(*) { flex-shrink: 0; }
  /* The toolbar and the tree are one thing: what you can make, and what is
     there. A panel's standard gap under its actions is meant to separate
     sections, and it reads here as a hole above the first file. */
  .filelist > :global(.panel-actions) { margin-bottom: calc(var(--spacing) * 1.5); }
  .explorer-scroll { flex: 1 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  /* Pushed to the far end by the name, which takes the free width. */
  .explorer-preview { display: flex; flex: none; align-items: center; justify-content: center; width: calc(var(--spacing) * 7); height: calc(var(--spacing) * 7); margin-inline-end: calc(var(--spacing) * -1.5); padding: 0; border: 0; border-radius: var(--radius-base); background: none; color: var(--color-text-secondary); }
  button.explorer-preview { color: var(--color-text-muted); opacity: 0.45; cursor: pointer; }
  button.explorer-preview:hover, button.explorer-preview:focus-visible { opacity: 1; color: var(--color-text-secondary); background: var(--color-row-hover); }
  button.explorer-preview.shown { color: var(--color-text-secondary); opacity: 1; cursor: pointer; }
  button.explorer-preview:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .explorer-actions {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 1);
    flex-wrap: wrap;
  }
</style>
