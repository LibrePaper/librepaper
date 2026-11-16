<script>
  import { SHELL_HEADERS } from "../../lib/api.js";
  import { m } from "../../lib/paraglide/messages.js";
  import { say } from "../../lib/toast.svelte.js";
  import Modal from "../Modal.svelte";

  let { slug, title: documentTitle } = $props();

  let savingTemplate = $state(false);
  let templateModalOpen = $state(false);
  let templateName = $state("");
  let templateError = $state("");
  let templateSaveKey = "";
  let templateSaveRequestId = "";

  export function open() {
    templateName = documentTitle || "";
    templateError = "";
    templateSaveKey = "";
    templateSaveRequestId = "";
    templateModalOpen = true;
  }

  async function saveTemplate() {
    const title = templateName.trim();
    if (!title) {
      templateError = m.readerc_template_name_required();
      return;
    }
    const key = `librepaper:template-save:${slug}:${title}`;
    if (templateSaveKey !== key || !templateSaveRequestId) {
      templateSaveKey = key;
      try {
        templateSaveRequestId = sessionStorage.getItem(key) || "";
      } catch {
        templateSaveRequestId = "";
      }
      if (!templateSaveRequestId) {
        templateSaveRequestId = crypto.randomUUID();
        try { sessionStorage.setItem(key, templateSaveRequestId); } catch { /* in-memory retries still reuse it */ }
      }
    }
    savingTemplate = true;
    try {
      const response = await fetch(`/api/documents/${slug}/template`, {
        method: "POST",
        headers: { ...SHELL_HEADERS, "Content-Type": "application/json" },
        body: JSON.stringify({ title, request_id: templateSaveRequestId }),
      });
      if (!response.ok) {
        templateError = (await response.json().catch(() => ({}))).error || m.readerc_template_save_failed();
        return;
      }
      try { sessionStorage.removeItem(key); } catch { /* no persistent key was available */ }
      templateSaveKey = "";
      templateSaveRequestId = "";
      templateModalOpen = false;
      say(m.readerc_template_saved({ title }), { id: "reader:saved-template" });
    } catch (error) {
      templateError = error?.message || m.readerc_template_save_failed();
    } finally {
      savingTemplate = false;
    }
  }
</script>

<Modal bind:open={templateModalOpen} title={m.readerd_save_as_template()}
  onclose={() => { templateError = ""; templateName = ""; }}
  confirm={{ label: savingTemplate ? m.common_saving() : m.common_save(), disabled: savingTemplate, onclick: saveTemplate }}>
  {#snippet children()}
    <label class="label">
      <span class="label-text">{m.readerd_template_name()}</span>
      <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
      <input class="input" autofocus bind:value={templateName} />
    </label>
    {#if templateError}<p class="lp-text-error text-sm">{templateError}</p>{/if}
  {/snippet}
</Modal>
