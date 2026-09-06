<script>
  import PanelHeader from "./PanelHeader.svelte";

  let { diagnostics = [], main = "", canOpen = () => false, onopen } = $props();
  const errors = $derived(diagnostics.filter((item) => item.severity !== "warning"));
  const warnings = $derived(diagnostics.filter((item) => item.severity === "warning"));
</script>

<section class="panel" aria-label="Warnings and errors">
  <PanelHeader title="Diagnostics" />
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
</section>
