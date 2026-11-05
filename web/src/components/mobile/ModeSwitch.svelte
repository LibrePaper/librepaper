<script>
  import { m } from "../../lib/paraglide/messages.js";

  // Preview or source, for the phone. The row is always 2.75rem tall so each
  // segment is a 44px target, but the visible pill is shorter and is drawn
  // inside it. When `collapsed`, the row slides out of view while nobody is
  // using it, which gives the document back its room; it comes back the moment
  // focus lands in it, so a keyboard or switch user is never left without it.
  let { mode = "document", collapsed = false, onchange = () => {} } = $props();
</script>

<div class="mobile-mode-switch" class:collapsed role="group" aria-label={m.mobilenav_mode_switch()}>
  <div class="pill">
    <button
      type="button"
      data-mode="document"
      aria-pressed={mode === "document"}
      onclick={() => onchange("document")}
    >
      <span class="segment" class:selected={mode === "document"}>{m.common_preview()}</span>
    </button>
    <button
      type="button"
      data-mode="source"
      aria-pressed={mode === "source"}
      onclick={() => onchange("source")}
    >
      <span class="segment" class:selected={mode === "source"}>{m.common_source()}</span>
    </button>
  </div>
</div>

<style>
  .mobile-mode-switch {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    height: 2.75rem;
    padding-inline: 0.5rem;
    border-bottom: 1px solid var(--color-pane-edge);
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

  .pill {
    display: flex;
    width: 100%;
    height: 2.25rem;
    padding: 0.125rem;
    border-radius: var(--radius-base);
    background: var(--color-subtle);
  }

  /* The button is 2.75rem tall, so it overhangs the 2.25rem pill by 0.25rem on
     each side. That overhang is the extra touch area. */
  button {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    height: 2.75rem;
    margin-block: -0.25rem;
    align-self: center;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    color: var(--color-text-secondary);
    font: inherit;
    cursor: pointer;
  }

  button:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: -2px;
  }

  .segment {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 2rem;
    border: 1px solid transparent;
    border-radius: var(--radius-base);
    font-size: 0.875rem;
    box-sizing: border-box;
  }

  .segment.selected {
    background: var(--color-raised);
    border-color: var(--color-pane-edge);
    color: var(--color-text);
  }
</style>
