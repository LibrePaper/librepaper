<script>
  // The settings, as a preferences window rather than a form: a navigation
  // list of categories at the left, one category at a time at the right. It
  // opens from the workspace navbar or sidebar, and any entry point can open
  // it on a given category -- connection controls can land on Companion.
  import { tick } from "svelte";
  import * as companionControl from "../../lib/companion/control.js";
  import Modal from "../Modal.svelte";
  import { offered, search, has } from "./registry.js";
  import EditorSettings from "./EditorSettings.svelte";
  import LatexFilesSettings from "./LatexFilesSettings.svelte";
  import QuotaSettings from "./QuotaSettings.svelte";
  import BuildSettings from "./BuildSettings.svelte";
  import CompanionBlock from "./CompanionBlock.svelte";
  import IntegrationSettings from "./IntegrationSettings.svelte";
  import RenderingSettings from "./RenderingSettings.svelte";
  import LocalAppSettings from "./LocalAppSettings.svelte";
  import AccountSettings from "./AccountSettings.svelte";
  import RemoteSettings from "./RemoteSettings.svelte";
  import BackupsSettings from "./BackupsSettings.svelte";
  import CompanionTools from "./CompanionTools.svelte";

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

  let body = $state(null);
  $effect(() => companionControl.onSettingsRequested(() => {
    category = "local";
    open = true;
  }));
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
          <CompanionBlock id="render-local" needs="Calepin, Pandoc and Quarto" />

          <section class="settings-subsection">
            <div class="settings-section-title"><h4 class="settings-subhead">Detected tools</h4><span class="settings-scope">This computer</span></div>
            <CompanionTools />
          </section>

          <section class="settings-subsection">
            <div class="settings-section-title"><h4 class="settings-subhead">LaTeX</h4><span class="settings-scope">This browser</span></div>
            <BuildSettings format="latex" {userId} onpreferences={onbuildpreferences} />
            <LatexFilesSettings />
          </section>

          <section class="settings-subsection">
            <div class="settings-section-title"><h4 class="settings-subhead">Typst and Calepin</h4></div>
            <IntegrationSettings name="calepin" />
          </section>

          <section class="settings-subsection">
            <div class="settings-section-title"><h4 class="settings-subhead">Markdown and Quarto</h4></div>
            <BuildSettings format="markdown" {userId} onpreferences={onbuildpreferences} />
            <IntegrationSettings name="quarto" />
            <RenderingSettings {userId} {onquartooptions} />
          </section>
        {:else if shown.id === "integrations"}
          <CompanionBlock id="integrations-companion" needs="Zotero" />

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
