<script>
  // A web project's Git panel. The project cannot run git, so this panel runs
  // nothing on the project itself. What the server knows (the last push) belongs
  // to the controller (lib/reader/web-git.svelte.js). In the desktop app the
  // panel also connects a git repository on this computer: the clone or the
  // folder is made by the app, and the reader is taken to the folder project.
  // The GitHub Action that keeps a repository in step is described in the docs.
  import { untrack } from "svelte";
  import { since } from "../../../lib/dates.js";
  import { remoteUrlProblem } from "../../../lib/git-url.js";

  let {
    web,
    slug,
    linkKey = "",
    open = false,
    desktop = false,
    // Each throws with the message the panel shows when the desktop app refuses.
    onclone,
    onfolder,
  } = $props();

  const DOCS = "/docs/git.html";
  const view = $derived(web.state);

  // The address that is typed, the problem with it, and the action that is
  // running ("clone" or "folder"), so both controls wait for it.
  let address = $state("");
  let running = $state("");
  let failure = $state("");

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

  const pushText = $derived.by(() => {
    if (!view.loaded) return "Checking for pushes…";
    if (view.pushProblem) return "The last push could not be read.";
    if (!view.push) return "No push yet.";
    const who = view.push.by ? ` by ${view.push.by},` : "";
    return `Last pushed${who} ${since(view.push.at)}${view.push.label ? `: ${view.push.label}` : ""}`;
  });
  const pushTitle = $derived(view.push?.at ? new Date(view.push.at).toLocaleString() : "");

  async function connect(event) {
    event.preventDefault();
    if (running) return;
    const problem = remoteUrlProblem(address);
    if (problem) {
      failure = problem;
      return;
    }
    running = "clone";
    failure = "";
    try {
      await onclone(address.trim());
    } catch (error) {
      failure = error?.message || "The desktop app could not clone that repository.";
    } finally {
      running = "";
    }
  }

  async function chooseFolder() {
    if (running) return;
    running = "folder";
    failure = "";
    try {
      await onfolder();
    } catch (error) {
      failure = error?.message || "The desktop app could not keep a folder for this project.";
    } finally {
      running = "";
    }
  }
</script>

<div class="git-web">
  <section class="git-block" aria-labelledby="git-web-repository">
    <h3 id="git-web-repository" class="panel-section-title">Repository</h3>
    <p class="git-push" role="status" title={pushTitle}>{pushText}</p>
    {#if !desktop}
      <p class="git-lead">To connect a git repository, open this project in the LibrePaper desktop app.</p>
    {/if}
  </section>

  {#if desktop}
    <section class="git-block" aria-labelledby="git-web-connect">
      <h3 id="git-web-connect" class="panel-section-title">Connect a repository</h3>
      <p class="git-lead">Work on this project in a git repository on this computer. Git uses your own sign-in.</p>
      <form class="git-form" onsubmit={connect}>
        <label class="git-field panel-meta">Repository address
          <input class="input git-input" type="text" placeholder="https://github.com/owner/repo.git" autocomplete="off" spellcheck="false"
                 bind:value={address} disabled={running !== ""} />
        </label>
        <div class="git-row">
          <button type="submit" class="btn btn-sm lp-control-tonal-brand" disabled={running !== ""}>
            {running === "clone" ? "Cloning…" : "Clone and connect…"}
          </button>
        </div>
      </form>
      <div class="git-row">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={running !== ""} onclick={chooseFolder}>Use a folder on this computer…</button>
      </div>
      <p class="panel-meta git-note">An empty folder, or a clone you already have.</p>
      {#if failure}<p class="lp-text-error git-error" role="alert">{failure}</p>{/if}
    </section>
  {/if}

  <p class="panel-meta git-note"><a href={DOCS} target="_blank" rel="noopener noreferrer">Read the Git docs</a>. The docs also show a <a href="{DOCS}#github-action" target="_blank" rel="noopener noreferrer">GitHub Action</a> that keeps a repository in step without this computer.</p>
</div>

<style>
  .git-web { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 5); }
  .git-block + .git-block { padding-top: calc(var(--spacing) * 4); border-top: 1px solid var(--color-divider); }
  .git-lead { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-note { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-push { margin: calc(var(--spacing) * 2) 0 0; font-weight: 500; overflow-wrap: anywhere; }
  .git-form { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-field { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--spacing); }
  .git-input { width: 100%; min-width: 0; }
  .git-error { margin: calc(var(--spacing) * 2) 0 0; overflow-wrap: anywhere; }
  .git-web a { color: var(--color-link); }
</style>
