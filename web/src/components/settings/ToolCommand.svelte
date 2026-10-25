<script>
  // The command that renders one output of one format on this computer: the
  // whole command on one line, the file it leaves behind, and whether the
  // program was found. Every change goes through the desktop app's own
  // confirmation dialog, which shows the full command.
  import SettingRow from "./SettingRow.svelte";
  import StatusPill from "./StatusPill.svelte";
  import * as desktopClient from "../../lib/desktop/client.js";
  import { splitArgs, joinArgs } from "../../lib/desktop/args.js";

  let { format, output } = $props();

  const TITLES = { pdf: "PDF command", html: "HTML command", docx: "Word command" };
  const HELP = "{input} is the main file and {stem} is its path without the extension. The command runs in the project folder.";

  let settingsError = $state("");
  let pendingDialogAction = $state(false);
  let editingCommand = $state("");
  let editingResult = $state("");
  let record = $state(null);
  let savedFeedback = $state(false);
  let loadId = 0;

  const key = $derived(desktopClient.rendererKey(format, output));
  const isConnected = Boolean(desktopClient.desktop());

  $effect(() => {
    const requestedKey = key;
    const requestId = ++loadId;
    record = null;
    pendingDialogAction = false;
    settingsError = "";
    savedFeedback = false;
    if (isConnected) void loadSettings(requestId, requestedKey);
  });

  async function loadSettings(requestId, requestedKey) {
    try {
      const loaded = (await desktopClient.manage("/state"))?.settings?.renderers?.[requestedKey];
      if (requestId !== loadId || requestedKey !== key) return;
      if (loaded) show(loaded);
      else settingsError = "The desktop app has no command for this output.";
    } catch (error) {
      if (requestId === loadId && requestedKey === key) {
        settingsError = error?.message || "Could not load the desktop app settings.";
      }
    }
  }

  // What the desktop app holds, and the inputs reset to it.
  function show(loaded) {
    record = loaded;
    editingCommand = joinArgs(loaded.command);
    editingResult = loaded.result || "";
  }

  async function apply(change) {
    if (!canEdit) return;
    pendingDialogAction = true;
    savedFeedback = false;
    const requestId = loadId;
    const requestedKey = key;
    try {
      await change(requestedKey);
      if (requestId === loadId) {
        settingsError = "";
        savedFeedback = true;
        await loadSettings(requestId, requestedKey);
      }
    } catch (error) {
      if (requestId === loadId) {
        settingsError = error?.name === "Refused" && error?.status === 403
          ? "The command was not changed."
          : error?.message || "Could not change the command.";
      }
    } finally {
      if (requestId === loadId) pendingDialogAction = false;
    }
  }

  const save = () => apply((requestedKey) => desktopClient.setRenderer(requestedKey, { command: splitArgs(editingCommand), result: editingResult.trim() }));
  const reset = () => apply((requestedKey) => desktopClient.resetRenderer(requestedKey));

  const canEdit = $derived(isConnected && record !== null && !pendingDialogAction);
  const unchanged = $derived(record !== null && joinArgs(splitArgs(editingCommand)) === joinArgs(record.command) && editingResult.trim() === record.result);
  const invalid = $derived(!splitArgs(editingCommand).length || !editingResult.trim());
  const status = $derived(record === null ? null
    : record.found ? { label: record.version ? `Found · ${record.version}` : "Found", tone: "good" }
    : { label: "Not found", tone: "warn" });
</script>

<SettingRow id={`render-${format}-${output}`} title={TITLES[output] || `${output} command`} description={HELP} stacked>
  <div class="tool-command">
    {#if status}<StatusPill label={status.label} tone={status.tone} />{/if}
    <input class="input input-sm setting-input" type="text" spellcheck="false" aria-label={`${TITLES[output] || output} for ${format}`}
           bind:value={editingCommand} oninput={() => savedFeedback = false} disabled={!canEdit} />
    <label class="tool-command-result">
      <span class="setting-description">Result file</span>
      <input class="input input-sm setting-input" type="text" spellcheck="false" aria-label={`Result file for ${format} ${output}`}
             bind:value={editingResult} oninput={() => savedFeedback = false} disabled={!canEdit} />
    </label>
    {#if settingsError}<p class="setting-description tool-command-error" role="alert">{settingsError}</p>{/if}
    <div class="setting-actions tool-command-actions">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={!canEdit || unchanged || invalid} onclick={() => void save()}>{pendingDialogAction ? "Saving…" : "Save"}</button>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={!canEdit || !record?.edited} onclick={() => void reset()}>Reset to default</button>
      {#if savedFeedback}<span class="setting-feedback" role="status">Saved</span>{/if}
    </div>
    {#if pendingDialogAction}
      <p class="setting-description">Confirm the change in the dialog on this computer.</p>
    {/if}
  </div>
</SettingRow>

<style>
  .tool-command { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); align-items: stretch; }
  .tool-command-result { display: flex; flex-direction: column; gap: calc(var(--spacing) * 1); }
  .tool-command-error { color: var(--color-error-text); }
  .tool-command-actions { display: flex; gap: calc(var(--spacing) * 2); }
</style>
