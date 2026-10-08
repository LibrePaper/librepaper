<script>
  // The settings, as a preferences window rather than a form: a navigation
  // list of categories at the left, one category at a time at the right. It
  // opens from the workspace navbar or sidebar, and any entry point can open
  // it on a given category -- connection controls can land on Tools.
  import { onMount, tick } from "svelte";
  import * as companionControl from "../../lib/companion/control.js";
  import { createMachineView } from "../../lib/companion/machine.svelte.js";
  import Modal from "../Modal.svelte";
  import { offered, search, has } from "./registry.js";
  import EditorSettings from "./EditorSettings.svelte";
  import CompanionStatus from "./CompanionStatus.svelte";
  import LatexFilesSettings from "./LatexFilesSettings.svelte";
  import QuotaSettings from "./QuotaSettings.svelte";
  import BuildSettings from "./BuildSettings.svelte";
  import RenderingSettings from "./RenderingSettings.svelte";
  import ToolsSettings from "./ToolsSettings.svelte";
  import AccountSettings from "./AccountSettings.svelte";
  import RemoteSettings from "./RemoteSettings.svelte";
  import BackupsSettings from "./BackupsSettings.svelte";
  import AgentSettings from "./AgentSettings.svelte";
  import DiagnosticsSettings from "./DiagnosticsSettings.svelte";

  let {
    open = $bindable(false),
    category = $bindable("editor"),
    sourceFormat = "",
    mayEdit = false,
    // The editor.
    keys = "default",
    onkeys = undefined,
    // What the workspace can currently be asked to do, for the shortcut table
    // the editor category shows. The reader assembles it; lib/commands.js
    // decides from it what is available.
    commands = {},
    // The renderers, chosen per format for every document in this browser.
    userId = "anonymous",
    onbuildpreferences = undefined,
    onquartooptions = undefined,
    // The account and the server.
    account = {},
    remoteConnected = false,
    remoteNote = "",
  } = $props();

  const view = createMachineView();
  onMount(() => view.start());

  const context = $derived({ format: sourceFormat, mayEdit, signedIn: Boolean(account.provider) });
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

  const approvals = $derived(view.list(view.state?.approvals).length);

  let body = $state(null);
  $effect(() => companionControl.onSettingsRequested(() => {
    category = "tools";
    open = true;
  }));
  async function go(id, entry = "") {
    category = id;
    if (!entry) return;
    await tick();
    body?.querySelector(`#${entry}`)?.scrollIntoView({ block: "start", behavior: "smooth" });
  }
</script>

<style>
  .settings-nav-item.separated {
    border-top: 1px solid var(--color-divider);
    margin-top: calc(var(--spacing) * 3);
    padding-top: calc(var(--spacing) * 3);
  }
  .settings-nav-badge {
    display: inline-flex;
    min-width: 1.25rem;
    padding: 0 0.35rem;
    border-radius: 999px;
    background: var(--color-warning-solid);
    color: white;
    font-size: var(--text-xs);
    margin-inline-start: 0.5rem;
  }
</style>

<Modal bind:open title="Settings" full>
  <div class="settings">
    <nav class="settings-nav" aria-label="Settings categories">
      <input class="input input-sm settings-search" type="search" placeholder="Search settings" aria-label="Search settings" bind:value={query} />
      {#each nav as item (item.id)}
        <button type="button" class="settings-nav-item" class:current={shown?.id === item.id} class:separated={item.separated}
                aria-current={shown?.id === item.id ? "page" : undefined} onclick={() => go(item.id)}>
          {item.says}
          {#if item.id === "tools" && approvals > 0}
            <span class="settings-nav-badge" aria-label={`${approvals} waiting for your answer`}>{approvals}</span>
          {/if}
        </button>
        {#each entriesOf(item.id) as entry (entry.id)}
          <button type="button" class="settings-nav-entry" onclick={() => go(item.id, entry.id)}>{entry.says}</button>
        {/each}
      {/each}
      {#if found && !nav.length}<p class="settings-nav-empty">Nothing matches.</p>{/if}
      <div class="settings-nav-companion"><CompanionStatus id="settings-companion" /></div>
    </nav>

    <div class="settings-body" bind:this={body}>
      {#if shown}
        <header class="settings-head">
          <h3 class="settings-category">{shown.says}</h3>
          {#if shown.note}<span class="settings-scope">{shown.note}</span>{/if}
        </header>
        {#if shown.id === "editor"}
          <EditorSettings {keys} {onkeys} {commands} />
        {:else if shown.id === "rendering"}
          <section class="settings-subsection">
            <div class="settings-section-title"><h4 class="settings-subhead">LaTeX</h4></div>
            <BuildSettings format="latex" {userId} onpreferences={onbuildpreferences} />
            <LatexFilesSettings />
          </section>

          <section class="settings-subsection">
            <div class="settings-section-title"><h4 class="settings-subhead">Markdown and Quarto</h4></div>
            <BuildSettings format="markdown" {userId} onpreferences={onbuildpreferences} ontools={() => go("tools")} />
            <RenderingSettings {userId} {onquartooptions} />
          </section>
        {:else if shown.id === "tools"}
          <ToolsSettings {view} />
        {:else if shown.id === "agents"}
          <AgentSettings {view} />
        {:else if shown.id === "backups"}
          <BackupsSettings {account} />
        {:else if shown.id === "account"}
          <RemoteSettings {remoteConnected} {remoteNote} />
          {#if row("storage-account")}<QuotaSettings />{/if}
          {#if row("account-erase")}<AccountSettings {account} />{/if}
        {:else if shown.id === "diagnostics"}
          <DiagnosticsSettings {view} />
        {/if}
      {/if}
    </div>
  </div>
</Modal>
