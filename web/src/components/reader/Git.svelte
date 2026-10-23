<script>
  import { untrack } from "svelte";
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import Modal from "../Modal.svelte";
  import { say } from "../../lib/toast.svelte.js";
  import { createBridge } from "../../lib/gitsync/bridge.js";
  import { syncApi } from "../../lib/gitsync/server.js";
  import { remoteIdentity } from "../../lib/gitsync/identity.js";
  import { currentPlatform, signInHelp } from "../../lib/gitsync/sign-in-help.js";
  import {
    checkRemote, connect, defaultMessage, disconnect, discardConflict, leaveBranch, localChanges, outgoing, pull, push, recover, remoteChanges, status,
  } from "../../lib/gitsync/sync.js";

  let { open = false, slug, key = "", session = null, rules = {}, me = {}, resolver, differ } = $props();

  const git = createBridge();
  const api = $derived(syncApi(slug, key));
  const DOCS = "/docs/git.html";

  // The sidecar's status, the shared record and what is saved on this computer.
  let info = $state(null);
  let link = $state(null);
  let pending = $state([]);
  let loaded = $state(false);
  let busy = $state(false);
  let error = $state(null);
  let generation = 0;

  // The pick made before anything is recorded.
  let remoteText = $state("");
  let grant = $state(null);
  let branches = $state([]);
  let branch = $state("");
  let typedBranch = $state(false);
  let discovered = "";
  let reconnect = $state(false);
  let folder = $state("");
  let refusals = $state([]);

  // The connected view: the remote check, the local list and the commit message.
  let remoteState = $state(null);
  let checking = $state(false);
  let checkedAt = $state(null);
  let checkGeneration = 0;
  // The files the remote has that this computer does not, while it is behind.
  let incoming = $state(null);
  let incomingGeneration = 0;
  let local = $state(null);
  let localGeneration = 0;
  let pulling = $state(false);
  let committing = $state(false);
  let message = $state("");
  let note = $state(null);
  let confirmingDisconnect = $state(false);
  let confirmingSwitch = $state(false);
  let identityLine = $state(null);
  const detailsPlace = { placement: "bottom-start", getAnchorRect: () => identityLine?.getBoundingClientRect() ?? null };

  let outcome = $state(null);
  let conflictSession = $state(null);

  const name = $derived(me?.name || me?.handle || "Someone");
  const plural = (count) => `${count} ${count === 1 ? "file" : "files"}`;
  const shortTime = (iso) => {
    const at = new Date(iso);
    return at.toDateString() === new Date().toDateString()
      ? at.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
      : at.toLocaleDateString([], { month: "short", day: "numeric" });
  };
  const fullTime = (iso, by) => `${new Date(iso).toLocaleString()}${by ? ` by ${by}` : ""}`;
  const cleanFolder = (value) => value.trim().replace(/^\/+|\/+$/g, "");
  const label = (value) => value?.name || value?.handle || value || "";

  const connectedHere = $derived(Boolean(link && info?.connected && info.connection_id === link.connection_id));
  const connectedView = $derived(Boolean(link && connectedHere && !reconnect));
  const identity = $derived(link ? remoteIdentity(link.remote_label, link.remote_kind) : null);
  const branchLine = $derived(link ? `${link.branch}${link.folder ? ` · ${link.folder}` : ""}` : "");
  const remote = $derived(grant ? { grant: grant.grant } : { url: remoteText.trim() });
  const haveRemote = $derived(Boolean(grant || remoteText.trim()));
  // The remote a sign-in help names: the connected one, else the one being set up.
  const remoteForHelp = $derived(connectedHere && link ? link.remote_label : remoteText.trim());
  const platform = currentPlatform();
  const resolving = $derived(Boolean(resolver?.state.active));
  const interrupted = $derived(pending.length > 0);
  const savedConflict = $derived(Boolean(conflictSession) && !interrupted);

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
      error = { auth: true, remote: remoteForHelp };
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

  function adopt(state) {
    info = state.git;
    link = state.link;
    pending = state.pending || [];
    // A resolver that is open keeps the session it is working on.
    if (!resolver?.state.active) conflictSession = state.conflict;
  }

  async function load() {
    const request = ++generation;
    try {
      const state = await status(context());
      if (request !== generation) return;
      adopt(state);
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

  // Asks the remote whether the branch has moved. It never pulls.
  async function checkNow() {
    const request = ++checkGeneration;
    checking = true;
    try {
      const answer = settled(await checkRemote(context()));
      if (request !== checkGeneration) return;
      remoteState = answer;
      checkedAt = answer.status === "unknown" ? null : new Date();
      if (answer.reconnect) reconnect = true;
      loadIncoming();
    } catch (failure) {
      if (request !== checkGeneration) return;
      remoteState = { status: "unknown", reason: failure?.message || "Could not check the remote." };
      checkedAt = null;
      loadIncoming();
      if (failure?.kind === "auth") fail(failure);
    } finally {
      if (request === checkGeneration) checking = false;
    }
  }

  // Lists what the remote has while it is behind. Any other state clears the list.
  async function loadIncoming() {
    const request = ++incomingGeneration;
    if (remoteState?.status !== "behind") {
      incoming = null;
      return;
    }
    try {
      const answer = await remoteChanges(context());
      if (request === incomingGeneration) incoming = answer;
    } catch (failure) {
      if (request === incomingGeneration) incoming = { status: "unknown", reason: failure?.message || "Could not list the remote changes." };
    }
  }

  // A refusal (a failed fetch, a sign-in Git could not make) reads as an
  // unknown state with its reason; a sign-in failure also shows the help.
  function settled(answer) {
    if (answer?.status !== "refused") return answer;
    if (answer.kind === "auth") fail({ kind: "auth" });
    return { status: "unknown", reason: answer.refusals?.[0]?.reason || "" };
  }

  // Lists the files that differ from the last sync. It never reaches the remote.
  async function refreshLocal() {
    const request = ++localGeneration;
    try {
      const answer = settled(await localChanges(context()));
      if (request === localGeneration) local = answer;
    } catch (failure) {
      if (request === localGeneration) local = { status: "unknown", reason: failure?.message || "Could not read the local changes." };
    }
  }

  $effect(() => {
    if (open && connectedView) {
      slug;
      untrack(() => {
        checkNow();
        refreshLocal();
      });
    }
  });

  // Edits to the project update the local list, once they stop for a moment.
  $effect(() => {
    if (!open || !connectedHere || !session) return;
    let timer = null;
    const unsubscribe = session.onFiles(() => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        if (!busy) refreshLocal();
      }, 1500);
    });
    return () => {
      clearTimeout(timer);
      unsubscribe?.();
    };
  });

  async function refresh() {
    adopt(await status(context()));
  }

  // Brings the status, the local list and the remote check up to date after an operation.
  async function refreshAll() {
    await refresh();
    refreshLocal();
    checkNow();
  }

  async function loadBranches() {
    refusals = [];
    branches = [];
    branch = "";
    typedBranch = false;
    discovered = "";
    if (!haveRemote) return;
    await guard(async () => {
      const answer = await git.branches(remote);
      branches = answer.branches || [];
      // A repository without commits lists no branch, only the one HEAD names.
      if (!branches.length && answer.default) branches = [answer.default];
      branch = branches.includes(answer.default) ? answer.default : branches[0] || "";
      discovered = answer.label || "";
      // Nothing to pick from: the branch is typed instead.
      if (!branches.length) {
        typedBranch = true;
        branch = "main";
      }
    });
  }

  async function chooseLocal() {
    await guard(async () => {
      grant = await git.chooseLocal();
      remoteText = "";
    });
    if (grant) await loadBranches();
  }

  async function doConnect() {
    if (!haveRemote || !branch.trim()) return;
    await guard(async () => {
      // The label stays unset unless the sidecar gave one; sync.js fills it from the sidecar.
      const known = discovered || grant?.label;
      const draft = { remote, branch: branch.trim(), folder: cleanFolder(folder), ...(known ? { label: known } : {}) };
      const answer = await connect(context(), draft);
      if (answer.status === "refused") {
        if (answer.kind === "auth") error = { auth: true, remote: remoteForHelp };
        if (answer.reconnect) reconnect = true;
        refusals = answer.refusals || [];
        return;
      }
      refusals = [];
      say(`Connected to ${answer.link.remote_label}`, { kind: "done", id: "git:connect" });
      await refresh();
    });
  }

  async function connectThisComputer() {
    await guard(async () => {
      await git.connect({ connection_id: link.connection_id, remote, branch: link.branch });
      reconnect = false;
      await refresh();
    });
  }

  function show(result) {
    outcome = null;
    if (result.link) link = result.link;
    const from = link?.remote_label || "the repository";
    if (result.pulled?.status === "pulled") show(result.pulled);
    if (result.status === "pulled") {
      say(`${name} pulled ${plural(result.files?.length || 0)} from ${from}`, { kind: "done", id: "git:pull" });
    } else if (result.status === "pushed") {
      const id = String(result.commit || "").slice(0, 7);
      say(`Pushed ${id} to ${from}`, { kind: "done", id: "git:push" });
    } else if (result.status === "nothing") {
      say(result.kind === "push" ? "Nothing to push" : "Nothing to pull", { kind: "note", id: "git:nothing" });
    } else if (result.status === "conflict") {
      conflictSession = result.session;
      resolver?.open(context(), conflictSession, resolved);
    } else if (result.status === "refused" && result.kind === "auth") {
      error = { auth: true, remote: remoteForHelp };
    } else if (result.status === "refused" && result.reconnect) {
      reconnect = true;
      outcome = null;
    } else {
      outcome = result;
    }
  }

  async function doPull() {
    note = null;
    await guard(async () => {
      pulling = true;
      try {
        show(await pull(context()));
      } finally {
        pulling = false;
      }
      await refreshAll();
    });
  }

  // A changed file opens its diff in the editor area; the panel stays.
  function openDiff(path) {
    differ?.show(context(), { label: link.remote_label, branch: link.branch }, path);
  }

  // Commits and pushes only the list the user has seen. It never pulls.
  async function commitAndPush() {
    note = null;
    await guard(async () => {
      committing = true;
      try {
        const shown = local?.status === "changes" ? local.files : [];
        const result = await outgoing(context());
        if (result.status === "behind") {
          remoteState = { status: "behind", head: result.head };
          loadIncoming();
          await refresh();
          return;
        }
        if (result.status !== "outgoing") {
          show(result.status === "nothing" ? { ...result, kind: "push" } : result);
          await refreshAll();
          return;
        }
        const key = (file) => `${file.status}:${file.path}`;
        const before = shown.map(key).sort().join("\n");
        const after = result.files.map(key).sort().join("\n");
        if (before !== after) {
          local = result.files.length ? { status: "changes", files: result.files } : { status: "none", files: [] };
          note = "The list changed. Check it and press Commit & push again.";
          await refresh();
          checkNow();
          return;
        }
        const pushed = await push(context(), message.trim(), { confirmed: result.outgoing });
        if (pushed.status === "behind") {
          remoteState = { status: "behind", head: pushed.head };
          loadIncoming();
          await refresh();
          return;
        }
        if (pushed.status === "moved") {
          note = "The repository changed. Press Commit & push again.";
        } else {
          show(pushed.status === "nothing" ? { ...pushed, kind: "push" } : pushed);
          message = "";
        }
        await refreshAll();
      } finally {
        committing = false;
      }
    });
  }

  async function resume() {
    if (conflictSession && !pending.length) {
      resolver?.open(context(), conflictSession, resolved);
      return;
    }
    await guard(async () => {
      const answer = await recover(context());
      if (answer.status === "refused") {
        show(answer);
      } else if (answer.pending.length) {
        error = { text: "The interrupted sync could not be finished yet. Try again later." };
      } else {
        say("Finished the interrupted sync", { kind: "done", id: "git:recover" });
      }
      await refresh();
      if (answer.conflict && !pending.length) {
        conflictSession = answer.conflict;
        resolver?.open(context(), conflictSession, resolved);
      }
    });
  }

  async function resolved(result) {
    show(result);
    await guard(refreshAll);
  }

  async function doDisconnect() {
    await guard(async () => {
      await disconnect(context());
      link = null;
      outcome = null;
      note = null;
      remoteState = null;
      incoming = null;
      incomingGeneration++;
      local = null;
      checkGeneration++;
      localGeneration++;
      checking = false;
      await refresh();
    });
  }

  async function confirmDisconnect() {
    confirmingDisconnect = false;
    await doDisconnect();
  }

  // Ends the connection on this branch and opens the setup form on the same
  // remote and folder, so another branch can be picked and connected.
  async function confirmSwitch() {
    confirmingSwitch = false;
    const spec = info?.remote;
    const keep = { folder: link?.folder || "", label: link?.remote_label || "" };
    let left = false;
    await guard(async () => {
      await leaveBranch(context());
      left = true;
      link = null;
      outcome = null;
      note = null;
      remoteState = null;
      incoming = null;
      incomingGeneration++;
      local = null;
      checkGeneration++;
      localGeneration++;
      checking = false;
    });
    if (!left) return;
    folder = keep.folder;
    if (spec?.grant) {
      grant = { grant: spec.grant, label: keep.label };
      remoteText = "";
    } else {
      grant = null;
      remoteText = spec?.url || "";
    }
    await loadBranches();
    await refresh();
  }

  // Abandons the conflicted pull and returns the panel to its usual view.
  async function cancelConflict() {
    const saved = conflictSession;
    await resolver?.discard();
    await guard(async () => {
      await discardConflict(context(), saved);
      conflictSession = null;
      say("Cancelled the pull", { kind: "note", id: "git:cancel" });
      await refreshAll();
    });
  }

  async function copyRemote() {
    try {
      await navigator.clipboard?.writeText(identity.full);
      say("Copied the remote URL", { kind: "done", id: "git:copy" });
    } catch {
      error = { text: "Could not copy the remote URL." };
    }
  }

  async function copyCommand(command) {
    try {
      await navigator.clipboard.writeText(command);
      say("Copied", { kind: "done", id: "git:copy-command" });
    } catch {
      say("Could not copy the command", { kind: "note", id: "git:copy-command" });
    }
  }

  // Runs the remote check again after a sign-in problem, or lists the branches
  // of a typed remote. The help goes away with the error.
  async function retrySignIn() {
    error = null;
    if (connectedHere) {
      await checkNow();
      await refreshLocal();
    } else if (remoteText.trim()) {
      await loadBranches();
    }
  }

  function menuChosen(item) {
    if (item.value === "open") window.open(identity.href, "_blank", "noopener");
    else if (item.value === "copy") copyRemote();
    else if (item.value === "disconnect") confirmingDisconnect = true;
    else if (item.value === "switch") confirmingSwitch = true;
  }

  // The one line that says where sync stands. It is derived from real state only.
  const sync = $derived.by(() => {
    if (pulling) return { icon: "spinner", text: "Pulling…" };
    if (resolving) return { icon: "triangle-alert", text: "Resolve the conflicts in the editor area.", cancel: { label: "Cancel", onclick: cancelConflict } };
    if (savedConflict) return { icon: "triangle-alert", text: "Conflicts need resolution", action: { label: "Resolve", onclick: resume, tone: "tonal" }, cancel: { label: "Cancel", onclick: cancelConflict } };
    if (interrupted) return { icon: "triangle-alert", text: "A sync was interrupted", action: { label: "Resume", onclick: resume, tone: "tonal" } };
    if (checking || remoteState === null) return { icon: "spinner", text: "Checking the remote…" };
    if (remoteState.status === "behind") return { icon: "arrow-down", text: "New commits on the remote" };
    if (remoteState.status === "current") return { icon: "check", text: "Up to date", checked: checkedAt?.toLocaleTimeString() || "", action: { label: "Pull", onclick: doPull, tone: "quiet" } };
    // The sign-in help below carries its own Try again.
    if (error?.auth) return { icon: "triangle-alert", text: "Git could not sign in." };
    return { icon: "triangle-alert", text: remoteState.reason || "Could not check the remote.", action: { label: "Retry", onclick: checkNow, tone: "quiet" } };
  });

  const reasonText = (item) => {
    if (item.reason === "suggestion") return "A pending suggestion sits in a file the pull would change. Accept or reject the suggestion, then pull again.";
    if (item.reason === "changed") return "Someone kept typing in these files. Try again.";
    return item.reason;
  };
  const gitVersion = $derived(info?.git ? `Git ${info.git}` : "no Git");
  const STATUS_LABELS = { A: "Added", M: "Modified", D: "Deleted" };
</script>

{#snippet alerts()}
  {#if outcome?.status === "refused"}
    <div class="gh-alert" role="alert">
      <div class="gh-alert-body">
        <p>The operation was refused.</p>
        {#each outcome.refusals || [] as item}
          <p>{reasonText(item)}</p>
          {#if item.paths?.length}<ul class="gh-list">{#each item.paths as path}<li>{path}</li>{/each}</ul>{/if}
        {/each}
      </div>
      <button type="button" class="gh-action" onclick={() => (outcome = null)}>Dismiss</button>
    </div>
  {/if}
  {#if error?.auth}
    {@const help = signInHelp(error.remote, platform)}
    <div class="gh-alert" role="alert">
      <div class="gh-alert-body">
        <p class="lp-text-error">{help.title}</p>
        <ol class="gh-list gh-steps">
          {#each help.steps as step}
            <li>
              {#if step.href}
                <a href={step.href} target="_blank" rel="noopener noreferrer">{step.text}</a>
              {:else}
                <span>{step.text}</span>
              {/if}
              {#if step.command}
                <span class="gh-command">
                  <code>{step.command}</code>
                  <IconButton icon="copy" label={`Copy ${step.command}`} tone="plain" size="btn-icon-sm" onclick={() => copyCommand(step.command)} />
                </span>
              {/if}
            </li>
          {/each}
        </ol>
        <div class="gh-row gh-help-actions">
          <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy || checking} onclick={retrySignIn}>Try again</button>
          <button type="button" class="gh-action" onclick={() => (error = null)}>Dismiss</button>
          <a class="gh-help-link" href={`${DOCS}#credentials`} target="_blank" rel="noopener noreferrer">Read more</a>
        </div>
      </div>
    </div>
  {:else if error}
    <div class="gh-alert" role="alert">
      <p class="gh-alert-body lp-text-error">{error.text}</p>
      <button type="button" class="gh-action" onclick={() => (error = null)}>Dismiss</button>
    </div>
  {/if}
{/snippet}

{#snippet changeRow(file)}
  {@const slash = file.path.lastIndexOf("/")}
  <li class="gh-change">
    <button type="button" class="gh-change-open" title={file.path}
      aria-current={differ?.state.active && differ.state.selected === file.path ? "true" : undefined}
      disabled={!differ || resolving} onclick={() => openDiff(file.path)}>
      <Icon name="file-text" size={14} />
      <span class="gh-change-name">{file.path.slice(slash + 1)}</span>
      {#if slash > 0}<span class="gh-change-folder gh-muted">{file.path.slice(0, slash)}</span>{/if}
      <span class="gh-mark" aria-label={STATUS_LABELS[file.status] || file.status}>{file.status}</span>
    </button>
  </li>
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
    {#if !connectedView}
      {#if resolver?.state.active}
        <div class="gh-box" role="status"><p>Resolve the conflicts in the editor area, then apply.</p></div>
      {:else if pending.length || conflictSession}
        <div class="gh-box" role="status">
          <p>{pending.length ? "A sync was interrupted." : "A conflict is saved."}</p>
          <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={resume}>Resume</button>
          {#if !pending.length && conflictSession}
            <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={cancelConflict}>Cancel</button>
          {/if}
        </div>
      {/if}
    {/if}

    {#if connectedView}
      <!-- The details hang below the branch line, not over it. -->
      <div class="gh-identity" bind:this={identityLine}>
        <Menu onSelect={menuChosen} positioning={detailsPlace}>
          <Menu.Trigger>
            {#snippet element(attributes)}
              <button {...attributes} type="button" class="gh-repo" aria-label="Repository details and actions" title={identity.full}>
                <span class="gh-name">{identity.name}</span><Icon name="chevron-down" size="12px" />
              </button>
            {/snippet}
          </Menu.Trigger>
          <ExplorerMenu positioning={detailsPlace}>
            <div class="gh-details">
              <p class="gh-details-label">Repository</p>
              <p class="gh-details-repo" title={identity.name}>{identity.name}</p>
              <dl class="gh-details-facts">
                <dt>Branch</dt><dd title={branchLine}>{link.branch}</dd>
                {#if label(link.connected_by)}<dt>Connected by</dt><dd>{label(link.connected_by)}</dd>{/if}
                {#if link.last_pull_at}<dt>Last pull</dt><dd title={fullTime(link.last_pull_at, label(link.last_pull_by))}>{shortTime(link.last_pull_at)}</dd>{/if}
                {#if link.last_push_at}<dt>Last push</dt><dd title={fullTime(link.last_push_at, label(link.last_push_by))}>{shortTime(link.last_push_at)}</dd>{/if}
              </dl>
            </div>
            <hr class="hr my-1" />
            {#if identity.href}<Menu.Item value="open" class="menuitem">Open in browser</Menu.Item>{/if}
            <Menu.Item value="copy" class="menuitem">Copy remote URL</Menu.Item>
            <Menu.Item value="switch" class="menuitem" disabled={resolving || savedConflict || interrupted}>Switch branch…</Menu.Item>
            <hr class="hr my-1" />
            <Menu.Item value="disconnect" class="menuitem">Disconnect…</Menu.Item>
          </ExplorerMenu>
        </Menu>
        <p class="panel-meta gh-branch" title={branchLine}><Icon name="git-branch" size={14} /><span class="gh-branch-text">{branchLine}</span></p>
      </div>

      <div class="gh-status" role="status" aria-live="polite">
        <span class="gh-status-icon" aria-hidden="true">
          {#if sync.icon === "spinner"}<span class="spinner"></span>{:else}<Icon name={sync.icon} size={14} />{/if}
        </span>
        <span class="gh-status-text">{sync.text}{#if sync.checked}<span class="gh-muted">{` · checked ${sync.checked}`}</span>{/if}</span>
        {#if sync.action}
          <button type="button"
            class={sync.action.tone === "tonal" ? "btn btn-sm lp-control-tonal-brand gh-status-action" : "gh-action gh-status-action"}
            disabled={busy || checking} onclick={sync.action.onclick}>{sync.action.label}</button>
        {/if}
        {#if sync.cancel}
          <button type="button" class="gh-action gh-status-action" disabled={busy || resolver?.state.busy} onclick={sync.cancel.onclick}>{sync.cancel.label}</button>
        {/if}
      </div>
      {@render alerts()}

      {#if remoteState?.status === "behind" && !resolving && !savedConflict && !interrupted}
        <div class="gh-remote">
          <div class="gh-local-head">
            <h4 class="panel-section-title"><Icon name="arrow-down" size={14} /> Remote changes</h4>
            {#if incoming?.status === "changes"}<span class="gh-count gh-muted">{plural(incoming.files.length)}</span>{/if}
            <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy || checking} onclick={doPull}>Pull</button>
          </div>
          {#if incoming?.status === "changes"}
            <ul class="gh-changes" class:gh-changes-long={incoming.files.length > 8}>
              {#each incoming.files as file (file.status + file.path)}
                {@render changeRow(file)}
              {/each}
            </ul>
          {:else if incoming?.status === "unknown"}
            <p class="panel-meta">{incoming.reason}</p>
          {/if}
        </div>
      {/if}

      {#if !resolving && !savedConflict && !interrupted}
        <div class="gh-local">
          <div class="gh-local-head">
            <h4 class="panel-section-title"><Icon name="arrow-up" size={14} /> Local changes</h4>
            {#if local?.status === "changes"}<span class="gh-count gh-muted">{plural(local.files.length)}</span>{/if}
            <IconButton icon="refresh-cw" label="Refresh local changes" tone="plain" size="btn-icon-sm" disabled={busy} onclick={refreshLocal} />
          </div>
          {#if !local}
            <p class="gh-muted">Reading the local files…</p>
          {:else if local.status === "none"}
            <p class="gh-muted">No local changes</p>
          {:else if local.status === "unknown"}
            <p class="gh-muted">{local.reason}</p>
          {:else if local.status === "changes"}
            <ul class="gh-changes" class:gh-changes-long={local.files.length > 8}>
              {#each local.files as file (file.status + file.path)}
                {@render changeRow(file)}
              {/each}
            </ul>
            <label class="gh-field panel-meta">Commit message
              <input class="input gh-input" type="text" bind:value={message} disabled={busy}
                placeholder={defaultMessage(local.files)} />
            </label>
            <div class="gh-row">
              <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || remoteState?.status === "behind"} onclick={commitAndPush}>{committing ? "Committing…" : "Commit & push"}</button>
            </div>
            {#if remoteState?.status === "behind"}
              <p class="gh-muted gh-hint">Pull before pushing</p>
            {/if}
          {/if}
        </div>
      {/if}
      {#if note}<p class="gh-note" role="status">{note}</p>{/if}
    {:else if link}
      <dl class="gh-facts">
        <dt>Remote</dt><dd>{link.remote_label}</dd>
        <dt>Branch</dt><dd>{link.branch}</dd>
      </dl>
      <p class="panel-meta">{reconnect ? "This computer is connected to a different repository for this project. Connect this computer again." : "This project is connected to a repository, but not on this computer."}</p>
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
          oninput={() => { grant = null; refusals = []; }} onchange={loadBranches} />
      </label>
      <div class="gh-row">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={chooseLocal}>Choose a local repository</button>
        {#if grant}<span class="panel-meta">{grant.label}</span>{/if}
      </div>
      {#if branches.length || typedBranch}
        <label class="gh-field panel-meta">Branch
          {#if typedBranch}
            <input class="input gh-input" type="text" bind:value={branch} oninput={() => (refusals = [])} disabled={busy}
              placeholder="main" />
          {:else}
          <select class="select gh-select" bind:value={branch} onchange={() => (refusals = [])} disabled={busy}>
            {#each branches as option (option)}<option value={option}>{option}</option>{/each}
          </select>
          {/if}
        </label>
        <label class="gh-field panel-meta">Folder (optional)
          <input class="input gh-input" type="text" bind:value={folder} oninput={() => (refusals = [])} disabled={busy}
            placeholder="Leave empty for the whole repository" />
        </label>
        <div class="gh-row">
          <button type="button" class="btn btn-sm lp-control-brand" disabled={busy || !branch.trim()} onclick={doConnect}>Connect</button>
        </div>
      {/if}
      {#if refusals.length}
        <div class="gh-box" role="alert">
          {#each refusals as item}
            <p>{item.reason}</p>
            {#if item.paths?.length}<ul class="gh-list">{#each item.paths as path}<li>{path}</li>{/each}</ul>{/if}
          {/each}
        </div>
      {/if}
    {/if}
  {/if}
  {#if !connectedView}
    {@render alerts()}
  {/if}
  </DesktopOnly>
  {#if confirmingDisconnect && identity}
    <Modal open title={`Disconnect this project from ${identity.name}?`} onclose={() => (confirmingDisconnect = false)}
      confirm={{ label: "Disconnect", tone: "error", oncancel: () => (confirmingDisconnect = false), onclick: confirmDisconnect }}>
      <p>Sync stops until you connect again. Nothing is deleted: the project stays in LibrePaper, and the repository and its branches stay as they are.</p>
    </Modal>
  {/if}
  {#if confirmingSwitch && link}
    <Modal open title="Switch branch" onclose={() => (confirmingSwitch = false)}
      confirm={{ label: "Switch", oncancel: () => (confirmingSwitch = false), onclick: confirmSwitch }}>
      <p>This project stops syncing with {link.branch}. Pick another branch of the same repository and connect. Files stay as they are in LibrePaper.</p>
    </Modal>
  {/if}
</section>

<style>
  .gh-panel :global(select) { max-width: 100%; }
  .gh-facts { display: grid; grid-template-columns: auto 1fr; gap: calc(var(--spacing) * .5) calc(var(--spacing) * 2); margin-block: calc(var(--spacing) * 2); }
  .gh-facts dt { color: var(--color-text-muted); font-size: var(--panel-meta-size); }
  .gh-facts dd { margin: 0; overflow-wrap: anywhere; }
  .gh-muted { color: var(--color-text-muted); }
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

  .gh-identity { margin-top: calc(var(--spacing) * 1); }
  .gh-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: .9375rem; font-weight: 500; }
  .gh-repo { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1); max-width: 100%; min-height: 1.75rem; margin-inline-start: calc(var(--spacing) * -1.5); padding: .15rem calc(var(--spacing) * 1.5); border: 0; border-radius: var(--radius-base); background: none; color: inherit; font: inherit; text-align: start; cursor: pointer; }
  .gh-repo:hover, .gh-repo[data-state="open"] { background: var(--color-row-hover); }
  .gh-repo :global(svg) { flex: none; color: var(--color-text-muted); transition: none; }
  .gh-repo[data-state="open"] :global(svg) { transform: rotate(180deg); }
  .gh-details { width: min(18rem, calc(100vw - 2rem)); padding: calc(var(--spacing) * 2) calc(var(--spacing) * 2.5); }
  .gh-details-label { margin: 0; color: var(--color-text-muted); font-size: var(--panel-meta-size); }
  .gh-details-repo { display: block; margin: 0 0 calc(var(--spacing) * 2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
  .gh-details-facts { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: calc(var(--spacing) * .75) calc(var(--spacing) * 3); margin: 0; font-size: var(--panel-meta-size); }
  .gh-details-facts dt { color: var(--color-text-muted); }
  .gh-details-facts dd { margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; text-align: end; }
  .gh-branch { display: flex; align-items: center; gap: calc(var(--spacing) * 1); min-width: 0; margin: calc(var(--spacing) * .5) 0 0; color: var(--color-text-muted); }
  .gh-branch :global(svg) { flex: none; }
  .gh-branch-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .gh-status { display: flex; align-items: start; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 4); }
  .gh-status-icon { display: flex; flex: none; align-items: center; height: 1.5em; }
  .gh-status-text { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
  .gh-status-action { flex: none; }

  .gh-alert { display: flex; align-items: start; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); padding: calc(var(--spacing) * 2); border-radius: var(--radius-base); background: var(--color-row-hover); }
  .gh-alert > .gh-action { flex: none; }
  .gh-alert-body { flex: 1 1 auto; min-width: 0; margin: 0; overflow-wrap: anywhere; }
  .gh-alert-body p { margin: 0 0 calc(var(--spacing) * 1.5); }
  .gh-steps { display: grid; gap: calc(var(--spacing) * 1.5); margin: 0 0 calc(var(--spacing) * 2); padding-inline-start: 1.25rem; list-style: decimal; font-size: inherit; }
  .gh-steps a { color: var(--color-link); }
  .gh-command { display: flex; align-items: start; gap: calc(var(--spacing) * 1); min-width: 0; margin-top: calc(var(--spacing) * .5); }
  .gh-command code { flex: 1 1 auto; min-width: 0; padding: .1rem .3rem; border-radius: .25rem; background: var(--color-sidebar); font-family: monospace; overflow-wrap: anywhere; white-space: normal; }
  .gh-command :global(button) { flex: none; }
  .gh-help-actions { flex-wrap: wrap; margin-top: calc(var(--spacing) * 1); }
  .gh-help-link { color: var(--color-link); }

  .gh-remote, .gh-local { margin-top: calc(var(--spacing) * 5); padding-top: calc(var(--spacing) * 4); border-top: 1px solid var(--color-divider); }
  .gh-local-head { display: flex; align-items: center; gap: calc(var(--spacing) * 2); min-height: 2rem; }
  .gh-local-head .panel-section-title { margin: 0; margin-inline-end: auto; }
  .gh-local-head .panel-section-title { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); }
  .gh-local-head .panel-section-title :global(svg) { flex: none; color: var(--color-text-muted); }
  .gh-count { font-size: var(--panel-meta-size); }
  .gh-changes { margin: calc(var(--spacing) * 1) 0 0; padding: 0; list-style: none; }
  .gh-changes-long { max-height: 16rem; overflow-y: auto; }
  .gh-change { min-width: 0; }
  .gh-change-open { display: flex; align-items: center; gap: calc(var(--spacing) * 1.5); width: calc(100% + var(--spacing) * 2); min-width: 0; min-height: 2rem; padding: 0 calc(var(--spacing) * 1); margin-inline: calc(var(--spacing) * -1); border: 0; border-radius: var(--radius-base); background: none; color: inherit; font: inherit; text-align: start; cursor: pointer; }
  .gh-change-open:hover:not(:disabled), .gh-change-open[aria-current="true"] { background: var(--color-row-hover); }
  .gh-change-open:disabled { cursor: default; }
  .gh-change-open > :global(svg) { flex: none; }
  .gh-change-name { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .gh-change-folder { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--panel-meta-size); }
  .gh-mark { flex: none; margin-inline-start: auto; font-weight: 600; font-variant-numeric: tabular-nums; }
  .gh-hint { margin: calc(var(--spacing) * 2) 0 0; font-size: var(--panel-meta-size); }
  .gh-note { margin: calc(var(--spacing) * 3) 0 0; font-size: var(--panel-meta-size); }
</style>
