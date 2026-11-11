<script>
  // A web project's Git panel. The project cannot run git, so this panel runs
  // nothing on the project itself. What the server knows (the last push) belongs
  // to the controller (lib/reader/web-git.svelte.js). In the desktop app the
  // panel also connects the project to a git repository, either a folder on this
  // computer that the person chose or a remote address. Get repository changes
  // and Send project changes move changes between that repository and the
  // project. That state belongs to lib/reader/web-connection.svelte.js; this
  // draws it. The GitHub Action that keeps a repository in step is described in
  // the docs.
  import { untrack } from "svelte";
  import { since } from "../../../lib/dates.js";
  import { locale } from "../../../lib/locale.js";
  import { m } from "../../../lib/paraglide/messages.js";
  import { repositoryInput } from "../../../lib/git-url.js";
  import { GIT_DOCS_URL as DOCS } from "../../../lib/site-links.js";
  import Icon from "../../Icon.svelte";
  import GitSignInHelp from "./GitSignInHelp.svelte";

  let {
    web,
    slug,
    linkKey = "",
    open = false,
    desktop = false,
    // The desktop app's connection to a repository. An editor in the desktop app has one.
    connection = null,
  } = $props();

  const view = $derived(web.state);
  const link = $derived(connection?.state);
  const status = $derived(link?.status ?? null);
  const busy = $derived(Boolean(link?.busy));
  // A merge blocks both transfers only while a file waits for a choice. One
  // whose files are settled but whose commit failed is retried by either.
  const merging = $derived(Boolean(status?.merging?.conflicts?.length));
  // What a refused transfer is called in the panel's sentences.
  const ACTION = { pull: m.gitweb_action_pull(), push: m.gitweb_action_push() };

  // A local repository is a folder on this computer; the status names its path.
  const local = $derived(Boolean(status?.local));
  const missing = $derived(Boolean(status?.missing));

  // The typed repository address or local path, and the problem with it.
  let address = $state("");
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

  // The project's last change from a repository, which the server records as a
  // push. Nothing is shown while it loads or when there was none.
  const pushLine = $derived.by(() => {
    if (view.pushProblem) return m.gitweb_push_problem();
    if (!view.push) return "";
    const when = since(view.push.at);
    const by = view.push.by;
    const label = view.push.label;
    if (by) return label ? m.gitweb_push_sent_by_label({ by, when, label }) : m.gitweb_push_sent_by({ by, when });
    return label ? m.gitweb_push_sent_label({ when, label }) : m.gitweb_push_sent({ when });
  });
  const pushTitle = $derived(view.push?.at ? new Date(view.push.at).toLocaleString(locale()) : "");

  async function connect(event) {
    event.preventDefault();
    if (busy) return;
    const input = repositoryInput(address);
    failure = input.problem;
    if (failure) return;
    if (input.kind === "local") await connection.connectLocal(input.value);
    else await connection.connect(input.value);
    if (connection.state.status?.connected) address = "";
  }

  // The folder chooser fills the field; the person still presses Connect.
  async function browse() {
    if (busy) return;
    failure = "";
    const path = await connection.choose();
    if (path) address = path;
  }
</script>

{#snippet signIn()}
  <p class="git-lead">{m.gitweb_signin_lead()}</p>
  <div class="git-row">
    <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={() => connection.login()}>
      {link.busy === "login" ? m.gitweb_sign_in_busy() : m.common_sign_in()}
    </button>
  </div>
{/snippet}

{#snippet problemLine()}
  {#if failure || link.problem}<p class="lp-text-error git-error" role="alert">{failure || link.problem}</p>{/if}
{/snippet}

{#snippet connectForm()}
  <form class="git-form" onsubmit={connect}>
    <label class="panel-meta" for="git-web-address">{m.gitweb_address_label()}</label>
    <div class="git-field-row">
      <input id="git-web-address" class="input git-input" type="text" placeholder="https://github.com/owner/repo.git" autocomplete="off" spellcheck="false"
             bind:value={address} disabled={busy} oninput={() => { failure = ""; }} />
      <button type="button" class="btn btn-sm lp-control-tonal-brand git-browse" disabled={busy} onclick={browse}>
        <Icon name="folder" size={14} />
        {link.busy === "choose" ? m.common_choosing() : m.gitweb_browse()}
      </button>
    </div>
    {@render problemLine()}
    <div class="git-row">
      <button type="submit" class="btn btn-sm lp-control-brand" disabled={busy || !address.trim()}>
        {link.busy === "connect" || link.busy === "connect-local" ? m.common_connecting() : m.gitweb_connect()}
      </button>
    </div>
    <p class="panel-meta git-note">{m.gitweb_credentials_note()}</p>
  </form>
{/snippet}

{#snippet disconnectFoot()}
  <div class="git-foot">
    <button type="button" class="git-action" disabled={busy} onclick={() => connection.disconnect()}>
      {link.busy === "disconnect" ? m.gitweb_disconnecting() : m.gitweb_disconnect()}
    </button>
    <p class="panel-meta">{local ? m.gitweb_disconnect_local_note() : m.gitweb_disconnect_note()}</p>
  </div>
{/snippet}

{#snippet connected()}
  {#if missing}
    <p class="git-remote" title={status.path}>{m.gitweb_repo_path({ path: status.path })}</p>
    <p class="git-lead" role="alert">{m.gitweb_folder_gone()}</p>
    {@render disconnectFoot()}
  {:else}
    <p class="git-remote" title={local ? status.path : status.url}>{m.gitweb_repo_branch({ location: local ? status.path : status.url, branch: status.branch })}</p>
    {#if !status.signed_in}
      {@render signIn()}
    {:else}
      <div class="git-row">
        <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || merging} onclick={() => connection.pull()}>
          {link.busy === "pull" ? m.gitweb_get_changes_busy() : m.gitweb_get_changes()}
        </button>
        <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || merging} onclick={() => connection.push()}>
          {link.busy === "push" ? m.gitweb_send_changes_busy() : m.gitweb_send_changes()}
        </button>
      </div>
      <p class="panel-meta git-note">{m.gitweb_transfer_note()}</p>
    {/if}

    {#if link.result}<p class="git-line" role="status">{link.result}</p>{/if}
    {#if link.interrupted}
      <div class="git-box" role="alert">
        <p>{link.interrupted}</p>
        <p>{m.gitweb_interrupted_wait()}</p>
        <div class="git-row">
          <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={() => connection.retryTransfer()}>
            {link.busy === "retry" ? m.gitweb_retry_busy() : m.gitweb_retry_transfer()}
          </button>
        </div>
      </div>
    {/if}
    {#if link.refusal?.reason === "changed"}
      <div class="git-box" role="alert">
        <p>{m.gitweb_changed_files()}</p>
        <ul class="git-list">{#each link.refusal.paths as path (path)}<li>{path}</li>{/each}</ul>
        <p>{m.gitweb_get_changes_again()}</p>
      </div>
    {:else if link.refusal?.reason === "suggestion"}
      <div class="git-box" role="alert">
        <p>{m.gitweb_suggestion_refusal({ path: link.refusal.path || m.gitweb_a_file(), action: ACTION[link.refusal.action] })}</p>
      </div>
    {/if}

    {#if status.merging && !merging && local}
      <p class="git-lead" role="status">{m.gitweb_merge_wait_local()}</p>
    {:else if status.merging && !merging}
      <p class="git-lead" role="status">{m.gitweb_merge_wait_remote()}</p>
    {:else if status.merging && local}
      <div class="git-box">
        <p>{m.gitweb_conflict_both()}</p>
        <ul class="git-list" aria-label={m.gitweb_conflict_files()}>{#each status.merging.conflicts as path (path)}<li>{path}</li>{/each}</ul>
        <p>{m.gitweb_conflict_resolve_local()}</p>
      </div>
    {:else if status.merging}
      <div class="git-box">
        <p>{m.gitweb_conflict_both_choose()}</p>
        <ul class="git-conflicts" aria-label={m.gitweb_conflict_files()}>
          {#each status.merging.conflicts as path (path)}
            <li class="git-conflict">
              <span class="git-path" title={path}>{path}</span>
              <span class="git-choices">
                <button type="button" class="git-action" disabled={busy} onclick={() => connection.resolve(path, "project")}>{m.gitweb_keep_project()}</button>
                <button type="button" class="git-action" disabled={busy} onclick={() => connection.resolve(path, "repository")}>{m.gitweb_take_repository()}</button>
              </span>
            </li>
          {/each}
        </ul>
      </div>
    {/if}

    {@render disconnectFoot()}
  {/if}
{/snippet}

<div class="git-web">
  <section aria-labelledby="git-web-repository">
    <h3 id="git-web-repository" class="panel-section-title">{m.gitweb_repository_heading()}</h3>
    {#if pushLine}<p class="git-push" role="status" title={pushTitle}>{pushLine}</p>{/if}
    {#if !desktop}
      <p class="git-lead">{m.gitweb_open_desktop()}</p>
    {:else if connection}
      {#if !link.loaded}
        <p class="panel-muted" role="status">{m.common_loading()}</p>
      {:else if status?.connected}
        {@render connected()}
        {@render problemLine()}
      {:else if status}
        <p class="git-lead">{m.gitweb_connect_lead()}</p>
        {#if !status.signed_in}{@render signIn()}{/if}
        {@render connectForm()}
      {:else}
        {@render problemLine()}
      {/if}
      {#if link.signInHelp}
        <GitSignInHelp remote={link.signInHelp.remote} detail={link.signInHelp.detail} {busy}
                       onretry={() => connection.retry()} ondismiss={() => connection.dismiss()} />
      {/if}
    {/if}
  </section>

  <p class="panel-meta git-foot-note"><a href={DOCS} target="_blank" rel="noopener noreferrer">{m.gitweb_read_git_docs()}</a>. {m.gitweb_docs_show_action()} <a href="{DOCS}#github-action" target="_blank" rel="noopener noreferrer">GitHub Action</a> {m.gitweb_docs_keeps_repo()}</p>
</div>

<style>
  .git-web { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 5); }
  .git-lead { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-note { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-push { margin: calc(var(--spacing) * 2) 0 0; font-weight: 500; overflow-wrap: anywhere; }
  .git-remote { margin: calc(var(--spacing) * 1.5) 0 0; font-weight: 500; overflow-wrap: anywhere; }
  .git-line { margin: calc(var(--spacing) * 3) 0 0; overflow-wrap: anywhere; }
  .git-form { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-field-row { display: flex; gap: calc(var(--spacing) * 2); align-items: center; }
  .git-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-input { flex: 1 1 auto; min-width: 0; }
  .git-browse { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); flex: none; }
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
  .git-foot-note { margin: 0; overflow-wrap: anywhere; }
  .git-error { margin: calc(var(--spacing) * 2) 0 0; overflow-wrap: anywhere; }
  .git-web a { color: var(--color-link); }
  /* Phones: the connect field keeps 16px so iOS does not zoom, and the text
     actions and buttons are finger-sized. */
  @media (max-width: 767px) {
    .git-input { font-size: 1rem; }
    .git-action { display: inline-flex; align-items: center; min-height: 2.75rem; }
    .git-web .btn { min-height: 2.75rem; }
  }
</style>
