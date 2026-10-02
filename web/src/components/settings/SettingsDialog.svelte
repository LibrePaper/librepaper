<script>
  // The settings, as a preferences window rather than a form: a navigation
  // list of categories at the left, one category at a time at the right. It
  // opens from the workspace navbar or sidebar, and any entry point can open
  // it on a given category -- connection controls can land on Companion.
  import { tick } from "svelte";
  import Modal from "../Modal.svelte";
  import { offered, search, has } from "./registry.js";
  import SettingRow from "./SettingRow.svelte";
  import EditorSettings from "./EditorSettings.svelte";
  import LatexFilesSettings from "./LatexFilesSettings.svelte";
  import ProjectFolderSetting from "./ProjectFolderSetting.svelte";
  import QuotaSettings from "./QuotaSettings.svelte";
  import BuildSettings from "./BuildSettings.svelte";
  import IntegrationSettings from "./IntegrationSettings.svelte";
  import RenderingSettings from "./RenderingSettings.svelte";
  import LocalAppSettings from "./LocalAppSettings.svelte";
  import AccountSettings from "./AccountSettings.svelte";
  import RemoteSettings from "./RemoteSettings.svelte";
  import BackupsSettings from "./BackupsSettings.svelte";
  import { companion } from "../../lib/companion/status.svelte.js";
  import { capabilityFor } from "../../lib/build-catalog.js";

  let {
    open = $bindable(false),
    category = $bindable("editor"),
    sourceFormat = "",
    mayEdit = false,
    // The editor.
    keys = "default",
    onkeys,
    // What the workspace can currently be asked to do, for the shortcut table
    // the editor category shows. The reader assembles it; lib/commands.js
    // decides from it what is available.
    commands = {},
    // The build.
    buildPreferences = {},
    documentId = "",
    userId = "anonymous",
    onbuildpreferences,
    main = "",
    onbindingid,
    localExecution = false,
    onlocalexecution,
    onapplyoptions,
    // The account and the server.
    account = {},
    remoteConnected = false,
    remoteNote = "",
  } = $props();

  const context = $derived({ format: sourceFormat, mayEdit, signedIn: Boolean(account.provider), tool: buildPreferences.tool });
  const local = $derived(companion.status);
  $effect(() => companion.watch());
  const canBuildCurrentFormat = $derived(["latex", "typst", "markdown", "quarto"].includes(sourceFormat));
  const quartoOptionsRelevant = $derived(sourceFormat === "quarto" || (sourceFormat === "markdown" && buildPreferences.tool === "quarto"));
  const quartoExecutionRelevant = $derived(sourceFormat === "quarto" && mayEdit);
  const typstLocal = $derived(capabilityFor(local?.capabilities, "typst"));
  const available = $derived(offered(context));
  // The category shown: the one asked for, or the first offered when that is
  // not (the document changed format, or this browser lost the right to edit).
  const shown = $derived(available.find((each) => each.id === category) || available[0]);
  const row = (id) => Boolean(shown) && has(shown, id, context);

  let query = $state("");
  const found = $derived(search(query, context));
  // The navigation: every offered category or, while searching, only those
  // with a matching row.
  const nav = $derived(found ? found.map((match) => match.category) : available);
  const entriesOf = (id) => found?.find((match) => match.category.id === id)?.entries || [];

  let body = $state(null);
  async function go(id, entry = "") {
    category = id;
    if (!entry) return;
    await tick();
    body?.querySelector(`#${entry}`)?.scrollIntoView({ block: "start", behavior: "smooth" });
  }
</script>

<Modal bind:open title="Settings" full>
  <div class="settings">
    <nav class="settings-nav" aria-label="Settings categories">
      <input class="input input-sm settings-search" type="search" placeholder="Search settings" aria-label="Search settings" bind:value={query} />
      {#each nav as item (item.id)}
        <button type="button" class="settings-nav-item" class:current={shown?.id === item.id}
                aria-current={shown?.id === item.id ? "page" : undefined} onclick={() => go(item.id)}>{item.says}</button>
        {#each entriesOf(item.id) as entry (entry.id)}
          <button type="button" class="settings-nav-entry" onclick={() => go(item.id, entry.id)}>{entry.says}</button>
        {/each}
      {/each}
      {#if found && !nav.length}<p class="settings-nav-empty">Nothing matches.</p>{/if}
    </nav>

    <div class="settings-body" bind:this={body}>
      {#if shown}
        <header class="settings-head">
          <h3 class="settings-category">{shown.says}</h3>
          {#if shown.note}<span class="settings-scope">{shown.note}</span>{/if}
        </header>
        {#if shown.id === "editor"}
          <EditorSettings {keys} {onkeys} {commands} />
        {:else if shown.id === "render"}
          {#if canBuildCurrentFormat}
            <BuildSettings format={sourceFormat} {documentId} {userId} preferences={buildPreferences} onpreferences={onbuildpreferences} />
            {#if row("render-folder")}<ProjectFolderSetting {sourceFormat} {main} {mayEdit} {onbindingid} />{/if}
          {:else}
            <SettingRow id="render-tool" title="Build tool" description="Build choices apply to LaTeX, Typst, Markdown, and Quarto documents.">
              <span class="setting-description">No build tool applies to this document.</span>
            </SettingRow>
          {/if}

          <section class="settings-subsection">
            <h4 class="settings-subhead">LaTeX</h4>
            {#if sourceFormat !== "latex"}<p class="setting-description render-section-note">These downloaded files are browser-wide and can be cleared from any document.</p>{/if}
            <LatexFilesSettings />
          </section>

          <section class="settings-subsection">
            <h4 class="settings-subhead">Typst</h4>
            {#if sourceFormat !== "typst"}<p class="setting-description render-section-note">Typst settings apply when the current document is a Typst document.</p>{/if}
            <SettingRow id="typst-status" title="Browser and local tools" description="Typst can build in this browser. A local Typst builder may also be available when a companion is connected.">
              <span class="setting-description" role="status">{local?.state === "connected" ? (typstLocal?.available ? `Local Typst available${typstLocal.version ? ` (version ${typstLocal.version})` : ""}.` : "Typst is not available from the connected companion.") : ({ unknown: "Local companion has not been checked.", unreachable: "Local companion is disconnected.", denied: "Local companion access is unavailable.", unauthorized: "Connect this site to check local tools.", reachable: "Connect this site to check local tools.", incompatible: "Update the local companion to check local tools." })[local?.state] || "Local companion status unavailable."}</span>
            </SettingRow>
          </section>

          <section class="settings-subsection">
            <h4 class="settings-subhead">Quarto</h4>
            <IntegrationSettings name="quarto" />
            <RenderingSettings options={buildPreferences} {onapplyoptions} disabled={!quartoOptionsRelevant} />
            <SettingRow id="quarto-execution" title="Local code execution" description="Code runs on this computer with your user account's permissions.">
              <span class="setting-description">Allow paired Quarto documents to run local code</span>
              <button type="button" role="switch" class="switch local-execution-switch" aria-label="Allow paired Quarto documents to run local code" aria-checked={localExecution} data-state={localExecution ? "checked" : "unchecked"} disabled={!quartoExecutionRelevant} onclick={() => onlocalexecution?.(!localExecution)}>
                <span class="switch-thumb" data-state={localExecution ? "checked" : "unchecked"}></span>
              </button>
            </SettingRow>
            {#if !quartoExecutionRelevant}<p class="setting-description render-section-note">Local code execution can be changed from an editable Quarto document.</p>{/if}
          </section>

          <section class="settings-subsection">
            <h4 class="settings-subhead">Calepin</h4>
            {#if sourceFormat !== "typst"}<p class="setting-description render-section-note">Calepin builds Typst documents through the local companion.</p>{/if}
            <IntegrationSettings name="calepin" />
          </section>
        {:else if shown.id === "integrations"}
          <section class="settings-subsection">
            <h4 class="settings-subhead">Zotero</h4>
            <IntegrationSettings name="zotero" />
          </section>
        {:else if shown.id === "local"}
          <LocalAppSettings />
        {:else if shown.id === "backups"}
          <BackupsSettings {account} />
        {:else if shown.id === "account"}
          <RemoteSettings {remoteConnected} {remoteNote} />
          {#if row("storage-account")}<QuotaSettings />{/if}
          {#if row("account-erase")}<AccountSettings {account} />{/if}
        {/if}
      {/if}
    </div>
  </div>
</Modal>

<style>
  .render-section-note { margin-block: calc(var(--spacing) * 3); }
</style>
