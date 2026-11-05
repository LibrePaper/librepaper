<script>
  import Icon from "../Icon.svelte";
  import { m } from "../../lib/paraglide/messages.js";

  // The row that rides on top of the on-screen keyboard while editing. It is
  // fixed to the keyboard's top edge by --librepaper-keyboard, so it stays
  // with the keys as they open and close.
  let { onundo = () => {}, onredo = () => {}, oninsert = () => {}, onmore = () => {} } = $props();

  // A tap on these buttons must not take the focus from the editor, or the
  // keyboard closes under the finger. The mousedown and pointerdown are both
  // cancelled, as FormatToolbar does.
  const keepFocus = (event) => event.preventDefault();
</script>

<div class="mobile-keyboard-bar" role="toolbar" aria-label={m.mobilenav_editing_tools()}>
  <button type="button" data-action="undo" aria-label={m.common_undo()} title={m.common_undo()}
          onmousedown={keepFocus} onpointerdown={keepFocus} onclick={onundo}>
    <Icon name="undo-2" size={20} />
  </button>
  <button type="button" data-action="redo" aria-label={m.common_redo()} title={m.common_redo()}
          onmousedown={keepFocus} onpointerdown={keepFocus} onclick={onredo}>
    <Icon name="redo-2" size={20} />
  </button>
  <button type="button" data-action="insert" aria-label={m.common_insert()} title={m.common_insert()}
          onmousedown={keepFocus} onpointerdown={keepFocus} onclick={oninsert}>
    <Icon name="plus" size={20} />
  </button>
  <button type="button" data-action="more" aria-label={m.mobilenav_more()} title={m.mobilenav_more()}
          onmousedown={keepFocus} onpointerdown={keepFocus} onclick={onmore}>
    <Icon name="ellipsis" size={20} />
  </button>
</div>

<style>
  .mobile-keyboard-bar {
    position: fixed;
    left: 0;
    right: 0;
    bottom: var(--librepaper-keyboard, 0px);
    z-index: 20;
    display: flex;
    height: 2.75rem;
    border-top: 1px solid var(--color-pane-edge);
    background: var(--color-shell);
  }

  button {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    color: var(--color-text-secondary);
    cursor: pointer;
  }

  button:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: -2px;
  }
</style>
