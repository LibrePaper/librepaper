<script>
  import { tick } from "svelte";
  import DictationButton from "./DictationButton.svelte";
  import { textareaTarget } from "../lib/dictation/targets.js";
  let { placeholder = "Message…", disabled = false, canSend = !disabled, onsend, draft: controlledDraft = undefined, initialDraft = "", ondraft } = $props();
  let draft = $state("");
  let initialized = false;
  let sending = $state(false);
  let input;
  let height = $state(null);
  let resize = null;
  // Whether dictation is currently listening into *this* composer's
  // textarea, so the footer hint can say so (SPEC-dictation.md 4.8). The
  // textarea target itself carries the words into `draft` through the same
  // `oninput={update}` below -- `textareaTarget.insert` dispatches a
  // bubbling `input` event on the element, which is exactly what that
  // handler is already listening for, so dictation needs no extra wiring
  // into the draft/ondraft mechanics.
  let dictating = $state(false);

  function setHeight(value) {
    height = Math.max(80, Math.min(value, window.innerHeight * 0.6));
  }

  function startResize(event) {
    if (event.button !== 0) return;
    event.preventDefault();
    resize = { id: event.pointerId, y: event.clientY, height: input.getBoundingClientRect().height };
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function moveResize(event) {
    if (resize?.id === event.pointerId) setHeight(resize.height + resize.y - event.clientY);
  }

  function endResize(event) {
    if (resize?.id !== event.pointerId) return;
    resize = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  }

  function resizeKey(event) {
    if (!["ArrowUp", "ArrowDown"].includes(event.key)) return;
    event.preventDefault();
    setHeight(input.getBoundingClientRect().height + (event.key === "ArrowUp" ? 24 : -24));
  }

  $effect(() => {
    if (!initialized) {
      draft = controlledDraft === undefined ? initialDraft : controlledDraft;
      initialized = true;
    } else if (controlledDraft !== undefined && controlledDraft !== draft) draft = controlledDraft;
  });

  function update(event) {
    draft = event.currentTarget.value;
    ondraft?.(draft);
  }

  async function submit(event) {
    event.preventDefault();
    const submitted = draft;
    const text = submitted.trim();
    if (!text || disabled || !canSend || sending) return;
    sending = true;
    try {
      if (await onsend?.(text) && draft === submitted) {
        draft = "";
        ondraft?.(draft);
      }
    } finally {
      sending = false;
    }
  }

  function keydown(event) {
    if (event.key !== "Enter" || event.isComposing || event.keyCode === 229) return;
    if (event.shiftKey || event.ctrlKey) {
      event.preventDefault();
      const textarea = event.currentTarget;
      const at = textarea.selectionStart;
      const next = `${draft.slice(0, at)}\n${draft.slice(textarea.selectionEnd)}`;
      draft = next;
      ondraft?.(draft);
      void tick().then(() => textarea.setSelectionRange(at + 1, at + 1));
      return;
    }
    event.preventDefault();
    if (canSend && !disabled) event.currentTarget.form?.requestSubmit();
  }
</script>

<form class="chat-form" onsubmit={submit}>
  <div class="composer-input">
    <textarea bind:this={input} class="textarea" rows="4" style:height={height === null ? undefined : `${height}px`} value={draft} {placeholder} aria-label="Message"
      {disabled} required oninput={update} onkeydown={keydown}></textarea>
    <button type="button" class="resize-handle" aria-label="Resize message input" title="Drag up to expand · Arrow keys to resize"
      onpointerdown={startResize} onpointermove={moveResize} onpointerup={endResize}
      onpointercancel={endResize} onlostpointercapture={() => resize = null} onkeydown={resizeKey}>
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M3 2h7v7M5 2l5 5M8 2l2 2" /></svg>
    </button>
  </div>
  <div class="composer-footer">
    <span class="panel-meta">{dictating ? "Listening… click the microphone or press Escape to stop" : "Enter to send · Shift+Enter for a new line"}</span>
    <DictationButton target={() => textareaTarget(input)} label="Dictate" onlistening={(value) => (dictating = value)} />
    <button type="submit" class="btn preset-filled-primary-500" disabled={disabled || !canSend || sending || !draft.trim()}>Send</button>
  </div>
</form>
<style>
  .chat-form { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .composer-input { position: relative; }
  .composer-input textarea { display: block; width: 100%; min-height: 80px; max-height: 60dvh; resize: none; padding-right: 24px; }
  .resize-handle { position: absolute; top: 1px; right: 1px; width: 24px; height: 24px; display: grid; place-items: center; cursor: ns-resize; touch-action: none; color: var(--color-surface-500-500); border-radius: 3px; }
  .resize-handle:focus-visible { outline: 2px solid var(--color-primary-500); }
  .resize-handle svg { fill: none; stroke: currentColor; stroke-width: 1; }
  .composer-footer { display: flex; align-items: center; justify-content: space-between; gap: var(--spacing); flex-wrap: wrap; }
</style>
