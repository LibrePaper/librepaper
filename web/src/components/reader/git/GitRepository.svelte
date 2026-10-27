<script>
  // The Repository section of a folder project's Git panel: the folder's own
  // git repository. Initialize it, see the branch and the remote, list the
  // changes, commit them, pull from and push to the remote, or do all three with
  // Sync with remote. The state and
  // the actions belong to the controller (lib/reader/local-git.svelte.js); this
  // draws them. It is the person's repository: their identity, credential
  // helpers and hooks apply, and nothing is written but what git writes.
  import { untrack } from "svelte";
  import Icon from "../../Icon.svelte";
  import GitSignInHelp from "./GitSignInHelp.svelte";

  let { git, differ = null, open = false, ondiff = undefined } = $props();

  const DOCS = "/docs/git.html";
  const view = $derived(git.state);
  const status = $derived(view.status);
  const busy = $derived(view.busy !== "");

  // What the person typed. Until they type, the message shows its default.
  let typedMessage = $state(null);
  let left = $state([]);
  let identityName = $state("");
  let identityEmail = $state("");
  let creating = $state(false);
  let newBranch = $state("");
  let settingRemote = $state(false);
  let typedRemote = $state(null);

  const message = $derived(typedMessage ?? git.defaultMessage());

  // The state is read when the panel opens and then again every half minute
  // while it stays open and the window is showing, because the folder changes
  // under it as the project is edited.
  $effect(() => {
    if (!open) return;
    void git.refresh();
    const timer = setInterval(() => {
      if (document.visibilityState !== "hidden") void git.refresh();
    }, 30_000);
    return () => clearInterval(timer);
  });

  // The branches are listed when the repository is first seen and again when
  // its branch changes, so the menu is never one that was made before a switch.
  $effect(() => {
    if (!open || !status?.repository) return;
    void status.branch;
    untrack(() => { void git.loadBranches(); });
  });

  const plural = (count, noun) => `${count} ${noun}${count === 1 ? "" : "s"}`;
  const changes = $derived(Array.isArray(status?.changes) ? status.changes : []);

  // The comparison in the editor area is for a file that has changes. Once the
  // file is committed or reverted there is nothing left to compare, so it goes.
  $effect(() => {
    if (!differ?.state.active || !status?.repository) return;
    const path = differ.state.path;
    if (!changes.some((change) => change.path === path)) untrack(() => differ.close());
  });
  const merging = $derived(Boolean(status?.merging));
  const remotes = $derived(Array.isArray(status?.remotes) ? status.remotes : []);
  // The remote that Fetch, Pull and Push use. The desktop app names it in the
  // status, and it is null when the repository has no remote.
  const remote = $derived(remotes.find((entry) => entry.name === status?.remote) ?? null);
  const branchNames = $derived.by(() => {
    const names = view.branches?.names ?? [];
    return status?.branch && !names.includes(status.branch) ? [status.branch, ...names] : names;
  });
  // Branches that only a remote has. Choosing one checks it out as a local branch.
  const remoteNames = $derived((view.branches?.remote ?? []).filter((name) => !branchNames.includes(name)));

  // A commit takes everything unless a file was left out. A renamed file sends
  // its old name too, so the deletion of that name is committed with the new one.
  // When another change sits at the old name, that change has its own box, so a
  // file written again in place of the old one stays out when its box is unticked.
  const chosen = $derived(changes.filter((change) => !left.includes(change.path)));
  const included = $derived(chosen.flatMap((change) => (change.from && !changes.some((other) => other.path === change.from) ? [change.path, change.from] : [change.path])));
  const partial = $derived(!merging && chosen.length < changes.length);
  const canCommit = $derived(!busy && message.trim() !== "" && (merging || included.length > 0));

  const upstreamText = $derived.by(() => {
    if (!remote) return "No remote yet. Set one to pull from and push to it.";
    if (!status?.upstream) return "No upstream branch yet. The first push to remote sets it.";
    const ahead = Number(status.ahead);
    const behind = Number(status.behind);
    if (!Number.isFinite(ahead) || !Number.isFinite(behind)) return `Follows ${status.upstream}.`;
    if (!ahead && !behind) return `Up to date with ${status.upstream}.`;
    return `${ahead} ahead, ${behind} behind ${status.upstream}.`;
  });

  const MARK = { added: "A", modified: "M", deleted: "D", renamed: "R", conflicted: "U" };
  const KIND = { added: "Added", modified: "Modified", deleted: "Deleted", renamed: "Renamed", conflicted: "Conflicted" };
  function split(path) {
    const slash = path.lastIndexOf("/");
    return slash > 0 ? [path.slice(slash + 1), path.slice(0, slash)] : [path, ""];
  }

  function include(path, on) {
    left = on ? left.filter((entry) => entry !== path) : [...left, path];
  }

  async function submitCommit(event) {
    event.preventDefault();
    await git.commit(message, partial ? included : undefined);
    if (git.state.steps.some((step) => step.step === "commit" && !step.failed)) {
      typedMessage = null;
      left = [];
    }
  }

  async function submitIdentity(event) {
    event.preventDefault();
    await git.saveIdentity(identityName, identityEmail);
    if (!git.state.identity) {
      typedMessage = null;
      left = [];
    }
  }

  async function submitBranch(event) {
    event.preventDefault();
    await git.createBranch(newBranch);
    if (!git.state.error) {
      creating = false;
      newBranch = "";
    }
  }

  async function submitRemote(event) {
    event.preventDefault();
    await git.setRemote(typedRemote ?? remote?.url ?? "", remote?.name ?? "origin");
    if (!git.state.error) {
      settingRemote = false;
      typedRemote = null;
    }
  }

  const pickBranch = (event) => {
    const next = event.currentTarget.value;
    if (next && next !== status?.branch) void git.switchBranch(next);
  };
</script>

{#snippet results()}
  <div class="git-results" role="status" aria-live="polite">
    {#each view.steps as step (step.step + step.text)}
      <p class="git-line" class:git-failed={step.failed}>
        {#if step.failed}<Icon name="triangle-alert" size={14} />{/if}<span>{step.text}</span>
      </p>
    {/each}
    {#if view.conflicts.length}
      <div class="git-box" role="alert">
        <p>{plural(view.conflicts.length, "file")} in conflict. Resolve {view.conflicts.length === 1 ? "it" : "them"} in the editor, then Commit.</p>
        <ul class="git-list">{#each view.conflicts as path (path)}<li>{path}</li>{/each}</ul>
      </div>
    {/if}
    {#if view.refusal?.reason === "dirty"}
      <div class="git-box" role="alert">
        <p>These files have changes that are not committed{view.refusal.paths.length ? "" : ", so the pull did not start"}. Commit them, then pull from remote again.</p>
        {#if view.refusal.paths.length}
          <ul class="git-list">{#each view.refusal.paths as path (path)}<li>{path}</li>{/each}</ul>
        {/if}
      </div>
    {:else if view.refusal?.reason === "behind"}
      <div class="git-box" role="alert"><p>The remote has commits you do not have. Pull from remote first.</p></div>
    {:else if view.refusal?.reason === "network"}
      <div class="git-box" role="alert">
        <p>Git could not reach the remote. Check the connection and the address, then try again.</p>
        {#if view.refusal.detail}<p class="panel-meta git-detail">{view.refusal.detail}</p>{/if}
      </div>
    {/if}
  </div>
{/snippet}

<section class="git-block" aria-labelledby="git-repository-title">
  <h3 id="git-repository-title" class="panel-section-title">Repository</h3>
  {#if !view.loaded}
    <p class="panel-muted" role="status">Loading…</p>
  {:else if !status}
    <p class="panel-muted">The repository could not be read.</p>
    <div class="git-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => git.refresh()}>Try again</button>
    </div>
  {:else if !status.git?.found || status.git?.supported === false}
    <p class="git-lead">
      {status.git?.found && status.git?.version
        ? `Git ${status.git.version} is installed, and LibrePaper needs Git 2.38 or later.`
        : "Git was not found on this computer, and LibrePaper needs Git 2.38 or later."}
    </p>
    <p class="panel-meta"><a href={DOCS} target="_blank" rel="noopener noreferrer">Read how to set up Git</a></p>
  {:else if !status.repository}
    <p class="git-lead">This folder is not a git repository.</p>
    <div class="git-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => git.init()}>
        {view.busy === "init" ? "Initializing…" : "Initialize"}
      </button>
    </div>
  {:else}
    <div class="git-branch">
      <Icon name="git-branch" size={14} />
      {#if creating}
        <form class="git-inline" onsubmit={submitBranch}>
          <input class="input git-input" type="text" aria-label="Name of the new branch" placeholder="New branch" bind:value={newBranch}
                 disabled={busy} autocomplete="off" spellcheck="false" />
          <button type="submit" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !newBranch.trim()}>Create</button>
          <button type="button" class="git-action" onclick={() => { creating = false; newBranch = ""; }}>Cancel</button>
        </form>
      {:else}
        <select class="select git-select" aria-label="Branch" value={status.branch ?? ""} disabled={busy || !branchNames.length} onchange={pickBranch}>
          {#if !status.branch}<option value="">No branch</option>{/if}
          {#each branchNames as name (name)}<option value={name}>{name}</option>{/each}
          {#if remoteNames.length}
            <optgroup label="On the remote">
              {#each remoteNames as name (name)}<option value={name}>{name}</option>{/each}
            </optgroup>
          {/if}
        </select>
        <button type="button" class="git-action" disabled={busy} onclick={() => (creating = true)}>New branch</button>
      {/if}
    </div>
    <p class="panel-meta git-note git-where">{upstreamText}</p>

    <div class="git-remote">
      <Icon name="link" size={14} />
      <span class="git-remote-text" title={remote?.url ?? ""}>{remote ? `${remote.name} ${remote.url}` : "No remote"}</span>
      <button type="button" class="git-action" disabled={busy} onclick={() => (settingRemote = !settingRemote)}>Set remote</button>
    </div>
    {#if settingRemote}
      <form class="git-form" onsubmit={submitRemote}>
        <label class="git-field panel-meta">Address of the remote
          <input class="input git-input" type="text" placeholder="https://github.com/you/paper.git" disabled={busy}
                 value={typedRemote ?? remote?.url ?? ""} oninput={(event) => (typedRemote = event.currentTarget.value)}
                 autocomplete="off" spellcheck="false" />
        </label>
        <div class="git-row">
          <button type="submit" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !(typedRemote ?? remote?.url ?? "").trim()}>Save</button>
          <button type="button" class="git-action" onclick={() => { settingRemote = false; typedRemote = null; }}>Cancel</button>
        </div>
      </form>
    {/if}

    <h4 class="git-subtitle">Changes{changes.length ? ` (${changes.length})` : ""}</h4>
    {#if merging}
      <p class="panel-meta git-note">A merge is in progress. Resolve any conflicts in the editor, then Commit.</p>
    {/if}
    {#if !changes.length}
      <p class="panel-meta git-note">No changes.</p>
    {:else}
      <ul class="git-changes" aria-label="Changed files">
        {#each changes as change (change.path)}
          {@const parts = split(change.path)}
          <li class="git-change">
            {#if !merging}
              <input type="checkbox" class="git-check" checked={!left.includes(change.path)} disabled={busy}
                     aria-label={`Include ${change.path} in the commit`} onchange={(event) => include(change.path, event.currentTarget.checked)} />
            {:else}
              <span></span>
            {/if}
            <span class="git-change-name" title={change.from ? `${change.from} renamed to ${change.path}` : change.path}>
              <span class="git-change-file">{parts[0]}</span>{#if parts[1]}<span class="git-change-folder panel-meta">{parts[1]}</span>{/if}
            </span>
            <span class="git-mark" title={KIND[change.kind] ?? change.kind} aria-label={KIND[change.kind] ?? change.kind}>{MARK[change.kind] ?? "?"}</span>
            <button type="button" class="git-action" disabled={!differ || busy}
                    aria-current={differ?.state.active && differ.state.path === change.path ? "true" : undefined}
                    aria-label={`Compare ${change.path} with the last commit`} onclick={() => ondiff?.(change)}>Diff</button>
          </li>
        {/each}
      </ul>
    {/if}

    {#if view.identity}
      <form class="git-box git-identity" onsubmit={submitIdentity} aria-label="Who you are to git">
        <p>Git needs a name and an email for your commits. They are saved in this repository only.</p>
        <label class="git-field panel-meta">Name
          <input class="input git-input" type="text" bind:value={identityName} disabled={busy} autocomplete="name" />
        </label>
        <label class="git-field panel-meta">Email
          <input class="input git-input" type="email" bind:value={identityEmail} disabled={busy} autocomplete="email" />
        </label>
        <div class="git-row">
          <button type="submit" class="btn btn-sm lp-control-brand" disabled={busy || !identityName.trim() || !identityEmail.trim()}>
            {view.identity.then === "sync" ? "Save and sync with remote" : "Save and commit"}
          </button>
          <button type="button" class="git-action" disabled={busy} onclick={() => git.cancelIdentity()}>Cancel</button>
        </div>
      </form>
    {:else}
      <form class="git-form" onsubmit={submitCommit}>
        <label class="git-field panel-meta">Message
          <input class="input git-input" type="text" value={message} maxlength="200" disabled={busy}
                 oninput={(event) => (typedMessage = event.currentTarget.value)} />
        </label>
        <div class="git-row">
          <button type="submit" class="btn btn-sm lp-control-brand" disabled={!canCommit}>
            {view.busy === "commit" ? "Committing…" : "Commit"}
          </button>
        </div>
      </form>
    {/if}

    <div class="git-row git-remote-actions">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !remote} onclick={() => git.pull()}>
        {view.busy === "pull" ? "Pulling from remote…" : "Pull from remote"}
      </button>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !remote} onclick={() => git.push()}>
        {view.busy === "push" ? "Pushing to remote…" : "Push to remote"}
      </button>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !remote} onclick={() => git.fetch()}>
        {view.busy === "fetch" ? "Fetching from remote…" : "Fetch from remote"}
      </button>
    </div>
    <div class="git-row">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || !remote}
              onclick={() => git.sync(typedMessage ?? "")}>
        {view.busy === "sync" ? "Syncing with remote…" : "Sync with remote"}
      </button>
      <span class="panel-meta git-sync-note">Commit, then pull from remote, then push to remote.</span>
    </div>
    {@render results()}
    {#if view.auth}
      <GitSignInHelp remote={remote?.url ?? ""} name={remote?.name ?? "origin"} detail={view.auth.detail} {busy}
                     onretry={() => git.retrySignIn()} ondismiss={() => git.dismiss()} />
    {/if}
  {/if}
  {#if view.error}
    <p class="lp-text-error git-error" role="alert">{view.error}</p>
  {/if}
</section>

<style>
  .git-lead { margin: 0 0 calc(var(--spacing) * 3); }
  .git-note { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-where { margin-top: calc(var(--spacing) * .5); }
  .git-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-form { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 3); }
  .git-form > .git-row { margin-top: 0; }
  .git-field { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--spacing); }
  .git-input { width: 100%; min-width: 0; }
  .git-branch { display: flex; align-items: center; gap: calc(var(--spacing) * 1.5); min-width: 0; }
  .git-branch :global(svg), .git-remote :global(svg) { flex: none; color: var(--color-text-muted); }
  .git-select { flex: 1 1 0; min-width: 0; max-width: 100%; }
  .git-inline { display: flex; flex-wrap: wrap; align-items: center; flex: 1 1 0; min-width: 0; gap: calc(var(--spacing) * 1.5); }
  .git-inline .git-input { flex: 1 1 6rem; width: auto; }
  .git-remote { display: flex; align-items: center; gap: calc(var(--spacing) * 1.5); min-width: 0; margin-top: calc(var(--spacing) * 3); }
  .git-remote-text { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-subtitle { margin: calc(var(--spacing) * 4) 0 0; font: inherit; font-weight: var(--panel-title-weight); }
  .git-changes { margin: calc(var(--spacing) * 1) 0 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .git-change { display: grid; grid-template-columns: auto minmax(0, 1fr) auto auto; align-items: center; gap: calc(var(--spacing) * 1.5); min-width: 0; min-height: 2rem; border-bottom: 1px solid var(--color-divider); }
  .git-check { margin: 0; }
  .git-change-name { display: flex; align-items: baseline; gap: calc(var(--spacing) * 1.5); min-width: 0; }
  .git-change-file { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-change-folder { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-mark { font-weight: 600; font-variant-numeric: tabular-nums; }
  .git-action { padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; cursor: pointer; }
  .git-action:hover:not(:disabled), .git-action[aria-current="true"] { background: var(--color-row-hover); }
  .git-action:disabled { opacity: .5; cursor: default; }
  .git-remote-actions { margin-top: calc(var(--spacing) * 4); }
  .git-sync-note { margin: 0; }
  .git-results { margin-top: calc(var(--spacing) * 3); }
  .git-results:empty { display: none; }
  .git-line { display: flex; align-items: baseline; gap: calc(var(--spacing) * 1.5); margin: 0 0 calc(var(--spacing) * 1.5); overflow-wrap: anywhere; }
  .git-line :global(svg) { flex: none; align-self: center; color: var(--color-error-text); }
  .git-box { margin-top: calc(var(--spacing) * 2); padding: calc(var(--spacing) * 2); border-radius: var(--radius-base); background: var(--color-row-hover); }
  .git-box p { margin: 0 0 calc(var(--spacing) * 1.5); overflow-wrap: anywhere; }
  .git-box p:last-child { margin-bottom: 0; }
  .git-identity { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 3); }
  .git-identity p { margin: 0; }
  .git-identity > .git-row { margin-top: 0; }
  .git-list { margin: 0 0 calc(var(--spacing) * 1.5); padding-inline-start: calc(var(--spacing) * 4); overflow-wrap: anywhere; font-size: var(--panel-meta-size); }
  .git-detail { overflow-wrap: anywhere; }
  .git-error { margin: calc(var(--spacing) * 3) 0 0; overflow-wrap: anywhere; }
  a { color: var(--color-link); }
</style>
