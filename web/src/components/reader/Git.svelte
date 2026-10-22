<script>
  import PanelHeader from "../PanelHeader.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import GitResolver from "./GitResolver.svelte";
  import { say } from "../../lib/toast.svelte.js";
  import { createBridge } from "../../lib/gitsync/bridge.js";
  import { syncApi } from "../../lib/gitsync/server.js";
  import {
    compare, connect, outgoing, pull, push, recover, status,
  } from "../../lib/gitsync/sync.js";

  let { open = false, slug, key = "", session = null, rules = {}, me = {} } = $props();

  const git = createBridge();
  const api = $derived(syncApi(slug, key));
  const DOCS = "/docs/git.html";

  let info = $state(null);
  let link = $state(null);
  let loaded = $state(false);
  let busy = $state(false);
  let error = $state(null);
  let generation = 0;

  // The pick made before anything is recorded.
  let remoteText = $state("");
  let grant = $state(null);
  let branches = $state([]);
  let branch = $state("");
  let folder = $state("");
  let comparison = $state(null);
  let direction = $state("librepaper");
  let draft = null;

  // The review before a push.
  let review = $state(null);
  let message = $state("");

  let outcome = $state(null);
  let resolverOpen = $state(false);
  let conflictSession = $state(null);

  const name = $derived(me?.name || me?.handle || "Someone");
  const plural = (count) => `${count} ${count === 1 ? "file" : "files"}`;
  const when = (iso, by) => (iso ? `${new Date(iso).toLocaleString()}${by ? ` by ${by}` : ""}` : "never");
  const cleanFolder = (value) => value.trim().replace(/^\/+|\/+$/g, "");
  const label = (value) => value?.name || value?.handle || value || "";

  const connectedHere = $derived(Boolean(link && info?.connected && info?.connection_id === link.connection_id));
  const remote = $derived(grant ? { grant: grant.grant } : { url: remoteText.trim() });
  const haveRemote = $derived(Boolean(grant || remoteText.trim()));

  function context() {
    return {
      slug,
      session,
      rules,
      git,
      api,
      me: { name: me?.name || me?.handle || "", handle: me?.handle || me?.name || "" },
    };
  }

  function fail(failure) {
    if (failure?.kind === "auth") {
      error = { auth: true };
    } else {
      error = { text: failure?.message || "Something went wrong." };
    }
  }

  async function guard(action) {
    if (busy) return;
    busy = true;
    error = null;
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
      const [state, answer] = await Promise.all([status(context()), api.getLink()]);
      if (request !== generation) return;
      info = state;
      link = answer.link;
      if (state?.session && !conflictSession) conflictSession = state.session;
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

  async function refresh() {
    const [state, answer] = await Promise.all([status(context()), api.getLink()]);
    info = state;
    link = answer.link;
  }

  function resetDraft() {
    comparison = null;
    draft = null;
    review = null;
  }

  async function loadBranches() {
    comparison = null;
    branches = [];
    branch = "";
    if (!haveRemote) return;
    await guard(async () => {
      const answer = await git.branches(remote);
      branches = answer.branches || [];
      branch = branches.includes(answer.default) ? answer.default : branches[0] || "";
    });
  }

  async function chooseLocal() {
    await guard(async () => {
      grant = await git.chooseLocal();
      remoteText = "";
    });
    if (grant) await loadBranches();
  }

  async function doCompare() {
    if (!haveRemote || !branch) return;
    await guard(async () => {
      draft = { remote, label: grant?.label || remoteText.trim(), branch, folder: cleanFolder(folder) };
      comparison = await compare(context(), draft);
      direction = "librepaper";
    });
  }

  async function doConnect() {
    if (!draft) return;
    await guard(async () => {
      const result = await connect(context(), draft, direction);
      resetDraft();
      show(result);
      await refresh();
    });
  }

  async function connectThisComputer() {
    await guard(async () => {
      await git.connect({ connection_id: link.connection_id, remote, branch: link.branch });
      await refresh();
    });
  }

  function show(result) {
    outcome = null;
    const from = link?.remote_label || "the repository";
    if (result.link) link = result.link;
    if (result.status === "pulled") {
      say(`${name} pulled ${plural(result.files)} from ${from}`, { kind: "done", id: "git:pull" });
    } else if (result.status === "pushed") {
      const id = String(result.commit || "").slice(0, 7);
      say(`Pushed ${id} to ${from}`, { kind: "done", id: "git:push" });
    } else if (result.status === "nothing") {
      say(result.kind === "push" ? "Nothing to push" : "Nothing to pull", { kind: "note", id: "git:nothing" });
    } else if (result.status === "conflict") {
      conflictSession = result.session;
      resolverOpen = true;
    } else {
      outcome = result;
    }
  }

  async function doPull() {
    await guard(async () => {
      show(await pull(context()));
      await refresh();
    });
  }

  async function reviewPush() {
    await guard(async () => {
      const result = await outgoing(context());
      if (["nothing", "pulled", "conflict", "refused", "moved"].includes(result.status)) {
        show(result.status === "nothing" ? { ...result, kind: "push" } : result);
        review = null;
      } else {
        review = result;
        message = "";
      }
      await refresh();
    });
  }

  async function doPush() {
    if (!review) return;
    await guard(async () => {
      const result = await push(context(), message.trim(), { confirmed: review.prepared });
      if (result.status === "pushed" || result.status === "nothing") review = null;
      if (result.status === "moved") {
        review = null;
        outcome = result;
      } else {
        show(result.status === "nothing" ? { ...result, kind: "push" } : result);
      }
      await refresh();
    });
  }

  async function resume() {
    if (info?.session && !info?.pending) {
      conflictSession = info.session;
      resolverOpen = true;
      return;
    }
    await guard(async () => {
      show(await recover(context()));
      await refresh();
    });
  }

  async function resolved(result) {
    show(result);
    await guard(refresh);
  }

  async function disconnect() {
    await guard(async () => {
      await api.deleteLink();
      await git.forget();
      link = null;
      outcome = null;
      review = null;
      resetDraft();
      await refresh();
    });
  }

  const reasonText = (item) => {
    if (item.reason === "suggestion") return "A pending suggestion sits in a file the pull would change. Accept or reject the suggestion, then pull again.";
    if (item.reason === "changed") return "Someone kept typing in these files. Try again.";
    return item.reason;
  };
  const gitVersion = $derived(info?.git ? `Git ${info.git}` : "no Git");
</script>

{#snippet files(title, paths)}
  {#if paths?.length}
    <h4 class="panel-section-title">{title} ({paths.length})</h4>
    <ul class="gh-list">
      {#each paths as path (path)}<li>{path}</li>{/each}
    </ul>
  {/if}
{/snippet}

<section class="panel gh-panel" aria-label="Git">
  <PanelHeader title="Git" />
  <DesktopOnly feature="Git sync" description="Keep this project in sync with a Git repository.">
  {#if !loaded}
    <p class="panel-muted" role="status">Loading…</p>
  {:else if info && !info.supported}
    <div class="gh-box" role="alert">
      <p>Found {gitVersion}. Git 2.38 or later is needed.</p>
      <p><a href={DOCS} target="_blank" rel="noopener noreferrer">How to set up Git</a></p>
    </div>
  {:else}
    {#if info?.pending || info?.session}
      <div class="gh-box" role="status">
        <p>{info.pending ? "A sync was interrupted." : "A conflict is saved."}</p>
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={resume}>Resume</button>
      </div>
    {/if}

    {#if link && connectedHere}
      <dl class="gh-facts">
        <dt>Remote</dt><dd>{link.remote_label}</dd>
        <dt>Branch</dt><dd>{link.branch}</dd>
        <dt>Folder</dt><dd>{link.folder || "(whole repository)"}</dd>
        <dt>Connected by</dt><dd>{label(link.connected_by) || "unknown"}</dd>
        <dt>Last pull</dt><dd>{when(link.last_pull_at, label(link.last_pull_by))}</dd>
        <dt>Last push</dt><dd>{when(link.last_push_at, label(link.last_push_by))}</dd>
      </dl>
      <div class="gh-row">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={doPull}>Pull</button>
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={reviewPush}>Review push</button>
      </div>

      {#if review}
        {@render files("Will go out on Push", review.outgoing)}
        <label class="gh-field panel-meta">Commit message
          <input class="input gh-input" type="text" bind:value={message} disabled={busy}
            placeholder={review.message || "The names of the changed files"} />
        </label>
        <div class="gh-row">
          <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={doPush}>Push</button>
          <button type="button" class="gh-action" disabled={busy} onclick={() => (review = null)}>Cancel</button>
        </div>
      {/if}
    {:else if link}
      <dl class="gh-facts">
        <dt>Remote</dt><dd>{link.remote_label}</dd>
        <dt>Branch</dt><dd>{link.branch}</dd>
      </dl>
      <p class="panel-meta">This project is connected to a repository, but not on this computer.</p>
      {#if link.remote_kind === "local"}
        <div class="gh-row">
          <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={async () => { await chooseLocal(); if (grant) await connectThisComputer(); }}>Choose the local repository</button>
        </div>
      {:else}
        <label class="gh-field panel-meta">Remote
          <input class="input gh-input" type="text" bind:value={remoteText} disabled={busy}
            placeholder="https://example.com/owner/repo.git" />
        </label>
        <div class="gh-row">
          <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || !remoteText.trim()} onclick={connectThisComputer}>Connect this computer</button>
        </div>
      {/if}
    {:else}
      <p class="panel-meta">Sync this project with a Git repository.</p>
      <label class="gh-field panel-meta">Remote
        <input class="input gh-input" type="text" bind:value={remoteText} disabled={busy}
          placeholder="HTTPS or SSH URL"
          oninput={() => { grant = null; resetDraft(); }} onchange={loadBranches} />
      </label>
      <div class="gh-row">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={chooseLocal}>Choose a local repository</button>
        {#if grant}<span class="panel-meta">{grant.label}</span>{/if}
      </div>
      {#if branches.length}
        <label class="gh-field panel-meta">Branch
          <select class="select gh-select" bind:value={branch} onchange={resetDraft} disabled={busy}>
            {#each branches as option (option)}<option value={option}>{option}</option>{/each}
          </select>
        </label>
        <label class="gh-field panel-meta">Folder (optional)
          <input class="input gh-input" type="text" bind:value={folder} oninput={resetDraft} disabled={busy}
            placeholder="Leave empty for the whole repository" />
        </label>
        <div class="gh-row">
          <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || !branch} onclick={doCompare}>Compare</button>
        </div>
      {/if}
      {#if comparison}
        {#if comparison.refusals?.length}
          <div class="gh-box" role="alert">
            {#each comparison.refusals as item}
              <p>{item.reason}</p>
              {#if item.paths?.length}<ul class="gh-list">{#each item.paths as path}<li>{path}</li>{/each}</ul>{/if}
            {/each}
          </div>
        {/if}
        {@render files("Will come in", comparison.incoming)}
        {@render files("Will go out", comparison.outgoing)}
        {@render files("Differ on both sides", comparison.differing)}
        {#if comparison.folders?.length}
          <h4 class="panel-section-title">Synced folders</h4>
          <ul class="gh-list">
            {#each comparison.folders as [folderName, count] (folderName)}<li>{folderName}: {plural(count)}</li>{/each}
          </ul>
        {/if}
        {#if comparison.unsynced?.length}
          <h4 class="panel-section-title">Not synced ({comparison.unsynced.length})</h4>
          <ul class="gh-list">
            {#each comparison.unsynced as item (item.path)}<li>{item.path} <span class="panel-meta">{item.reason}</span></li>{/each}
          </ul>
        {/if}
        {#if comparison.differing?.length}
          <fieldset class="gh-choice">
            <legend class="panel-meta">{plural(comparison.differing.length)} differ. Which copies do you keep?</legend>
            <label><input type="radio" name="git-direction" value="librepaper" bind:group={direction} />
              Keep LibrePaper's copies. The repository gets them on the first push.</label>
            <label><input type="radio" name="git-direction" value="repository" bind:group={direction} />
              Keep the repository's copies. LibrePaper takes them now.</label>
          </fieldset>
        {/if}
        <div class="gh-row">
          <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || comparison.refusals?.length > 0} onclick={doConnect}>Connect</button>
          <button type="button" class="gh-action" disabled={busy} onclick={resetDraft}>Cancel</button>
        </div>
      {/if}
    {/if}

    {#if outcome?.status === "moved"}
      <div class="gh-box" role="alert"><p>The repository changed while you reviewed. Review the push again.</p></div>
    {:else if outcome?.status === "refused"}
      <div class="gh-box" role="alert">
        <p>The operation was refused.</p>
        {#each outcome.refusals || [] as item}
          <p>{reasonText(item)}</p>
          {#if item.paths?.length}<ul class="gh-list">{#each item.paths as path}<li>{path}</li>{/each}</ul>{/if}
        {/each}
      </div>
    {/if}

    {#if link}
      <div class="gh-row">
        <button type="button" class="gh-action gh-destructive" disabled={busy} onclick={disconnect}>Disconnect</button>
      </div>
    {/if}
  {/if}
  {#if error?.auth}
    <p class="lp-text-error" role="alert">Git could not sign in to the remote. Check your credential helper or SSH agent. <a href={DOCS} target="_blank" rel="noopener noreferrer">Read more</a></p>
  {:else if error}
    <p class="lp-text-error" role="alert">{error.text}</p>
  {/if}
  </DesktopOnly>
</section>

<GitResolver bind:open={resolverOpen} ctx={context()} session={conflictSession} onresult={resolved} />

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
  .gh-choice { display: grid; gap: calc(var(--spacing) * 1.5); margin: calc(var(--spacing) * 2) 0 0; padding: 0; border: 0; }
  .gh-destructive:hover:not(:disabled) { color: var(--color-error-text); }
</style>
