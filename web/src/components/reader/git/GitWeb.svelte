<script>
  // A web project's Git panel. The project cannot run git, so this panel runs
  // nothing on the project itself. What the server knows (the last push) belongs
  // to the controller (lib/reader/web-git.svelte.js). In the desktop app the
  // panel also connects the project to a git repository: the desktop app keeps
  // a clone on this computer, and Get repository changes and Send project
  // changes move changes between the clone and the project. That state belongs
  // to lib/reader/web-connection.svelte.js;
  // this draws it. The GitHub Action that keeps a repository in step is
  // described in the docs.
  import { untrack } from "svelte";
  import { since } from "../../../lib/dates.js";
  import { remoteUrlProblem } from "../../../lib/git-url.js";
  import GitSignInHelp from "./GitSignInHelp.svelte";

  let {
    web,
    slug,
    linkKey = "",
    open = false,
    desktop = false,
    // The desktop app's connection to a repository. An editor in the desktop app has one.
    connection = null,
    // Keeps a folder on this computer. It throws with the message the panel shows.
    onfolder,
  } = $props();

  const DOCS = "/docs/git.html";
  const view = $derived(web.state);
  const link = $derived(connection?.state);
  const status = $derived(link?.status ?? null);
  const busy = $derived(Boolean(link?.busy));
  // A merge blocks both transfers only while a file waits for a choice. One
  // whose files are settled but whose commit failed is retried by either.
  const merging = $derived(Boolean(status?.merging?.conflicts?.length));
  // What a refused transfer is called in the panel's sentences.
  const ACTION = { pull: "getting repository changes", push: "sending project changes" };

  // The address that is typed, and the problem with it. A Use a folder action
  // that is running holds its own button.
  let address = $state("");
  let failure = $state("");
  let folderBusy = $state(false);
  let folderFailure = $state("");

  $effect(() => {
    if (!open) return;
    const project = slug;
    const options = { key: linkKey };
    untrack(() => { void web.refresh(project, options); });
    const timer = setInterval(() => {
      if (document.visibilityState !== "hidden") void web.refresh(project, options);
    }, 30_000);
    return () => clearInterval(timer);
  });

  // The connection is read when the panel opens and then again every half
  // minute while it stays open and the window is showing, because the repository
  // and the project change under it.
  $effect(() => {
    if (!open || !desktop || !connection) return;
    untrack(() => { void connection.refresh(); });
    const timer = setInterval(() => {
      if (document.visibilityState !== "hidden") void connection.refresh();
    }, 30_000);
    return () => clearInterval(timer);
  });

  // The project's last change from a repository, which the server records as a push.
  const pushText = $derived.by(() => {
    if (!view.loaded) return "Checking for changes sent from a repository…";
    if (view.pushProblem) return "The last change sent from a repository could not be read.";
    if (!view.push) return "No changes sent from a repository yet.";
    const who = view.push.by ? ` by ${view.push.by},` : "";
    return `Last sent from a repository${who} ${since(view.push.at)}${view.push.label ? `: ${view.push.label}` : ""}`;
  });
  const pushTitle = $derived(view.push?.at ? new Date(view.push.at).toLocaleString() : "");

  async function connect(event) {
    event.preventDefault();
    if (busy) return;
    failure = remoteUrlProblem(address);
    if (failure) return;
    await connection.connect(address);
    if (connection.state.status?.connected) address = "";
  }

  async function chooseFolder() {
    if (folderBusy || busy) return;
    folderBusy = true;
    folderFailure = "";
    try {
      await onfolder();
    } catch (error) {
      folderFailure = error?.message || "The desktop app could not keep a folder for this project.";
    } finally {
      folderBusy = false;
    }
  }
</script>

{#snippet signIn()}
  <p class="git-lead">Sign in so the desktop app can reach this project.</p>
  <div class="git-row">
    <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={() => connection.login()}>
      {link.busy === "login" ? "Signing in…" : "Sign in"}
    </button>
  </div>
{/snippet}

{#snippet connectForm()}
  <form class="git-form" onsubmit={connect}>
    <label class="git-field panel-meta">Repository address
      <input class="input git-input" type="text" placeholder="https://github.com/owner/repo.git" autocomplete="off" spellcheck="false"
             bind:value={address} disabled={busy} />
    </label>
    <div class="git-row">
      <button type="submit" class="btn btn-sm lp-control-tonal-brand" disabled={busy}>
        {link.busy === "connect" ? "Connecting…" : "Connect"}
      </button>
    </div>
  </form>
  <div class="git-row">
    <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy || folderBusy} onclick={chooseFolder}>Use a folder on this computer…</button>
  </div>
  <p class="panel-meta git-note">To edit in a clone you already have, or in a new empty folder.</p>
  {#if folderFailure}<p class="lp-text-error git-error" role="alert">{folderFailure}</p>{/if}
{/snippet}

{#snippet connected()}
  <p class="git-remote" title={status.url}>{status.url}, branch {status.branch}</p>
  {#if !status.signed_in}
    {@render signIn()}
  {:else}
    <div class="git-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy || merging} onclick={() => connection.pull()}>
        {link.busy === "pull" ? "Getting repository changes…" : "Get repository changes"}
      </button>
      <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || merging} onclick={() => connection.push()}>
        {link.busy === "push" ? "Sending project changes…" : "Send project changes"}
      </button>
    </div>
    <p class="panel-meta git-note">Get repository changes imports the repository's commits into this project. Send project changes sends this project's changes to the repository branch above.</p>
  {/if}

  {#if link.result}<p class="git-line" role="status">{link.result}</p>{/if}
  {#if link.refusal?.reason === "changed"}
    <div class="git-box" role="alert">
      <p>The project changed these files since the repository was last read:</p>
      <ul class="git-list">{#each link.refusal.paths as path (path)}<li>{path}</li>{/each}</ul>
      <p>Get repository changes again.</p>
    </div>
  {:else if link.refusal?.reason === "suggestion"}
    <div class="git-box" role="alert">
      <p>A pending suggestion in {link.refusal.path || "a file"} stops {ACTION[link.refusal.action]}. Accept or reject it, then try again.</p>
    </div>
  {/if}

  {#if status.merging && !merging}
    <p class="git-lead" role="status">The files are settled, but the merge could not be committed. Get repository changes or send project changes to try again.</p>
  {:else if status.merging}
    <div class="git-box">
      <p>These files changed in both the project and the repository. Choose which version to keep.</p>
      <ul class="git-conflicts" aria-label="Files that changed on both sides">
        {#each status.merging.conflicts as path (path)}
          <li class="git-conflict">
            <span class="git-path" title={path}>{path}</span>
            <span class="git-choices">
              <button type="button" class="git-action" disabled={busy} onclick={() => connection.resolve(path, "project")}>Keep project's</button>
              <button type="button" class="git-action" disabled={busy} onclick={() => connection.resolve(path, "repository")}>Take repository's</button>
            </span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  <div class="git-foot">
    <button type="button" class="git-action" disabled={busy} onclick={() => connection.disconnect()}>
      {link.busy === "disconnect" ? "Disconnecting…" : "Disconnect"}
    </button>
    <p class="panel-meta">Disconnecting forgets the copy on this computer. The repository and the project stay as they are.</p>
  </div>
{/snippet}

<div class="git-web">
  <section class="git-block" aria-labelledby="git-web-repository">
    <h3 id="git-web-repository" class="panel-section-title">Repository</h3>
    <p class="git-push" role="status" title={pushTitle}>{pushText}</p>
    {#if !desktop}
      <p class="git-lead">To send and get changes with git, open this project in the LibrePaper desktop app.</p>
      <p class="git-lead">You can also keep a GitHub repository in sync with this project through a <a href="{DOCS}#github-action" target="_blank" rel="noopener noreferrer">GitHub Action</a>, with no desktop app. <a href={DOCS} target="_blank" rel="noopener noreferrer">Read the Git docs</a> to set it up.</p>
    {/if}
  </section>

  {#if desktop && connection}
    <section class="git-block" aria-labelledby="git-web-connect">
      <h3 id="git-web-connect" class="panel-section-title">{status?.connected ? "Connected repository" : "Connect a repository"}</h3>
      {#if !link.loaded}
        <p class="panel-muted" role="status">Loading…</p>
      {:else if status?.connected}
        {@render connected()}
      {:else if status}
        <p class="git-lead">Connect this project to a git repository. Git uses your own sign-in.</p>
        {#if !status.signed_in}{@render signIn()}{/if}
        {@render connectForm()}
      {/if}
      {#if failure || link.problem}<p class="lp-text-error git-error" role="alert">{failure || link.problem}</p>{/if}
      {#if link.signInHelp}
        <GitSignInHelp remote={link.signInHelp.remote} detail={link.signInHelp.detail} {busy}
                       onretry={() => connection.retry()} ondismiss={() => connection.dismiss()} />
      {/if}
    </section>
  {/if}

  {#if desktop}
    <p class="panel-meta git-note"><a href={DOCS} target="_blank" rel="noopener noreferrer">Read the Git docs</a>. The docs also show a <a href="{DOCS}#github-action" target="_blank" rel="noopener noreferrer">GitHub Action</a> that keeps a repository in step without this computer.</p>
  {/if}
</div>

<style>
  .git-web { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 5); }
  .git-block + .git-block { padding-top: calc(var(--spacing) * 4); border-top: 1px solid var(--color-divider); }
  .git-lead { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-note { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-push { margin: calc(var(--spacing) * 2) 0 0; font-weight: 500; overflow-wrap: anywhere; }
  .git-remote { margin: calc(var(--spacing) * 1.5) 0 0; font-weight: 500; overflow-wrap: anywhere; }
  .git-line { margin: calc(var(--spacing) * 3) 0 0; overflow-wrap: anywhere; }
  .git-form { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-field { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--spacing); }
  .git-input { width: 100%; min-width: 0; }
  .git-box { margin-top: calc(var(--spacing) * 3); padding: calc(var(--spacing) * 2); border-radius: var(--radius-base); background: var(--color-row-hover); overflow-wrap: anywhere; }
  .git-box p { margin: 0 0 calc(var(--spacing) * 1.5); }
  .git-box p:last-child { margin-bottom: 0; }
  .git-list { margin: 0 0 calc(var(--spacing) * 1.5); padding-inline-start: calc(var(--spacing) * 4); font-size: var(--panel-meta-size); }
  .git-conflicts { margin: calc(var(--spacing) * 2) 0 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .git-conflict { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * .5); padding-block: calc(var(--spacing) * 1.5); border-bottom: 1px solid var(--color-divider); }
  .git-path { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-choices { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 1); margin-inline-start: calc(var(--spacing) * -1.5); }
  .git-action { padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; cursor: pointer; }
  .git-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .git-action:disabled { opacity: .5; cursor: default; }
  .git-foot { margin-top: calc(var(--spacing) * 6); padding-top: calc(var(--spacing) * 3); border-top: 1px solid var(--color-divider); }
  .git-foot .git-action { margin-inline-start: calc(var(--spacing) * -1.5); }
  .git-foot p { margin: calc(var(--spacing) * 1) 0 0; }
  .git-error { margin: calc(var(--spacing) * 2) 0 0; overflow-wrap: anywhere; }
  .git-web a { color: var(--color-link); }
</style>
