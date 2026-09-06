<script>
  // The project's directory, as a panel of the column.
  //
  // Paths are shown as paths. There is no folder tree to expand, because a
  // paper has a dozen files and a tree is for hundreds: `chapters/03.tex` is
  // one line that says where it is, and a reader of this list does not have to
  // open two disclosures to find out.
  //
  // The main file is first and the rest are sorted, so the list does not
  // reorder itself as somebody types. Beside each file are the initials of
  // whoever has their caret in it, which is what makes a modular paper feel
  // like one room rather than several.
  //
  // Everybody is shown the list: it is the shape of the project, and a reader
  // who cannot edit still wants to know what is in it. Only an editor can
  // open a file, because opening one means the source pane, and only an
  // editor has one.
  import IconButton from "./IconButton.svelte";
  import Row from "./layout/Row.svelte";
  import { checkPath } from "../lib/paths.js";

  let {
    files = [],
    open = "",
    peers = new Map(),
    mayEdit = false,
    rules = {},
    onopen,
    onadd,
    onrename,
    onremove,
    onmain,
    onfigure,
    ontext,
    ondownload,
  } = $props();

  let adding = $state(false);
  let renaming = $state("");
  let draft = $state("");
  let refusal = $state("");
  let chooser = $state(null);

  /// A figure is chosen rather than named: its bytes come from the person's
  /// own disk, and the name it will be known by is the name it already has.
  /// The refusal comes first -- the extension and the ceilings are checked
  /// here so that a file that cannot be stored is not uploaded first.
  function chooseFigures(event) {
    const chosen = [...(event.target.files || [])];
    event.target.value = ""; // so the same file can be chosen twice
    offer(chosen);
  }

  /// Files from a chooser or a drop. What each one becomes follows from its
  /// name, by the same rule the server uses: a `.tex` is a text and is read
  /// into the document, a `.png` is a figure and its bytes are stored. A name
  /// that is neither is refused here, before anything is uploaded.
  export function offer(chosen) {
    refusal = "";
    for (const file of chosen) {
      const answer = checkPath(rules, file.name);
      if (answer.error) {
        refusal = answer.error;
        continue;
      }
      if (files.some((known) => known.path.toLowerCase() === file.name.toLowerCase())) {
        refusal = `${file.name}: there is already a file with that name`;
        continue;
      }
      if (answer.kind === "asset") onfigure?.(file);
      else ontext?.(file);
    }
  }

  // The rules refuse a name here as well as at the server, so the reason is
  // shown where the person is typing rather than arriving as a status code.
  // The server checks it again; this is the early word, never the enforcement.
  function refuse(path, taken) {
    const answer = checkPath(rules, path);
    if (answer.error) return answer.error;
    if (taken.some((file) => file.path.toLowerCase() === path.trim().toLowerCase())) {
      return `${path}: there is already a file with that name`;
    }
    return "";
  }

  function startAdding() {
    adding = true;
    renaming = "";
    draft = "";
    refusal = "";
  }

  function startRenaming(file) {
    renaming = file.id;
    adding = false;
    draft = file.path;
    refusal = "";
  }

  function cancel() {
    adding = false;
    renaming = "";
    draft = "";
    refusal = "";
  }

  function commit() {
    const path = draft.trim();
    // Renaming a file to its own name is not an edit, and should not be an
    // error either.
    const others = renaming ? files.filter((file) => file.id !== renaming) : files;
    const said = refuse(path, others);
    if (said) {
      refusal = said;
      return;
    }
    if (renaming) {
      const original = files.find((file) => file.id === renaming);
      onrename?.(renaming, path, original?.kind);
    }
    else onadd?.(path);
    cancel();
  }

  function keyed(event) {
    if (event.key === "Enter") commit();
    if (event.key === "Escape") cancel();
  }

  const figures = $derived(files.filter((file) => file.kind === "asset").length);
</script>

<!-- One of the column's panels: the column itself, with the tabs that choose
     between them, is the reader's. -->
<div class="panel filelist">
  <header class="mb-3 pt-3">
    <Row justify="between">
      <h3 class="h5">Files</h3>
      {#if files.length}
        <small class="text-surface-600-400">
          {files.length} {files.length === 1 ? "file" : "files"}{figures
            ? ` · ${figures} ${figures === 1 ? "figure" : "figures"}`
            : ""}
        </small>
      {/if}
    </Row>
    <!-- What can be done to the directory, in one row: a text is named, a
         figure is chosen from a disk, and the whole of it can be taken away
         as a zip -- which is anyone's to do, since anyone here can read it. -->
    <div class="filetools-row mt-2">
      {#if mayEdit}
        <IconButton icon="file-plus" label="Add a file" onclick={startAdding} />
        <IconButton icon="image" label="Add a figure" onclick={() => chooser?.click()} />
      {/if}
      <IconButton icon="download" label="Download the project as a zip" title="Download" onclick={() => ondownload?.()} />
    </div>
    {#if mayEdit}
      <input
        class="chooser"
        type="file"
        multiple
        bind:this={chooser}
        onchange={chooseFigures}
        aria-label="Choose a figure to add"
      />
    {/if}
    {#if refusal}
      <p class="refusal mt-2" role="alert">{refusal}</p>
    {/if}
  </header>

  <ul>
    {#each files as file (file.id)}
      <li class:open={file.id === open} class:mainfile={file.main}>
        {#if renaming === file.id}
          <!-- The field exists because somebody just asked to rename this file,
               so the caret belongs in it. The rule is about a page that takes
               the focus on load, which this is not. -->
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="name"
            bind:value={draft}
            onkeydown={keyed}
            onblur={cancel}
            aria-label="Rename {file.path}"
            autofocus
          />
        {:else if mayEdit}
          <button class="path" onclick={() => onopen?.(file)} title={file.path}>
            {file.path}
          </button>
        {:else}
          <!-- A reader has no source pane to open a file in, so the path is
               a fact rather than a control. -->
          <span class="path" title={file.path}>{file.path}</span>
        {/if}
        {#if renaming !== file.id}
          <span class="who">{(peers.get(file.id) || []).slice(0, 3).join(" ")}</span>
          {#if mayEdit}
            <span class="filetools">
              {#if !file.main && file.kind === "text"}
                <IconButton
                  icon="star"
                  tone="plain"
                  size="btn-icon-sm"
                  label="Make {file.path} the main file"
                  onclick={() => onmain?.(file)}
                />
              {/if}
              <IconButton
                icon="pencil"
                tone="plain"
                size="btn-icon-sm"
                label="Rename {file.path}"
                onclick={() => startRenaming(file)}
              />
              {#if !file.main}
                <IconButton
                  icon="trash"
                  tone="plain"
                  size="btn-icon-sm"
                  label="Delete {file.path}"
                  onclick={() => onremove?.(file)}
                />
              {/if}
            </span>
          {/if}
        {/if}
      </li>
    {/each}
    {#if adding}
      <li class="adding">
        <!-- As above: this row exists because somebody just asked to add a file. -->
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="name"
          bind:value={draft}
          onkeydown={keyed}
          placeholder="chapters/03.tex"
          aria-label="The new file's name"
          autofocus
        />
      </li>
    {/if}
  </ul>
</div>
