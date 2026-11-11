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
  //
  // Every state has the same shape: a heading row, the content of the state, the
  // last activity when there is one, and one footer with the docs link (and
  // Disconnect when a repository is connected).
  import { untrack } from "svelte";
  import { since } from "../../../lib/dates.js";
  import { locale } from "../../../lib/locale.js";
  import { m } from "../../../lib/paraglide/messages.js";
  import { repositoryIdentity, repositoryInput } from "../../../lib/git-url.js";
  import { GIT_DOCS_URL as DOCS } from "../../../lib/site-links.js";
  import Icon from "../../Icon.svelte";
  import IconButton from "../../IconButton.svelte";
  import Modal from "../../Modal.svelte";
  import GitSignInHelp from "./GitSignInHelp.svelte";

  let {
    web,
    slug,
    linkKey = "",
    open = false,
    desktop = false,
    // The desktop app's connection to a repository. An editor in the desktop app has one.
    connection = null,
    // Opens the history panel, from the project's last change.
    onhistory = null,
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
  // The repository is shown only once the connection is read and the project is linked to one.
  const linked = $derived(Boolean(connection && link?.loaded && status?.connected));
  const identity = $derived(linked ? repositoryIdentity(status) : null);
  const branchLine = $derived([status?.branch, identity?.detail].filter(Boolean).join(" · "));

  // The typed repository address or local path, and the problem with it.
  let address = $state("");
  let failure = $state("");
  let disconnecting = $state(false);
  // Relative times ("just now") are read again every half minute while the panel is open.
  let now = $state(Date.now());

  // The copy button says so for a moment after it has copied the address.
  let copied = $state(false);
  let copiedTimer = 0;
  $effect(() => () => clearTimeout(copiedTimer));

  $effect(() => {
    if (!open) return;
    const project = slug;
    const options = { key: linkKey };
    untrack(() => { void web.refresh(project, options); });
    const timer = setInterval(() => {
      now = Date.now();
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

  const pushTitle = $derived(view.push?.at ? new Date(view.push.at).toLocaleString(locale()) : "");

  // The project's last change from a repository, which the server records as a
  // push. The sync crate writes it as "subject and N more" when several commits
  // came in together.
  const activity = $derived.by(() => {
    const push = view.push;
    if (!push) return null;
    const label = push.label ?? "";
    const match = /^(.*) and (\d+) more$/s.exec(label);
    const subject = (match ? match[1] : label).trim() || m.gitweb_activity_untitled();
    const more = match ? Number(match[2]) : 0;
    const parts = [];
    if (push.by) parts.push({ text: push.by });
    const when = since(push.at);
    if (when) parts.push({ text: when, title: pushTitle });
    if (more) parts.push({ text: m.gitweb_activity_more_commits({ count: more }) });
    return { subject, label, parts };
  });
  // The activity shows a problem reading the history instead of the change. A
  // missing folder has no repository to describe, so it shows no activity.
  const showActivity = $derived(Boolean((view.pushProblem || activity) && !(linked && missing)));

  // The heading says whether the repository is connected and working.
  const headingStatus = $derived.by(() => {
    if (!linked) return null;
    if (missing) return { icon: "triangle-alert", text: m.gitweb_status_missing(), tone: "lp-text-warning" };
    if (!status.signed_in) return { icon: "triangle-alert", text: m.gitweb_status_sign_in(), tone: "lp-text-warning" };
    return { icon: "check", text: m.remotesettings_status_connected(), tone: "git-ok" };
  });

  // The latest transfer this session, in one sentence.
  const summary = $derived.by(() => {
    const last = link?.last;
    if (!last) return null;
    void now;
    const when = since(new Date(last.at).toISOString());
    if (!last.ok) {
      const text = last.action === "get" ? m.gitweb_summary_get_failed()
        : last.action === "send" ? m.gitweb_summary_send_failed()
        : m.gitweb_summary_failed();
      return { ok: false, text, detail: last.detail || "" };
    }
    let text;
    if (last.action === "get") {
      text = last.nothing || !last.files
        ? m.gitweb_summary_get_nothing({ when })
        : m.gitweb_summary_get_files({ count: last.files, when });
    } else if (last.action === "send") {
      if (last.nothing) text = m.gitweb_summary_send_nothing({ when });
      else text = last.branch ? m.gitweb_summary_send({ branch: last.branch, when }) : m.gitweb_summary_send_plain({ when });
    } else if (last.action === "retry") {
      text = m.gitweb_summary_retry({ when });
    } else {
      // A settled merge says what to do next rather than when it happened.
      text = m.web_connection_merged();
    }
    return { ok: true, text, detail: "" };
  });

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

  async function copyAddress() {
    if (!identity) return;
    try {
      await navigator.clipboard.writeText(identity.full);
    } catch {
      // The browser refused the clipboard; the address is on screen to copy by hand.
      return;
    }
    copied = true;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => { copied = false; }, 1500);
  }
</script>

{#snippet errorLine(text)}
  <p class="git-error lp-text-error" role="alert"><Icon name="triangle-alert" size={14} /><span>{text}</span></p>
{/snippet}

{#snippet signIn()}
  <p class="git-lead">{m.gitweb_signin_lead()}</p>
  <div class="git-row">
    <button type="button" class="btn btn-sm lp-control-brand" disabled={busy} onclick={() => connection.login()}>
      {link.busy === "login" ? m.gitweb_sign_in_busy() : m.common_sign_in()}
    </button>
  </div>
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
    {#if failure || link.problem}{@render errorLine(failure || link.problem)}{/if}
    <div class="git-row">
      <button type="submit" class="btn btn-sm lp-control-brand" disabled={busy || !address.trim()}>
        {link.busy === "connect" || link.busy === "connect-local" ? m.common_connecting() : m.gitweb_connect()}
      </button>
    </div>
  </form>
{/snippet}

{#snippet signInHelp()}
  <GitSignInHelp remote={link.signInHelp.remote} detail={link.signInHelp.detail} {busy}
                 onretry={() => connection.retry()} ondismiss={() => connection.dismiss()} />
{/snippet}

{#snippet identityBlock()}
  <div class="git-identity">
    <Icon name={identity.icon} size={18} />
    <div class="git-identity-text">
      <div class="git-name-row">
        {#if identity.href}
          <a class="git-name" href={identity.href} title={identity.full} target="_blank" rel="noopener noreferrer">{identity.name}</a>
        {:else}
          <span class="git-name" title={identity.full}>{identity.name}</span>
        {/if}
        <IconButton icon="copy" size="btn-icon-sm"
                    label={copied ? m.tokenreveal_copied() : local ? m.gitweb_copy_path() : m.gitweb_copy_address()}
                    onclick={copyAddress} />
      </div>
      <p class="panel-meta git-detail">
        <Icon name="git-branch" size={12} />
        <span class="git-detail-text" title={branchLine}>{branchLine}</span>
      </p>
    </div>
  </div>
{/snippet}

{#snippet linkedBody()}
  {@render identityBlock()}
  {#if missing}
    <p class="git-lead" role="alert">{m.gitweb_folder_gone()}</p>
  {:else}
    {#if !status.signed_in}
      {@render signIn()}
    {:else}
      <div class="git-row">
        <button type="button" class="btn btn-sm lp-control-tonal-brand git-transfer" disabled={busy || merging}
                title={m.gitweb_get_tip()} onclick={() => connection.pull()}>
          <Icon name="arrow-down" size={14} />
          {link.busy === "pull" ? m.gitweb_get_changes_busy() : m.gitweb_get_changes()}
        </button>
        <button type="button" class="btn btn-sm lp-control-tonal-brand git-transfer" disabled={busy || merging}
                title={m.gitweb_send_tip()} onclick={() => connection.push()}>
          <Icon name="arrow-up" size={14} />
          {link.busy === "push" ? m.gitweb_send_changes_busy() : m.gitweb_send_changes()}
        </button>
      </div>
    {/if}

    {#if summary}
      <div class="git-summary-block">
        <p class="panel-meta git-summary" class:lp-text-error={!summary.ok} role="status">
          <span class="git-mark" class:git-ok={summary.ok}>
            <Icon name={summary.ok ? "check" : "triangle-alert"} size={14} />
          </span>
          <span>{summary.text}</span>
        </p>
        {#if summary.detail}
          <details class="git-details">
            <summary>{m.common_details()}</summary>
            <p>{summary.detail}</p>
          </details>
        {/if}
      </div>
    {/if}
    {#if link.problem && !(link.last && !link.last.ok)}{@render errorLine(link.problem)}{/if}
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

    {#if link.signInHelp}{@render signInHelp()}{/if}
  {/if}
{/snippet}

{#snippet activitySection()}
  <section class="git-section" aria-labelledby="git-web-activity">
    <div class="git-heading">
      <h3 id="git-web-activity" class="panel-section-title">{m.gitweb_last_activity()}</h3>
      {#if activity && onhistory}
        <button type="button" class="git-action" onclick={onhistory}>{m.gitweb_view_history()}</button>
      {/if}
    </div>
    {#if view.pushProblem}
      <p class="panel-meta git-activity-meta">{m.gitweb_push_problem()}</p>
    {:else}
      <p class="git-activity-subject" title={activity.label}>{activity.subject}</p>
      <p class="panel-meta git-activity-meta">
        {#each activity.parts as part, index (index)}{#if index > 0}{" · "}{/if}<span title={part.title}>{part.text}</span>{/each}
      </p>
    {/if}
  </section>
{/snippet}

{#snippet footer()}
  <footer class="git-section git-foot">
    <a class="panel-meta" href={DOCS} target="_blank" rel="noopener noreferrer">{m.gitweb_git_documentation()}</a>
    {#if linked}
      <button type="button" class="git-action" disabled={busy} onclick={() => (disconnecting = true)}>
        <Icon name="link-off" size={14} />
        {link.busy === "disconnect" ? m.gitweb_disconnecting() : m.gitweb_disconnect()}
      </button>
    {/if}
  </footer>
{/snippet}

<div class="git-web">
  <section aria-labelledby="git-web-repository">
    <div class="git-heading">
      <h3 id="git-web-repository" class="panel-section-title">{m.gitweb_repository_heading()}</h3>
      {#if headingStatus}
        <span class="git-status text-sm {headingStatus.tone}"><Icon name={headingStatus.icon} size={14} /><span>{headingStatus.text}</span></span>
      {/if}
    </div>
    {#if !desktop}
      <p class="git-lead">{m.gitweb_open_desktop()}</p>
    {:else if connection}
      {#if !link.loaded}
        <p class="panel-muted" role="status">{m.common_loading()}</p>
      {:else if status?.connected}
        {@render linkedBody()}
      {:else if status}
        <p class="git-lead">{m.gitweb_connect_lead()}</p>
        {#if !status.signed_in}{@render signIn()}{/if}
        {@render connectForm()}
      {:else if failure || link.problem}
        {@render errorLine(failure || link.problem)}
      {/if}
      {#if link.signInHelp && !linked}{@render signInHelp()}{/if}
    {/if}
  </section>

  {#if showActivity}
    {@render activitySection()}
  {/if}

  {@render footer()}
</div>

<Modal bind:open={disconnecting}
       title={m.gitweb_disconnect_title()}
       description={local ? m.gitweb_disconnect_local_body() : m.gitweb_disconnect_remote_body()}
       confirm={{ label: m.gitweb_disconnect(), tone: "error", onclick: () => { disconnecting = false; connection?.disconnect(); } }} />

<style>
  .git-web { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 5); }
  .git-heading { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 2); }
  .git-status { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); font-weight: 500; }
  .git-ok { color: var(--color-success-text); }
  .git-lead { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-identity { display: grid; grid-template-columns: auto minmax(0, 1fr); align-items: start; gap: calc(var(--spacing) * 3); margin-top: calc(var(--spacing) * 2); }
  .git-identity-text { min-width: 0; }
  .git-name-row { display: flex; align-items: center; gap: calc(var(--spacing) * 1); min-width: 0; }
  .git-name { min-width: 0; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-detail { display: flex; align-items: center; gap: calc(var(--spacing) * 1.5); min-width: 0; margin: calc(var(--spacing) * .5) 0 0; }
  .git-detail-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-form { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-field-row { display: flex; gap: calc(var(--spacing) * 2); align-items: center; }
  .git-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-transfer { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); }
  .git-input { flex: 1 1 auto; min-width: 0; }
  .git-browse { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); flex: none; }
  .git-error { display: flex; align-items: flex-start; gap: calc(var(--spacing) * 1.5); margin: calc(var(--spacing) * 2) 0 0; font-size: var(--panel-meta-size); overflow-wrap: anywhere; }
  .git-summary-block { margin-top: calc(var(--spacing) * 3); }
  .git-summary { display: flex; align-items: flex-start; gap: calc(var(--spacing) * 1.5); margin: 0; overflow-wrap: anywhere; }
  .git-mark { flex: none; }
  .git-details { margin-top: calc(var(--spacing) * 1); }
  .git-details summary { cursor: pointer; color: var(--color-link); }
  .git-details p { margin: calc(var(--spacing) * 1) 0 0; overflow-wrap: anywhere; }
  .git-box { margin-top: calc(var(--spacing) * 3); padding: calc(var(--spacing) * 2); border-radius: var(--radius-base); background: var(--color-row-hover); overflow-wrap: anywhere; }
  .git-box p { margin: 0 0 calc(var(--spacing) * 1.5); }
  .git-box p:last-child { margin-bottom: 0; }
  .git-list { margin: 0 0 calc(var(--spacing) * 1.5); padding-inline-start: calc(var(--spacing) * 4); font-size: var(--panel-meta-size); }
  .git-conflicts { margin: calc(var(--spacing) * 2) 0 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .git-conflict { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * .5); padding-block: calc(var(--spacing) * 1.5); border-bottom: 1px solid var(--color-divider); }
  .git-path { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-choices { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 1); margin-inline-start: calc(var(--spacing) * -1.5); }
  .git-action { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; cursor: pointer; }
  .git-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .git-action:disabled { opacity: .5; cursor: default; }
  .git-section { padding-top: calc(var(--spacing) * 3); border-top: 1px solid var(--color-divider); }
  .git-foot { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 2); }
  .git-activity-subject { margin: calc(var(--spacing) * 1.5) 0 0; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-activity-meta { margin: calc(var(--spacing) * 1) 0 0; overflow-wrap: anywhere; }
  .git-web a { color: var(--color-link); }
  /* Phones: the connect field keeps 16px so iOS does not zoom, and the text
     actions and buttons are finger-sized. */
  @media (max-width: 767px) {
    .git-input { font-size: 1rem; }
    .git-action { min-height: 2.75rem; }
    .git-web .btn { min-height: 2.75rem; }
  }
</style>
