<script>
  import PanelHeader from "../PanelHeader.svelte";
  import { say } from "../../lib/toast.svelte.js";
  import { forgeApi } from "../../lib/forge/client.js";
  import { AuthError, gitHub } from "../../lib/forge/github.js";
  import { accessToken, connect } from "../../lib/forge/auth.js";
  import {
    compareFirst, compareLink, connectLink, exportChanges, firstPush, pull, unsyncedFiles,
  } from "../../lib/forge/sync.js";

  let { open = false, slug, key = "", session = null, rules = {}, forge = null, me = {} } = $props();

  let link = $state(null);
  let loaded = $state(false);
  let signedIn = $state(false);
  let reconnect = $state(false);
  let busy = $state(false);
  let error = $state("");
  let generation = 0;

  // The pick made before anything is recorded.
  let repositories = $state([]);
  let branches = $state([]);
  let pickedId = $state("");
  let branch = $state("");
  let folder = $state("");
  let comparison = $state(null);
  let draft = null;

  let message = $state("");
  let unsynced = $state(null);
  let outcome = $state(null);
  let exported = $state(null);

  const api = $derived(forgeApi(slug, key));
  const picked = $derived(repositories.find((repo) => String(repo.id) === pickedId) || null);
  const name = $derived(me?.name || me?.handle || "Someone");
  const plural = (count) => `${count} ${count === 1 ? "file" : "files"}`;
  const when = (iso) => (iso ? new Date(iso).toLocaleString() : "never");
  const cleanFolder = (value) => value.trim().replace(/^\/+|\/+$/g, "");

  async function context() {
    const token = await accessToken();
    if (!token) throw new AuthError();
    return {
      slug,
      session,
      rules,
      github: gitHub({ token, api: forge?.api }),
      api,
      me: { name: me?.name || me?.handle || "", handle: me?.handle || me?.name || "" },
      web: forge?.web,
    };
  }

  function fail(failure) {
    if (failure instanceof AuthError) {
      reconnect = true;
      return;
    }
    error = failure?.message || "Something went wrong.";
  }

  async function guard(action) {
    if (busy) return;
    busy = true;
    error = "";
    reconnect = false;
    try {
      return await action();
    } catch (failure) {
      fail(failure);
    } finally {
      busy = false;
    }
  }

  async function load() {
    const request = ++generation;
    try {
      const answer = await api.getLink();
      if (request !== generation) return;
      link = answer.link;
      if (!link) signedIn = Boolean(await accessToken());
      if (!link && signedIn && !repositories.length) await loadRepositories();
    } catch (failure) {
      if (request === generation) fail(failure);
    } finally {
      if (request === generation) loaded = true;
    }
  }

  $effect(() => {
    if (open) {
      slug;
      load();
    }
  });

  async function loadRepositories() {
    const github = (await context()).github;
    repositories = await github.repositories();
  }

  async function signIn() {
    await guard(async () => {
      await connect();
      signedIn = true;
      reconnect = false;
      if (link) return;
      await loadRepositories();
    });
  }

  async function pickRepository() {
    comparison = null;
    branches = [];
    branch = "";
    if (!picked) return;
    await guard(async () => {
      const github = (await context()).github;
      branches = await github.branches(picked.name);
      branch = branches.includes(picked.defaultBranch) ? picked.defaultBranch : branches[0] || "";
    });
  }

  async function compare() {
    if (!picked || !branch) return;
    await guard(async () => {
      draft = {
        provider: "github",
        repository_id: picked.id,
        repository: picked.name,
        branch,
        folder: cleanFolder(folder),
        synced_map: {},
        synced_commit: null,
        export: null,
      };
      comparison = await compareFirst(await context(), draft);
    });
  }

  function cancel() {
    comparison = null;
    draft = null;
  }

  async function connectNow(ctx) {
    link = await connectLink(ctx, {
      repositoryId: draft.repository_id,
      repository: draft.repository,
      branch: draft.branch,
      folder: draft.folder,
    });
    comparison = null;
    draft = null;
  }

  async function refreshLink() {
    link = (await api.getLink()).link;
  }

  function show(result) {
    outcome = null;
    exported = null;
    if (result.status === "pulled") {
      say(`${name} pulled ${plural(result.files)} from GitHub`, { kind: "done", id: "github:pull" });
    } else if (result.status === "pushed") {
      say(`Pushed ${plural(result.files)} to GitHub`, { kind: "done", id: "github:push" });
    } else if (result.status === "nothing") {
      say(result.kind === "push" ? "Nothing to push" : "Nothing to pull", { kind: "note", id: "github:nothing" });
    } else {
      outcome = result;
    }
    if (result.link) link = result.link;
  }

  async function doPull() {
    await guard(async () => {
      const ctx = await context();
      if (!link) await connectNow(ctx);
      show(await pull(ctx));
      await refreshLink();
    });
  }

  async function doPush() {
    await guard(async () => {
      const ctx = await context();
      if (!link) await connectNow(ctx);
      const result = await firstPush(ctx, message.trim());
      show(result.status === "nothing" ? { ...result, kind: "push" } : result);
      if (result.status === "pushed") message = "";
      await refreshLink();
    });
  }

  async function branchOff() {
    await guard(async () => {
      const result = await exportChanges(await context());
      if (result.status === "exported") {
        exported = result;
        outcome = null;
        if (result.link) link = result.link;
      } else if (result.status === "nothing") {
        say("Nothing to put in a branch", { kind: "note", id: "github:nothing" });
      } else {
        outcome = result;
      }
    });
  }

  async function showUnsynced() {
    await guard(async () => {
      unsynced = await unsyncedFiles(await context(), link);
    });
  }

  async function disconnect() {
    await guard(async () => {
      await api.deleteLink();
      link = null;
      outcome = null;
      exported = null;
      unsynced = null;
      comparison = null;
      signedIn = Boolean(await accessToken());
      if (signedIn && !repositories.length) await loadRepositories();
    });
  }

  const reasonText = (item) => {
    if (item.reason === "suggestion") return "A pending suggestion sits in a file the pull would change. Accept or reject the suggestion, then pull again.";
    if (item.reason === "changed") return "These files changed while the pull ran. Try again.";
    return item.reason;
  };
  const exportUrl = $derived(link?.export?.branch ? compareLink(link, link.export.branch, forge?.web) : "");
</script>

{#snippet files(title, paths)}
  {#if paths.length}
    <h4 class="panel-section-title">{title} ({paths.length})</h4>
    <ul class="gh-list">
      {#each paths as path (path)}<li>{path}</li>{/each}
    </ul>
  {/if}
{/snippet}

{#snippet reconnectBox()}
  <div class="gh-box" role="alert">
    <p>GitHub no longer accepts the saved sign-in.</p>
    <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={signIn}>Reconnect with your account</button>
  </div>
{/snippet}

<section class="panel gh-panel" aria-label="GitHub">
  <PanelHeader title="GitHub" />
  {#if !loaded}
    <p class="panel-muted" role="status">Loading…</p>
  {:else if link}
    <dl class="gh-facts">
      <dt>Repository</dt><dd>{link.repository}</dd>
      <dt>Branch</dt><dd>{link.branch}</dd>
      <dt>Folder</dt><dd>{link.folder || "(whole repository)"}</dd>
      <dt>Connected by</dt><dd>{link.connected_by?.name || "unknown"}</dd>
      <dt>Last pull</dt><dd>{when(link.last_pull_at)}</dd>
      <dt>Last push</dt><dd>{when(link.last_push_at)}</dd>
    </dl>
    {#if reconnect}{@render reconnectBox()}{/if}
    <div class="gh-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={doPull}>Pull</button>
    </div>
    <label class="gh-field panel-meta">Commit message
      <input class="input gh-input" type="text" bind:value={message} disabled={busy}
        placeholder="The names of the changed files" />
    </label>
    <div class="gh-row">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={doPush}>Push</button>
    </div>

    {#if outcome?.status === "conflict"}
      <div class="gh-box" role="alert">
        <p>The pull was refused. These files changed in both places.</p>
        {@render files("Files", outcome.conflicts)}
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={branchOff}>Create a branch with my changes</button>
      </div>
    {:else if outcome?.status === "refused"}
      <div class="gh-box" role="alert">
        <p>The operation was refused.</p>
        {#each outcome.refusals as item}
          <p>{reasonText(item)}</p>
          {#if item.paths?.length}<ul class="gh-list">{#each item.paths as path}<li>{path}</li>{/each}</ul>{/if}
        {/each}
      </div>
    {/if}
    {#if outcome?.skipped?.length}
      {@render files("Skipped", outcome.skipped.map((item) => `${item.path}: ${item.reason}`))}
    {/if}
    {#if exported}
      <p class="panel-meta" role="status">Created the branch {exported.branch} with {plural(exported.files)}.</p>
    {/if}
    {#if exportUrl}
      <p class="panel-meta"><a href={exportUrl} target="_blank" rel="noopener noreferrer">Open the comparison on GitHub</a></p>
    {/if}

    <div class="gh-row">
      <button type="button" class="gh-action" disabled={busy} onclick={showUnsynced}>Show unsynced files</button>
    </div>
    {#if unsynced}
      {#if unsynced.length}
        <ul class="gh-list">
          {#each unsynced as item (item.path)}<li>{item.path} <span class="panel-meta">{item.reason}</span></li>{/each}
        </ul>
      {:else}
        <p class="panel-meta">Every file in the repository is synced.</p>
      {/if}
    {/if}
    <div class="gh-row">
      <button type="button" class="gh-action gh-destructive" disabled={busy} onclick={disconnect}>Disconnect</button>
    </div>
  {:else if !signedIn || reconnect}
    {#if reconnect}
      {@render reconnectBox()}
    {:else}
      <p class="panel-meta">Sync this project with a GitHub repository.</p>
      <div class="gh-row">
        <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={signIn}>Connect to GitHub</button>
      </div>
    {/if}
  {:else}
    <label class="gh-field panel-meta">Repository
      <select class="select gh-select" bind:value={pickedId} onchange={pickRepository} disabled={busy}>
        <option value="">Choose a repository</option>
        {#each repositories as repo (repo.id)}<option value={String(repo.id)}>{repo.name}</option>{/each}
      </select>
    </label>
    {#if picked}
      <label class="gh-field panel-meta">Branch
        <select class="select gh-select" bind:value={branch} onchange={() => (comparison = null)} disabled={busy}>
          {#each branches as name (name)}<option value={name}>{name}</option>{/each}
        </select>
      </label>
      <label class="gh-field panel-meta">Folder (optional)
        <input class="input gh-input" type="text" bind:value={folder} oninput={() => (comparison = null)} disabled={busy}
          placeholder="Leave empty for the whole repository" />
      </label>
      <div class="gh-row">
        <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || !branch} onclick={compare}>Compare</button>
      </div>
    {/if}
    {#if comparison}
      {#if comparison.refusals.length}
        <div class="gh-box" role="alert">
          {#each comparison.refusals as item}
            <p>{item.reason}</p>
            {#if item.paths?.length}<ul class="gh-list">{#each item.paths as path}<li>{path}</li>{/each}</ul>{/if}
          {/each}
        </div>
      {/if}
      {@render files("Will come in on Pull", comparison.incoming)}
      {@render files("Will go out on Push", comparison.outgoing)}
      {@render files("Differ on both sides", comparison.differing)}
      {#if comparison.folders.length}
        <h4 class="panel-section-title">Synced folders</h4>
        <ul class="gh-list">
          {#each comparison.folders as [folderName, count] (folderName)}<li>{folderName}: {plural(count)}</li>{/each}
        </ul>
      {/if}
      {#if comparison.differing.length}
        <div class="gh-box" role="alert">
          <p>Pull will be refused, because {plural(comparison.differing.length)} differ on both sides. Push will overwrite the repository's copies of those files.</p>
        </div>
      {/if}
      <label class="gh-field panel-meta">Commit message
        <input class="input gh-input" type="text" bind:value={message} disabled={busy}
          placeholder="The names of the changed files" />
      </label>
      <div class="gh-row">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={doPull}>Pull</button>
        <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={doPush}>Push</button>
        <button type="button" class="gh-action" disabled={busy} onclick={cancel}>Cancel</button>
      </div>
      {#if comparison.unsynced.length}
        <p class="panel-meta">{plural(comparison.unsynced.length)} in the repository are not synced.</p>
      {/if}
    {/if}
  {/if}
  {#if error}<p class="lp-text-error" role="alert">{error}</p>{/if}
</section>

<style>
  .gh-panel :global(select) { max-width: 100%; }
  .gh-facts { display: grid; grid-template-columns: auto 1fr; gap: calc(var(--spacing) * .5) calc(var(--spacing) * 2); margin-block: calc(var(--spacing) * 2); }
  .gh-facts dt { color: var(--color-text-muted); font-size: var(--panel-meta-size); }
  .gh-facts dd { margin: 0; overflow-wrap: anywhere; }
  .gh-field { display: grid; gap: var(--spacing); margin-top: calc(var(--spacing) * 2); }
  .gh-input, .gh-select { width: 100%; min-width: 0; }
  .gh-select { border: 0; background-color: var(--color-row-hover); font: inherit; border-radius: var(--radius-base); }
  .gh-row { display: flex; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .gh-box { margin-top: calc(var(--spacing) * 2); padding: calc(var(--spacing) * 2); border-radius: var(--radius-base); background: var(--color-row-hover); }
  .gh-box p { margin: 0 0 calc(var(--spacing) * 1.5); }
  .gh-list { margin: 0; padding-inline-start: calc(var(--spacing) * 4); overflow-wrap: anywhere; font-size: var(--panel-meta-size); }
  .gh-action { padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; cursor: pointer; }
  .gh-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .gh-action:disabled { opacity: .5; cursor: default; }
  .gh-destructive:hover:not(:disabled) { color: var(--color-error-text); }
</style>
