<script>
  import PanelHeader from "../PanelHeader.svelte";
  import { provenanceSentence } from "../../lib/latex/status-text.js";

  let {
    diagnostics = [],
    main = "",
    canOpen = () => false,
    onopen,
    localAppProblem = false,
    onretrylocal,
    // The two LaTeX-only additions: what actually produced the current
    // preview, and every backend that was tried to get there. Both come
    // straight off the compile result Reader.svelte kept as
    // `lastLatexResult`; empty for every other format.
    // "Compiler errors remain in Diagnostics with source locations where
    // available. Preserve both attempts' logs when a browser failure led to
    // a local attempt."
    provenance = null,
    attempts = [],
  } = $props();
  const errors = $derived(diagnostics.filter((item) => item.severity !== "warning"));
  const warnings = $derived(diagnostics.filter((item) => item.severity === "warning"));
  // The attempts that were superseded by the one that produced this preview
  // -- everything but the last -- kept collapsed by default: useful when a
  // fallback happened, noise otherwise.
  const earlier = $derived(attempts.length > 1 ? attempts.slice(0, -1) : []);

  /// The last 20 log lines: both backends' logs are preserved in full on the
  /// result, but a panel is not a terminal, and the tail is where a TeX or
  /// Biber run says why it stopped.
  function tail(log) {
    return (log || "").trim().split("\n").slice(-20).join("\n");
  }
</script>

<section class="panel" aria-label="Warnings and errors">
  <PanelHeader title="Diagnostics" />
  {#if localAppProblem}
    <button type="button" class="btn btn-sm preset-tonal-primary mb-4" onclick={onretrylocal}>Reconnect and retry preview</button>
  {/if}
  {#if provenance}
    <details class="mb-4">
      <summary class="panel-section-title">Compiled with</summary>
      <p class="text-surface-700-300 text-sm mt-2">{provenanceSentence(provenance)}</p>
    </details>
  {/if}
  {#if diagnostics.length === 0}
    <p class="panel-muted">No warnings or errors.</p>
  {:else}
    {#each [{ title: "Errors", items: errors, warning: false }, { title: "Warnings", items: warnings, warning: true }] as group}
      {#if group.items.length}
        <h3 class="panel-section-title mb-2">{group.title} ({group.items.length})</h3>
        <ul class="space-y-3 mb-4">
          {#each group.items as item}
            <li class="rounded-container border border-surface-200-800 p-3">
              <span class="badge mb-2 {group.warning ? 'preset-tonal-warning' : 'preset-tonal-error'}">
                {group.warning ? "Warning" : "Error"}
              </span>
              <p class="whitespace-pre-wrap break-words">{item.message}</p>
              {#if item.file || item.line > 0}
                <div class="panel-meta mt-2 break-all">
                  {#if canOpen(item)}
                    <button type="button" class="anchor text-left" onclick={() => onopen?.(item)}
                            title="Go to this location in the source">
                      {item.file || main}:{item.line}{item.column > 0 ? `:${item.column}` : ""}
                    </button>
                  {:else}
                    <span>{item.file || main}{item.line > 0 ? `:${item.line}` : ""}{item.line > 0 && item.column > 0 ? `:${item.column}` : ""}</span>
                  {/if}
                </div>
              {/if}
              {#if item.hints?.length}
                <ul class="panel-muted mt-2 space-y-1">
                  {#each item.hints as hint}<li class="whitespace-pre-wrap break-words">{hint}</li>{/each}
                </ul>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    {/each}
  {/if}
  {#if earlier.length}
    <details>
      <summary class="panel-section-title">Earlier attempts ({earlier.length})</summary>
      <ul class="space-y-3 mt-2">
        {#each earlier as attempt, index (index)}
          <li class="rounded-container border border-surface-200-800 p-3">
            <p class="text-sm">
              <strong>{attempt.stage}</strong> ({attempt.backend}){attempt.reason ? ` — ${attempt.reason}` : ""}
            </p>
            {#if attempt.log}
              <pre class="text-surface-700-300 text-xs whitespace-pre-wrap break-words mt-2">{tail(attempt.log)}</pre>
            {/if}
          </li>
        {/each}
      </ul>
    </details>
  {/if}
</section>
