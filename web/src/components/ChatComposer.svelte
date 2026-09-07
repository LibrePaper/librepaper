<script>
  import { tick } from "svelte";
  let { placeholder = "Message…", disabled = false, onsend } = $props();
  let draft = $state("");
  let sending = $state(false);
  async function submit(event) {
    event.preventDefault();
    const submitted = draft;
    const text = submitted.trim();
    if (!text || disabled || sending) return;
    sending = true;
    try {
      if (await onsend?.(text) && draft === submitted) draft = "";
    } finally {
      sending = false;
    }
  }
  function keydown(event) {
    if (event.key !== "Enter" || event.isComposing || event.keyCode === 229) return;
    if (event.ctrlKey) {
      event.preventDefault();
      const textarea = event.currentTarget;
      const at = textarea.selectionStart;
      draft = `${draft.slice(0, at)}\n${draft.slice(textarea.selectionEnd)}`;
      void tick().then(() => textarea.setSelectionRange(at + 1, at + 1));
      return;
    }
    event.preventDefault(); event.currentTarget.form?.requestSubmit();
  }
</script>
<form class="chat-form" onsubmit={submit}>
  <label class="label">Message
    <textarea class="textarea" rows="4" bind:value={draft} {placeholder} aria-label="Message" disabled={disabled || sending} required onkeydown={keydown}></textarea>
  </label>
  <button class="btn preset-filled-primary-500" disabled={disabled || sending || !draft.trim()}>Send</button>
</form>
<style>.chat-form { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }</style>
