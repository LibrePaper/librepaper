<script>
  import Icon from "../Icon.svelte";

  // A full-area list of commands that takes over the phone's reading area,
  // such as the editor's menu. It only reports the id of the row chosen; the
  // parent decides what that does and when this view goes away.
  let { groups = [], onchoose = () => {} } = $props();
</script>

<div class="mobile-command-view">
  {#each groups as group, index (index)}
    {#if index > 0}<div class="mobile-command-divider" aria-hidden="true"></div>{/if}
    {#if group.label}
      <div class="mobile-command-label">{group.label}</div>
    {/if}
    {#each group.items as item (item.id)}
      <button
        type="button"
        class="mobile-command-row"
        data-command={item.id}
        disabled={item.disabled}
        aria-pressed={item.checked}
        onclick={() => onchoose(item.id)}
      >
        <span class="mobile-command-text">
          <span class="mobile-command-name">{item.label}</span>
          {#if item.reason}<span class="mobile-command-reason">{item.reason}</span>{/if}
        </span>
        {#if item.checked === true}
          <span class="mobile-command-check"><Icon name="check" size={20} /></span>
        {/if}
      </button>
    {/each}
  {/each}
</div>

<style>
  .mobile-command-view {
    position: absolute;
    inset: 0;
    z-index: 5;
    overflow-y: auto;
    padding-bottom: env(safe-area-inset-bottom);
    background: var(--color-shell);
  }

  .mobile-command-label {
    padding: 0.75rem 1rem 0.25rem;
    font-size: 0.75rem;
    color: var(--color-text-secondary);
  }

  .mobile-command-divider {
    height: 1px;
    margin-block: 0.25rem;
    background: var(--color-divider);
  }

  .mobile-command-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    width: 100%;
    min-height: 2.75rem;
    padding-inline: 1rem;
    border: 0;
    background: none;
    color: var(--color-text);
    font: inherit;
    font-size: 0.875rem;
    text-align: start;
    cursor: pointer;
  }

  .mobile-command-row:hover,
  .mobile-command-row:active {
    background: var(--color-row-hover);
  }

  .mobile-command-row:disabled {
    opacity: 0.5;
    cursor: default;
    background: none;
  }

  .mobile-command-row:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: -2px;
  }

  .mobile-command-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }

  .mobile-command-reason {
    font-size: 0.75rem;
    color: var(--color-text-secondary);
  }

  .mobile-command-check {
    display: flex;
    flex: none;
    color: var(--color-brand);
  }
</style>
