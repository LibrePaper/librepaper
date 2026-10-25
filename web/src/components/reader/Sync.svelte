<script>
  // The Sync panel of a folder project: sign in, link the folder to a web
  // project or publish it as one, then pull what changed on the web and push
  // what changed here. The state and the actions belong to the controller
  // (lib/reader/local-sync.svelte.js); this draws them.
  import PanelHeader from "../PanelHeader.svelte";
  import Icon from "../Icon.svelte";

  let { sync, open = false, defaultTitle = "" } = $props();

  const view = $derived(sync.state);
  const status = $derived(view.status);
  const busy = $derived(view.busy !== "");

  // What the person typed. Until they type, the fields show their defaults.
  let url = $state("");
  let typedTitle = $state(null);
  let typedMessage = $state(null);
  const title = $derived(typedTitle ?? defaultTitle);
  const message = $derived(typedMessage ?? sync.defaultMessage());

  // The state is read when the panel opens and then again every half minute
  // while it stays open and the window is showing, because the folder and the
  // web project change under it.
  $effect(() => {
    if (!open) return;
    void sync.refresh();
    const timer = setInterval(() => {
      if (document.visibilityState !== "hidden") void sync.refresh();
    }, 30_000);
    return () => clearInterval(timer);
  });

  const plural = (count, noun) => `${count} ${noun}${count === 1 ? "" : "s"}`;
  const here = $derived(Number(status?.changes?.here) || 0);
  // With the site out of reach the counts were never read, so they are not
  // shown as zero.
  const unread = $derived(typeof status?.error === "string" && status.error !== "");
  const hereText = $derived(unread ? "Changes here unknown" : here === 0 ? "No changes here" : `${plural(here, "change")} here`);
  const webText = $derived(unread ? "The web project could not be reached" : status?.changes?.web ? "The web project changed" : "Up to date");
  const projectHref = $derived(status?.site && status?.slug ? `${String(status.site).replace(/\/+$/, "")}/docs/${status.slug}` : "");

  // One sentence for what a pull did.
  function pulledText(pull, conflicts) {
    const parts = [];
    if (pull.written) parts.push(`wrote ${plural(pull.written, "file")}`);
    if (pull.trashed) parts.push(`moved ${plural(pull.trashed, "file")} to the trash`);
    if (!parts.length) return conflicts.length ? "" : "Nothing new on the web.";
    const said = parts.join(" and ");
    return `${said[0].toUpperCase()}${said.slice(1)}.`;
  }

  async function submitPush(event) {
    event.preventDefault();
    await sync.push(message);
    if (sync.state.pushed) typedMessage = null;
  }
</script>

{#snippet results()}
  <div class="sync-results" role="status" aria-live="polite">
    {#if view.pull}
      {@const said = pulledText(view.pull, view.conflicts)}
      {#if said}<p class="sync-line">{said}</p>{/if}
      {#if view.pull.trimmed}
        <p class="sync-line panel-meta">The history of the web project was trimmed, so this took its current version as the new base.</p>
      {/if}
      {#if view.pull.skipped?.length}
        <p class="sync-line panel-meta">Left as they are, because this folder excludes them (ignored, hidden or not text): {view.pull.skipped.join(", ")}.</p>
      {/if}
    {/if}
    {#if view.conflicts.length}
      <p class="sync-line">{plural(view.conflicts.length, "file")} changed here and on the web. Choose which to keep.</p>
      <ul class="sync-conflicts" aria-label="Files that changed on both sides">
        {#each view.conflicts as path (path)}
          <li class="sync-conflict">
            <span class="sync-path" title={path}>{path}</span>
            <span class="sync-choices">
              <button type="button" class="sync-action" disabled={busy} onclick={() => sync.keepMine(path)}>Keep mine</button>
              <button type="button" class="sync-action" disabled={busy} onclick={() => sync.takeTheirs(path)}>Take theirs</button>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
    {#if view.pushed}
      <p class="sync-line">{view.pushed.nothing ? "Nothing to push." : `Pushed ${plural(view.pushed.files, "file")}.`}</p>
    {/if}
    {#if view.refusal?.reason === "changed"}
      <div class="sync-box" role="alert">
        <p>The web project changed {view.refusal.paths.length === 1 ? "this file" : "these files"} since your last pull.</p>
        {#if view.refusal.paths.length}
          <ul class="sync-list">{#each view.refusal.paths as path (path)}<li>{path}</li>{/each}</ul>
        {/if}
        <p>Pull first.</p>
      </div>
    {:else if view.refusal?.reason === "suggestion"}
      <div class="sync-box" role="alert">
        <p>A pending suggestion in {view.refusal.path || "a file"} stops the push. Accept or reject it on the web, then push again.</p>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet projectLine()}
  <p class="sync-project">
    <Icon name="link" size={14} />
    {#if projectHref}
      <a href={projectHref} target="_blank" rel="noopener noreferrer" title={projectHref}>{status.title || status.slug}</a>
    {:else}
      <span>{status.title || status.slug}</span>
    {/if}
  </p>
{/snippet}

{#snippet unlinkFoot()}
  <div class="sync-foot">
    <button type="button" class="sync-action" disabled={busy} onclick={() => sync.unlink()}>Unlink</button>
    <p class="panel-meta">Unlinking keeps the folder and the web project as they are.</p>
  </div>
{/snippet}

<section class="panel sync-panel" aria-label="Sync">
  <PanelHeader title="Sync" />
  {#if !view.loaded}
    <p class="panel-muted" role="status">Loading…</p>
  {:else if !status}
    <p class="panel-muted">The sync state could not be read.</p>
    <div class="sync-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => sync.refresh()}>Try again</button>
    </div>
  {:else if status.git}
    <!-- Pull, push and publish would write into a repository, so the desktop
         app refuses them and the panel does not offer them. -->
    <p class="sync-lead">This folder is a git repository. Sync it with git in a terminal, or with librepaper pull and push.</p>
    {#if status.linked}
      {@render projectLine()}
      {@render unlinkFoot()}
    {/if}
  {:else if status.other_site}
    <!-- Before the sign-in question: unlinking needs no token, and signing in
         to this site would not change whose link it is. -->
    <p class="sync-lead">This folder is linked to a project on another site. Unlink it to use it with this one.</p>
    {@render unlinkFoot()}
  {:else if !status.signed_in && view.waiting}
    <!-- This page has no session on the server, so the code is approved on the
         server's own page in another tab. The link is there for a browser that
         blocked the tab from opening. -->
    <p class="sync-lead">Approve this code in the other tab to finish signing in.</p>
    <p class="sync-code">{view.waiting.userCode}</p>
    <p class="sync-line" role="status">Waiting for the code to be approved in the other tab</p>
    <p class="sync-line panel-meta">
      <a href={view.waiting.url} target="_blank" rel="noopener noreferrer">Open the approval page again</a>
    </p>
    <div class="sync-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => sync.cancelLogin()}>Cancel</button>
    </div>
  {:else if !status.signed_in}
    <p class="sync-lead">Sign in to keep this folder and a project on the web in step. This uses your account on this server.</p>
    <div class="sync-row">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={() => sync.login()}>
        {view.busy === "login" ? "Signing in…" : "Sign in"}
      </button>
    </div>
  {:else if !status.linked}
    <p class="sync-lead">This folder is not linked to a project on the web.</p>
    <form class="sync-block" onsubmit={(event) => { event.preventDefault(); void sync.link(url); }}>
      <label class="sync-field panel-meta">Link to a project
        <input class="input sync-input" type="url" bind:value={url} disabled={busy}
               placeholder={`${location.origin}/docs/your-project`} autocomplete="off" spellcheck="false" />
      </label>
      <div class="sync-row">
        <button type="submit" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !url.trim()}>Link</button>
      </div>
    </form>
    <form class="sync-block" onsubmit={(event) => { event.preventDefault(); void sync.publish(title); }}>
      <label class="sync-field panel-meta">Or publish this folder as a new project
        <input class="input sync-input" type="text" value={title} maxlength="200" disabled={busy}
               oninput={(event) => (typedTitle = event.currentTarget.value)} />
      </label>
      <div class="sync-row">
        <button type="submit" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !title.trim()}>Publish</button>
      </div>
    </form>
  {:else}
    {@render projectLine()}
    <ul class="sync-facts" aria-label="Where this folder stands">
      <li><Icon name={unread ? "triangle-alert" : here ? "pencil" : "check"} size={14} /><span>{hereText}</span></li>
      <li><Icon name={unread ? "triangle-alert" : status.changes?.web ? "download" : "check"} size={14} /><span>{webText}</span></li>
    </ul>
    <div class="sync-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => sync.pull()}>
        {view.busy === "pull" ? "Pulling…" : "Pull"}
      </button>
    </div>
    <form class="sync-block" onsubmit={submitPush}>
      <label class="sync-field panel-meta">Message
        <input class="input sync-input" type="text" value={message} maxlength="200" disabled={busy}
               oninput={(event) => (typedMessage = event.currentTarget.value)} />
      </label>
      <div class="sync-row">
        <button type="submit" class="btn btn-sm lp-control-brand" disabled={busy}>
          {view.busy === "push" ? "Pushing…" : "Push"}
        </button>
      </div>
    </form>
    {@render results()}
    {@render unlinkFoot()}
  {/if}
  {#if view.error}
    <p class="lp-text-error sync-error" role="alert">{view.error}</p>
  {/if}
</section>

<style>
  .sync-lead { margin: 0 0 calc(var(--spacing) * 3); }
  .sync-code { margin: 0 0 calc(var(--spacing) * 3); font-family: var(--font-mono, monospace); font-size: var(--text-xl, 1.25rem); letter-spacing: .1em; overflow-wrap: anywhere; user-select: all; }
  .sync-block { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 4); }
  .sync-field { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--spacing); }
  .sync-input { width: 100%; min-width: 0; }
  .sync-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .sync-block > .sync-row { margin-top: 0; }
  .sync-project { display: flex; align-items: center; gap: calc(var(--spacing) * 1.5); min-width: 0; margin: 0; font-weight: 500; }
  .sync-project :global(svg) { flex: none; color: var(--color-text-muted); }
  .sync-project a, .sync-project span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sync-project a { color: var(--color-link); }
  .sync-facts { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 1); margin: calc(var(--spacing) * 3) 0 0; padding: 0; list-style: none; }
  .sync-facts li { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }
  .sync-facts :global(svg) { flex: none; color: var(--color-text-muted); }
  .sync-results { margin-top: calc(var(--spacing) * 4); }
  .sync-results:empty { display: none; }
  .sync-line { margin: 0 0 calc(var(--spacing) * 2); overflow-wrap: anywhere; }
  .sync-box { margin-top: calc(var(--spacing) * 2); padding: calc(var(--spacing) * 2); border-radius: var(--radius-base); background: var(--color-row-hover); }
  .sync-box p { margin: 0 0 calc(var(--spacing) * 1.5); overflow-wrap: anywhere; }
  .sync-box p:last-child { margin-bottom: 0; }
  .sync-list { margin: 0 0 calc(var(--spacing) * 1.5); padding-inline-start: calc(var(--spacing) * 4); overflow-wrap: anywhere; font-size: var(--panel-meta-size); }
  .sync-conflicts { margin: 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .sync-conflict { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * .5); padding-block: calc(var(--spacing) * 1.5); border-bottom: 1px solid var(--color-divider); }
  .sync-path { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sync-choices { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 1); margin-inline-start: calc(var(--spacing) * -1.5); }
  .sync-action { padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; cursor: pointer; }
  .sync-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .sync-action:disabled { opacity: .5; cursor: default; }
  .sync-foot { margin-top: calc(var(--spacing) * 6); padding-top: calc(var(--spacing) * 3); border-top: 1px solid var(--color-divider); }
  .sync-foot .sync-action { margin-inline-start: calc(var(--spacing) * -1.5); }
  .sync-foot p { margin: calc(var(--spacing) * 1) 0 0; }
  .sync-error { margin: calc(var(--spacing) * 3) 0 0; overflow-wrap: anywhere; }
</style>
