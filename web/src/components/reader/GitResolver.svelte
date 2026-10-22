<script>
  // Resolves the conflicts a pull found. Nothing is written into the project
  // until every path is resolved and Apply succeeds. The session is kept by
  // sync.js, so closing this keeps it.
  import Modal from "../Modal.svelte";
  import { applyResolution } from "../../lib/gitsync/sync.js";

  let { open = $bindable(false), ctx, session = null, onresult, onclose } = $props();

  // Per path: { kind: "text" | "binary", base, ours, theirs (text or null), baseId, oursId, theirsId }.
  let items = $state([]);
  // Per path: { side: "ours" | "theirs" } | { delete: true } | { text }.
  let resolutions = $state({});
  let loading = $state(false);
  let busy = $state(false);
  let error = $state("");
  let loaded = null;

  const decoder = new TextDecoder("utf-8", { fatal: true });
  const bytesOf = (base64) => Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));

  function decode(base64) {
    if (base64 == null) return null;
    const bytes = bytesOf(base64);
    if (bytes.includes(0)) return undefined;
    try {
      return decoder.decode(bytes);
    } catch {
      return undefined;
    }
  }

  async function load(current) {
    loading = true;
    error = "";
    try {
      const conflicts = current.conflicts || [];
      const ids = [...new Set(conflicts.flatMap((item) => [item.base, item.ours, item.theirs]).filter(Boolean))];
      const answer = ids.length ? await ctx.git.blobs(ids) : {};
      const blobs = answer?.blobs || answer || {};
      const carried = new Set(current.carried || []);
      const next = [];
      const chosen = {};
      for (const item of conflicts) {
        const texts = [item.base, item.ours, item.theirs].map((id) => (id ? decode(blobs[id]) : null));
        const binary = texts.some((text) => text === undefined);
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
        if (carried.has(item.path) && current.resolutions?.[item.path]) chosen[item.path] = current.resolutions[item.path];
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
      load(session);
    }
  });

  const deleteVsModify = (item) => (item.oursId == null) !== (item.theirsId == null);
  const choose = (path, value) => (resolutions = { ...resolutions, [path]: value });
  const edit = (path, text) => choose(path, { text });
  const resultText = (item) => resolutions[item.path]?.text ?? item.ours ?? "";
  const picked = (path, side) => resolutions[path]?.side === side;
  const resolved = $derived(items.length > 0 && items.every((item) => resolutions[item.path]));

  function close() {
    open = false;
    onclose?.();
  }

  async function apply() {
    if (busy || !resolved) return;
    busy = true;
    error = "";
    try {
      const result = await applyResolution(ctx, session, resolutions);
      if (result.status === "conflict") {
        session = result.session;
        loaded = null;
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

<Modal bind:open title="Resolve conflicts" wide wider cancelLabel="Close" onclose={onclose}
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
              <button type="button" class="btn btn-sm {resolutions[item.path]?.delete ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
                onclick={() => choose(item.path, { side: item.oursId == null ? "theirs" : "ours" })}>Keep the file</button>
              <button type="button" class="btn btn-sm {resolutions[item.path]?.delete ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
                onclick={() => choose(item.path, { delete: true })}>Delete the file</button>
            {:else}
              <button type="button" class="btn btn-sm {picked(item.path, 'ours') ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
                onclick={() => choose(item.path, { side: "ours" })}>Use LibrePaper's</button>
              <button type="button" class="btn btn-sm {picked(item.path, 'theirs') ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
                onclick={() => choose(item.path, { side: "theirs" })}>Use the repository's</button>
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
