<script>
  // Resolves the conflicts a pull found. Nothing is written into the project
  // until every path is resolved and Apply succeeds. The session is kept by
  // sync.js, so closing this keeps it.
  import Modal from "../Modal.svelte";
  import { applyResolution, saveResolutions } from "../../lib/gitsync/sync.js";

  let { open = $bindable(false), ctx, session = null, onresult, onclose = undefined } = $props();

  // Per path: { kind: "text" | "binary", base, ours, theirs (text or null), baseId, oursId, theirsId }.
  // "ours" is LibrePaper's copy and "theirs" the repository's.
  let items = $state([]);
  // Per path, as applyResolution takes it: { keep: "ours" | "theirs" } | { delete: true } | { text }.
  let resolutions = $state({});
  let loading = $state(false);
  let busy = $state(false);
  let error = $state("");
  let current = $state(null);
  let loaded = null;

  const decoder = new TextDecoder("utf-8", { fatal: true });

  /// The text of a blob, null for no blob, undefined for bytes that are not text.
  function decode(bytes) {
    if (bytes == null) return null;
    if (bytes.includes(0)) return undefined;
    try {
      return decoder.decode(bytes);
    } catch {
      return undefined;
    }
  }

  async function load(saved) {
    loading = true;
    error = "";
    try {
      const conflicts = saved.conflicts || [];
      const ids = [...new Set(conflicts.flatMap((item) => [item.base, item.ours, item.theirs]).filter(Boolean))];
      const blobs = ids.length ? await ctx.git.blobs(ids) : new Map();
      const next = [];
      const chosen = {};
      for (const item of conflicts) {
        const texts = [item.base, item.ours, item.theirs].map((id) => (id ? decode(blobs.get(id)) : null));
        const binary = item.kind === "asset" || texts.some((text) => text === undefined);
        next.push({
          path: item.path,
          kind: binary ? "binary" : "text",
          baseId: item.base,
          oursId: item.ours,
          theirsId: item.theirs,
          base: texts[0],
          ours: texts[1],
          theirs: texts[2],
        });
        const kept = saved.resolutions?.[item.path] ?? saved.carried?.[item.path];
        if (kept) chosen[item.path] = kept;
      }
      items = next;
      resolutions = chosen;
    } catch (failure) {
      error = failure?.message || "Could not read the conflicting files.";
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (open && session && session !== loaded) {
      loaded = session;
      current = session;
      load(session);
    }
  });

  const deleteVsModify = (item) => (item.oursId == null) !== (item.theirsId == null);
  let timer = null;

  function cancelSave() {
    if (timer) clearTimeout(timer);
    timer = null;
  }

  /// Stores the choices on the saved session so a closed window or a restart keeps them.
  async function persist() {
    cancelSave();
    if (!current) return;
    try {
      await saveResolutions(ctx, current, resolutions);
    } catch {
      // The choices stay on screen; the next change tries again.
    }
  }

  const choose = (path, value) => {
    resolutions = { ...resolutions, [path]: value };
    persist();
  };
  const edit = (path, text) => {
    resolutions = { ...resolutions, [path]: { text } };
    cancelSave();
    timer = setTimeout(persist, 500);
  };
  const resultText = (item) => resolutions[item.path]?.text ?? item.ours ?? "";
  const picked = (path, side) => resolutions[path]?.keep === side;
  const keepSide = (item) => (item.oursId == null ? "theirs" : "ours");
  const resolved = $derived(items.length > 0 && items.every((item) => resolutions[item.path]));

  function closed() {
    if (timer) persist();
    onclose?.();
  }

  function close() {
    open = false;
    closed();
  }

  async function apply() {
    if (busy || !resolved) return;
    busy = true;
    error = "";
    cancelSave();
    try {
      const result = await applyResolution(ctx, current, resolutions);
      if (result.status === "conflict" && result.unresolved?.length) {
        error = `These files still need a choice: ${result.unresolved.join(", ")}`;
        return;
      }
      if (result.status === "conflict") {
        // The repository moved and the merge was made again.
        current = result.session;
        loaded = result.session;
        await load(result.session);
        error = "The repository changed, so the conflicts were found again. Check the choices and apply again.";
        return;
      }
      open = false;
      onresult?.(result);
    } catch (failure) {
      error = failure?.message || "Could not apply the resolution.";
    } finally {
      busy = false;
    }
  }
</script>

<Modal bind:open title="Resolve conflicts" wide wider cancelLabel="Close" onclose={closed}
       confirm={{ label: busy ? "Applying…" : "Apply", disabled: !resolved || busy, onclick: apply, oncancel: close }}>
  {#if loading}
    <p class="panel-muted" role="status">Loading…</p>
  {:else}
    <p class="panel-meta">The project does not change until every file is resolved and you apply the result.</p>
    <div class="gr-list">
      {#each items as item (item.path)}
        <section class="gr-item" aria-label={item.path}>
          <h4 class="panel-section-title">{item.path} {resolutions[item.path] ? "(resolved)" : "(needs a choice)"}</h4>
          {#if item.kind === "text"}
            <div class="gr-three">
              {#if item.base != null}
                <label class="gr-pane panel-meta">Base
                  <textarea class="textarea gr-text" readonly rows="6" value={item.base}></textarea>
                </label>
              {/if}
              {#if item.theirs != null}
                <label class="gr-pane panel-meta">The repository's
                  <textarea class="textarea gr-text" readonly rows="6" value={item.theirs}></textarea>
                </label>
              {/if}
            </div>
            {#if item.ours != null && item.theirs != null}
              <label class="gr-pane panel-meta">Result
                <textarea class="textarea gr-text" rows="8" value={resultText(item)}
                  oninput={(event) => edit(item.path, event.currentTarget.value)}></textarea>
              </label>
            {/if}
          {:else}
            <p class="panel-meta">This file is not text. Keep one side's copy.</p>
          {/if}
          <div class="gr-row">
            {#if deleteVsModify(item)}
              <button type="button" class="btn btn-sm {picked(item.path, keepSide(item)) ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
                onclick={() => choose(item.path, { keep: keepSide(item) })}>Keep the file</button>
              <button type="button" class="btn btn-sm {resolutions[item.path]?.delete ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
                onclick={() => choose(item.path, { delete: true })}>Delete the file</button>
            {:else}
              <button type="button" class="btn btn-sm {picked(item.path, 'ours') ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
                onclick={() => choose(item.path, { keep: "ours" })}>Use LibrePaper's</button>
              <button type="button" class="btn btn-sm {picked(item.path, 'theirs') ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
                onclick={() => choose(item.path, { keep: "theirs" })}>Use the repository's</button>
            {/if}
          </div>
        </section>
      {/each}
    </div>
  {/if}
  {#if error}<p class="lp-text-error" role="alert">{error}</p>{/if}
</Modal>

<style>
  .gr-list { display: grid; gap: calc(var(--spacing) * 4); overflow-y: auto; }
  .gr-item { display: grid; gap: calc(var(--spacing) * 2); }
  .gr-three { display: grid; grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr)); gap: calc(var(--spacing) * 2); }
  .gr-pane { display: grid; gap: var(--spacing); min-width: 0; }
  .gr-text { width: 100%; font-family: var(--font-mono, monospace); font-size: var(--panel-meta-size); }
  .gr-row { display: flex; gap: calc(var(--spacing) * 2); }
</style>
