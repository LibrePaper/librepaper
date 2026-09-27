<script>
  import { tick } from "svelte";
  let {
    placeholder = "Message…",
    disabled = false,
    canSend = !disabled,
    onsend,
    draft: controlledDraft = undefined,
    initialDraft = "",
    ondraft,
    onstop = null,
    stopLabel = "Stop",
    status = "",
  } = $props();
  let draft = $state("");
  let initialized = false;
  let sending = $state(false);
  let input;

  // Auto-grow the textarea with its content: starts at about two lines,
  // grows as the draft grows, and caps at roughly 40% of the viewport
  // (about ten lines) where it scrolls instead of growing further. A hidden
  // textarea (a background tab) measures zero, so it keeps its rows until it
  // is shown; the observer below measures it again then, and whenever its
  // width changes, and the window listener keeps the cap current.
  let measuredWidth = 0;
  function autogrow() {
    if (!input) return;
    measuredWidth = input.clientWidth;
    input.style.height = "";
    if (!input.scrollHeight) return;
    const cap = window.innerHeight * 0.4;
    input.style.height = `${Math.min(input.scrollHeight, cap)}px`;
  }

  $effect(() => {
    const observer = new ResizeObserver(() => { if (input.clientWidth !== measuredWidth) autogrow(); });
    observer.observe(input);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (!initialized) {
      draft = controlledDraft === undefined ? initialDraft : controlledDraft;
      initialized = true;
    } else if (controlledDraft !== undefined && controlledDraft !== draft) draft = controlledDraft;
  });

  $effect(() => {
    draft;
    void tick().then(autogrow);
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

<svelte:window onresize={autogrow} />
<form class="chat-form" onsubmit={submit} data-cansend={canSend && !disabled && !sending}>
  <div class="composer-input">
    <textarea bind:this={input} class="textarea" rows="2" value={draft} {placeholder} aria-label="Message" title="Enter to send · Shift+Enter for a new line"
      {disabled} required oninput={update} onkeydown={keydown}></textarea>
  </div>
  <div class="composer-actions">
    <span class="panel-meta">Enter to send · Shift+Enter for a new line</span>
    <div class="composer-buttons">
      {#if status}<span class="panel-meta" role="status">{status}</span>{/if}
      {#if onstop}<button class="btn btn-sm lp-control-outline" type="button" onclick={() => onstop()}>{stopLabel}</button>{/if}
      <button class="btn btn-sm lp-control-brand" type="submit"
              disabled={disabled || !canSend || sending || !draft.trim()}>
        {sending ? "Sending…" : "Send"}
      </button>
    </div>
  </div>
</form>
<style>
  .chat-form { display: flex; flex-direction: column; gap: var(--spacing); }
  .composer-actions { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 2); }
  .composer-buttons { display: flex; align-items: center; gap: var(--spacing); }
  .composer-input textarea { display: block; width: 100%; resize: none; overflow-y: auto; }
</style>
