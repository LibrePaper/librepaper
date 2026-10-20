<script>
  import { tick } from "svelte";
  import IconButton from "./IconButton.svelte";
  let {
    placeholder = "Message…",
    disabled = false,
    canSend = !disabled,
    onsend,
    draft: controlledDraft = undefined,
    initialDraft = "",
    ondraft = undefined,
    onstop = null,
    stopLabel = "Stop",
  } = $props();
  let draft = $state("");
  let initialized = false;
  let sending = $state(false);
  let input;

  // Auto-grow the textarea with its content, from one line up to five lines,
  // then scroll inside the box. The cap is read from the textarea's own line
  // height and padding. A hidden textarea (a background tab) measures zero,
  // so it keeps its rows until it is shown; the observer below measures it
  // again then, and whenever its width changes.
  let measuredWidth = 0;
  function autogrow() {
    if (!input) return;
    measuredWidth = input.clientWidth;
    input.style.height = "";
    if (!input.scrollHeight) return;
    const style = getComputedStyle(input);
    const fontSize = parseFloat(style.fontSize);
    const lineHeight = style.lineHeight === "normal" ? fontSize * 1.4 : parseFloat(style.lineHeight);
    const cap = lineHeight * 5 + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
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
        input?.focus();
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

<form class="chat-form" onsubmit={submit} data-cansend={canSend && !disabled && !sending}>
  <div class="composer-box">
    <textarea bind:this={input} rows="1" value={draft} {placeholder} aria-label="Message"
      {disabled} required oninput={update} onkeydown={keydown}></textarea>
    <div class="composer-buttons">
      {#if onstop}
        <IconButton icon="square" label={stopLabel} tone="plain" size="btn-icon-sm" onclick={() => onstop()} />
      {/if}
      <IconButton type="submit" icon="send-horizontal" label="Send message" title="Send message. Shift+Enter adds a new line." tone="plain" size="btn-icon-sm"
        disabled={disabled || !canSend || sending || !draft.trim()} />
    </div>
  </div>
</form>
<style>
  .chat-form { display: flex; flex-direction: column; padding-bottom: env(safe-area-inset-bottom, 0px); }
  .composer-box {
    display: flex;
    align-items: flex-end;
    border: 1px solid var(--color-border);
    border-radius: calc(var(--radius-base) * 2);
  }
  .composer-box:focus-within { border-color: var(--color-brand); }
  .composer-box textarea {
    flex: 1;
    min-width: 0;
    display: block;
    border: 0;
    outline: none;
    background: transparent;
    font: inherit;
    color: inherit;
    resize: none;
    overflow-y: auto;
    padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 2);
  }
  .composer-buttons {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 0.5);
    align-self: flex-end;
    padding: calc(var(--spacing) * 1) calc(var(--spacing) * 1.5) calc(var(--spacing) * 1) 0;
  }
  @media (pointer: coarse) {
    .composer-buttons :global(.btn-icon) { min-width: 44px; min-height: 44px; }
  }
</style>
