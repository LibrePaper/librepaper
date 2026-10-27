<script>
  // The LibrePaper section of a folder project's Git panel: sign in, link the
  // folder to a web project or publish it as one, then get the changes from the
  // web and send the changes made here. The state and the actions belong to
  // the controller (lib/reader/local-sync.svelte.js); this draws them.
  import Icon from "../../Icon.svelte";

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

  // What a pull in a git repository did: the web versions it took in as
  // commits, then how git merged them.
  const MERGED = { "fast-forward": "Fast-forwarded", merged: "Merged", already: "Already merged", skipped: "Not merged" };
  const versionsText = (count) => (count === 0 ? "No new versions on the web." : `${plural(count, "version")} fetched as commits.`);

  // One sentence for what a pull did.
  function pulledText(pull, conflicts) {
    const parts = [];
    if (pull.written) parts.push(`wrote ${plural(pull.written, "file")}`);
    if (pull.trashed) parts.push(`moved ${plural(pull.trashed, "file")} to the trash`);
    if (!parts.length) return conflicts.length ? "" : "Nothing new on the web.";
    const said = parts.join(" and ");
    return `${said[0].toUpperCase()}${said.slice(1)}.`;
  }

  // What linking would meet, as sentences with the paths under each one. A
  // group with no path is left out.
  const previewLines = $derived.by(() => {
    const found = view.preview;
    if (!found) return [];
    const are = (count) => `${plural(count, "file")} ${count === 1 ? "is" : "are"}`;
    const they = (count) => (count === 1 ? "It" : "They");
    const lines = [
      { key: "project_only", paths: found.project_only, text: `${are(found.project_only.length)} only in the project. ${they(found.project_only.length)} will be added to this folder.` },
      { key: "local_only", paths: found.local_only, text: `${are(found.local_only.length)} only in this folder. ${found.local_only.length === 1 ? "It stays here until you send it." : "They stay here until you send them."}` },
      { key: "different", paths: found.different, text: `${are(found.different.length)} in both with different contents. ${status?.git ? "Git merges them." : "You choose which version to keep."}` },
      { key: "equal", paths: found.equal, text: `${are(found.equal.length)} in both with the same contents.` },
      { key: "blocked", paths: found.blocked.map(([path, why]) => `${path}: ${why}`), text: `${are(found.blocked.length)} not transferred.` },
      { key: "excluded", paths: found.excluded, text: `${are(found.excluded.length)} left out by this folder's rules.` },
    ];
    return lines.filter((line) => line.paths.length);
  });

  // The branch of a repository. Sync uses one branch, confirmed once. The
  // checkout may be on another branch or detached, which only the person
  // fixes with git.
  const git = $derived(Boolean(status?.git));
  const asksBranch = $derived(git && !status?.branch && Boolean(status?.checked_out));
  const detachedUnbound = $derived(git && !status?.branch && !status?.checked_out);
  const wrongBranch = $derived(git && Boolean(status?.branch) && status.checked_out !== status.branch);

  async function submitPush(event) {
    event.preventDefault();
    await sync.push(message);
    if (sync.state.pushed) typedMessage = null;
  }
</script>

{#snippet results()}
  <div class="sync-results" role="status" aria-live="polite">
    {#if view.pull?.repository}
      <p class="sync-line">{versionsText(view.pull.versions)}</p>
      {#if view.pull.merge === "conflicts"}
        <div class="sync-box" role="alert">
          <p>Merge conflicts in {view.pull.paths.join(", ")}. Resolve them in the editor, then Commit in the Repository section.</p>
        </div>
      {:else if view.pull.merge === "dirty"}
        <div class="sync-box" role="alert">
          <p>Commit your changes first: {view.pull.paths.join(", ")}</p>
        </div>
      {:else}
        <p class="sync-line">{MERGED[view.pull.merge]}.</p>
      {/if}
      {#if view.pull.trimmed}
        <p class="sync-line panel-meta">The history of the web project was trimmed, so this took its current version as the new base.</p>
      {/if}
    {:else if view.pull}
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
      <p class="sync-line">{view.pushed.nothing ? "Nothing to send to web." : `Sent ${plural(view.pushed.files, "file")} to web.`}</p>
    {/if}
    {#if view.refusal?.reason === "changed"}
      <div class="sync-box" role="alert">
        <p>The web project changed {view.refusal.paths.length === 1 ? "this file" : "these files"} since your last get from web.</p>
        {#if view.refusal.paths.length}
          <ul class="sync-list">{#each view.refusal.paths as path (path)}<li>{path}</li>{/each}</ul>
        {/if}
        <p>Get changes from web first.</p>
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

<section class="git-block sync-panel" aria-labelledby="git-librepaper-title">
  <h3 id="git-librepaper-title" class="panel-section-title">LibrePaper</h3>
  {#if !view.loaded}
    <p class="panel-muted" role="status">Loading…</p>
  {:else if !status}
    <p class="panel-muted">The sync state could not be read.</p>
    <div class="sync-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => sync.refresh()}>Try again</button>
    </div>
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
  {:else if !status.linked && view.preview}
    <p class="sync-lead">Linking compares this folder with the project, then takes the project in. Nothing has changed yet.</p>
    <p class="sync-project"><Icon name="link" size={14} /><span title={view.preview.url}>{view.preview.url}</span></p>
    {#if previewLines.length}
      <ul class="sync-preview" aria-label="What linking would meet">
        {#each previewLines as line (line.key)}
          <li>
            <details>
              <summary>{line.text}</summary>
              <ul class="sync-list">{#each line.paths as path (path)}<li>{path}</li>{/each}</ul>
            </details>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="sync-line">Both the project and this folder are empty.</p>
    {/if}
    <div class="sync-row">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={() => sync.confirmLink()}>
        {view.busy === "link" ? "Linking…" : "Link"}
      </button>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => sync.cancelPreview()}>Cancel</button>
    </div>
  {:else if !status.linked}
    <p class="sync-lead">This folder is not linked to a project on the web.</p>
    <form class="sync-block" onsubmit={(event) => { event.preventDefault(); void sync.previewLink(url); }}>
      <label class="sync-field panel-meta">Link to a project
        <input class="input sync-input" type="url" bind:value={url} disabled={busy}
               placeholder={`${location.origin}/docs/your-project`} autocomplete="off" spellcheck="false" />
      </label>
      <div class="sync-row">
        <button type="submit" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !url.trim()}>
          {view.busy === "preview" ? "Comparing…" : "Preview link"}
        </button>
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
    {#if status.pending}
      <div class="sync-box" role="alert">
        <p>An earlier send to the web was interrupted before its outcome was known. Choose Send changes to web to finish it. Getting changes, linking and unlinking wait until then.</p>
      </div>
    {/if}
    {#if asksBranch}
      <div class="sync-box" role="alert">
        <p>Choose the branch to sync. This folder is on branch {status.checked_out}, and sync will use it from now on.</p>
        <div class="sync-row">
          <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={() => sync.confirmBranch(status.checked_out)}>Sync branch {status.checked_out}</button>
        </div>
      </div>
    {:else if detachedUnbound}
      <div class="sync-box" role="alert">
        <p>HEAD is detached, so there is no branch to sync. Check out a branch with git first, such as git switch main.</p>
      </div>
    {:else if wrongBranch}
      <div class="sync-box" role="alert">
        <p>Sync uses branch {status.branch}, but this folder is {status.checked_out ? `on branch ${status.checked_out}` : "on a detached HEAD"}. Run git switch {status.branch} and try again. LibrePaper does not change branches.</p>
      </div>
    {:else if git && status.branch}
      <p class="sync-line panel-meta sync-git">Syncing branch {status.branch}.</p>
    {/if}
    {#if status.git}
      <p class="sync-line panel-meta sync-git">This folder is a git repository, so getting changes from web takes each web version in as a commit and git merges it.</p>
    {/if}
    <div class="sync-row">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => sync.pull()}>
        {view.busy === "pull" ? "Getting changes from web…" : "Get changes from web"}
      </button>
    </div>
    <form class="sync-block" onsubmit={submitPush}>
      <label class="sync-field panel-meta">Message
        <input class="input sync-input" type="text" value={message} maxlength="200" disabled={busy}
               oninput={(event) => (typedMessage = event.currentTarget.value)} />
      </label>
      <div class="sync-row">
        <button type="submit" class="btn btn-sm lp-control-brand" disabled={busy}>
          {view.busy === "push" ? "Sending changes to web…" : "Send changes to web"}
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
  .sync-git { margin: calc(var(--spacing) * 3) 0 0; }
  .sync-preview { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 1.5); margin: calc(var(--spacing) * 3) 0 0; padding: 0; list-style: none; }
  .sync-preview summary { cursor: pointer; overflow-wrap: anywhere; }
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
