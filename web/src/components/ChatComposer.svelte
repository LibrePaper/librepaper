<script>
  import { tick } from "svelte";
  let { placeholder = "Message…", disabled = false, canSend = !disabled, onsend, draft: controlledDraft = undefined, initialDraft = "", ondraft } = $props();
  let draft = $state("");
  let initialized = false;
  let sending = $state(false);

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
  <label class="label">Message
    <textarea class="textarea" rows="4" value={draft} {placeholder} aria-label="Message"
      {disabled} required oninput={update} onkeydown={keydown}></textarea>
  </label>
  <div class="composer-footer">
    <span class="panel-meta">Enter to send · Shift+Enter for a new line</span>
    <button class="btn preset-filled-primary-500" disabled={disabled || !canSend || sending || !draft.trim()}>Send</button>
  </div>
</form>
<style>
  .chat-form { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .composer-footer { display: flex; align-items: center; justify-content: space-between; gap: var(--spacing); flex-wrap: wrap; }
</style>
