<script>
  import { m } from "../../lib/paraglide/messages.js";

  // Preview or source, for the phone. It reads as the underline tab strip the
  // side panels use: each button is a full-height tab, and the selected one
  // gets a primary-coloured underline. When `collapsed`, the row slides out of
  // view while nobody is using it, which gives the document back its room; it
  // comes back the moment focus lands in it, so a keyboard or switch user is
  // never left without it.
  let { mode = "document", collapsed = false, onchange = () => {} } = $props();
</script>

<div class="mobile-mode-switch" class:collapsed role="group" aria-label={m.mobilenav_mode_switch()}>
  <button
    type="button"
    data-mode="document"
    aria-pressed={mode === "document"}
    onclick={() => onchange("document")}
  >
    {m.common_preview()}
  </button>
  <button
    type="button"
    data-mode="source"
    aria-pressed={mode === "source"}
    onclick={() => onchange("source")}
  >
    {m.common_source()}
  </button>
</div>

<style>
  .mobile-mode-switch {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: stretch;
    height: 2.75rem;
    border-bottom: 1px solid var(--color-divider);
    background: var(--color-shell);
  }

  .mobile-mode-switch.collapsed:not(:focus-within) {
    margin-top: -2.75rem;
    visibility: hidden;
  }

  @media (prefers-reduced-motion: no-preference) {
    .mobile-mode-switch {
      transition: margin-top 180ms ease, visibility 180ms ease;
    }
  }

  button {
    flex: 1 1 0;
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 0;
    padding: 0;
    border: 0;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--color-text-secondary);
    font: inherit;
    font-size: 0.875rem;
    white-space: nowrap;
    cursor: pointer;
  }

  button[aria-pressed="true"] {
    color: var(--color-link);
    border-bottom-color: var(--color-link);
  }

  button:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: -2px;
  }
</style>
