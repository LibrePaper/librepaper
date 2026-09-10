<script>
  // SPEC-dictation.md 4.8 "Status pill": the shortcut has no button of its
  // own to show feedback on, and a dictation started in a panel that later
  // scrolls out of view still needs somewhere to be stopped from. This is
  // that somewhere -- fixed to the window, not to any pane, so it survives
  // both.
  import { onDestroy } from "svelte";
  import { getDictation } from "../lib/dictation/service.js";
  import IconButton from "./IconButton.svelte";

  let snapshot = $state({ state: "idle", progress: null, model: null, device: null, reason: null, speaking: false });

  const unsubscribe = getDictation().subscribe((value) => { snapshot = value; });
  onDestroy(unsubscribe);

  const visible = $derived(snapshot.state !== "idle" && snapshot.state !== "unavailable");
  const percent = $derived(
    snapshot.progress?.total ? Math.min(100, Math.round((snapshot.progress.loaded / snapshot.progress.total) * 100)) : 0,
  );
</script>

{#if visible}
  <div class="card preset-filled-surface-100-900 dictation-pill fixed bottom-4 left-1/2 -translate-x-1/2 z-40 flex items-center gap-3 px-4 py-2 shadow-lg">
    {#if snapshot.state === "loading"}
      <span class="panel-meta" title={snapshot.progress?.file || ""}>{snapshot.model?.label || "Dictation"}</span>
      <div class="dictation-pill-track">
        <div class="dictation-pill-fill" style="width: {percent}%"></div>
      </div>
    {:else if snapshot.state === "listening"}
      <span class="dictation-pill-dot animate-pulse"></span>
      <span>Listening</span>
    {:else if snapshot.state === "transcribing"}
      <span>Transcribing…</span>
    {/if}
    <IconButton icon="x" label="Stop dictation" tone="plain" size="btn-icon-sm" onclick={() => getDictation().stop()} />
  </div>
{/if}

<style>
  /* The pill's own furniture: a small progress bar and the listening dot,
     neither of which exists as a Skeleton preset. Colours still come from
     the primary token, never written down. */
  .dictation-pill-track {
    width: 6rem;
    height: 0.375rem;
    border-radius: var(--radius-container);
    background: var(--color-surface-300-700);
    overflow: hidden;
  }
  .dictation-pill-fill {
    height: 100%;
    background: var(--color-primary-500);
  }
  .dictation-pill-dot {
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--color-primary-500);
  }
</style>
