<script>
  // A web project's Git panel. The project cannot run git, so this panel runs
  // nothing: it is where the GitHub Action that keeps a repository and the
  // project in step is set up and seen. What the server knows (the last push,
  // the Edit tokens) belongs to the controller (lib/reader/web-git.svelte.js);
  // the workflow and the commands are the ones docs/git.md prints.
  import { untrack } from "svelte";
  import Modal from "../../Modal.svelte";
  import ShareRow from "../share/ShareRow.svelte";
  import { since } from "../../../lib/dates.js";
  import { WORKFLOW_FILE, seedingCommands, workflowYaml } from "../../../lib/git-workflow.js";

  let { web, slug, owner = false, linkKey = "", open = false } = $props();

  const DOCS = "/docs/git.html";
  const view = $derived(web.state);
  const busy = $derived(view.busy !== "");
  const address = $derived(`${location.origin}/docs/${slug}`);

  // The token is shown while the panel is open and never afterwards.
  $effect(() => {
    if (!open) return;
    const project = slug;
    const options = { key: linkKey, owner };
    untrack(() => { void web.refresh(project, options); });
    const timer = setInterval(() => {
      if (document.visibilityState !== "hidden") void web.refresh(project, options);
    }, 30_000);
    return () => {
      clearInterval(timer);
      web.hide();
    };
  });

  const pushText = $derived.by(() => {
    if (!view.loaded) return "Checking for pushes…";
    if (view.pushProblem) return "The last push could not be read.";
    if (!view.push) return "No push yet.";
    const who = view.push.by ? ` by ${view.push.by},` : "";
    return `Last pushed${who} ${since(view.push.at)}${view.push.label ? `: ${view.push.label}` : ""}`;
  });
  const pushTitle = $derived(view.push?.at ? new Date(view.push.at).toLocaleString() : "");

  // What was last copied, so its button can say so for a moment, and the text
  // to select by hand when the browser blocked the clipboard.
  let copied = $state("");
  let copyFallback = $state("");
  let copyTimer;
  $effect(() => () => clearTimeout(copyTimer));
  async function copy(text, which) {
    try {
      await navigator.clipboard.writeText(text);
      copied = which;
      copyFallback = "";
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = ""), 1800);
    } catch {
      copied = "";
      copyFallback = text;
    }
  }

  const workflow = $derived(workflowYaml(address));
  const seeding = $derived(seedingCommands(address));
  const aloneCommand = $derived(`librepaper pull ${address}`);

  let confirming = $state(null);
  async function confirmRevoke() {
    const item = confirming;
    confirming = null;
    if (item) await web.revoke(item.id);
  }
  const select = (event) => event.currentTarget.select();
  function done() {
    web.hide();
    copyFallback = "";
  }
</script>

{#snippet copyButton(text, which, label)}
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => copy(text, which)}>
    {copied === which ? "Copied" : label}
  </button>
{/snippet}

<div class="git-web">
  <section class="git-block" aria-labelledby="git-web-repository">
    <h3 id="git-web-repository" class="panel-section-title">Repository</h3>
    <p class="git-lead">This project syncs with a git repository through its GitHub Action. Nothing runs on this computer.</p>
    <p class="git-push" role="status" title={pushTitle}>{pushText}</p>
  </section>

  <section class="git-block" aria-labelledby="git-web-token">
    <h3 id="git-web-token" class="panel-section-title">Token</h3>
    {#if !owner}
      <p class="git-lead">Only the owner can create a token.</p>
    {:else if view.reveal}
      <div class="git-reveal">
        <p class="git-warning" role="alert">Copy this token now. You won't be able to view it again.</p>
        <label class="git-field panel-meta">Token
          <input class="input git-code-input" type="text" readonly aria-label="Token for the GitHub Action" value={view.reveal.token} onclick={select} />
        </label>
        <div class="git-row">
          {@render copyButton(view.reveal.token, "token", "Copy token")}
        </div>
        <p class="panel-meta">Save it as a repository secret named <code>LIBREPAPER_TOKEN</code>. Use Settings, then Secrets and variables, then Actions.</p>
        <div class="git-row">
          <button type="button" class="btn btn-sm lp-control-brand" onclick={done}>Done</button>
        </div>
      </div>
    {:else}
      <p class="panel-meta git-note">The Action needs a token with the Edit role. A token for one project keeps the Action away from every other project.</p>
      {#if view.tokensProblem}
        <p class="lp-text-error" role="alert">{view.tokensProblem}</p>
      {:else if !view.loaded}
        <p class="panel-muted" role="status">Loading…</p>
      {:else if view.tokens.length}
        <ul class="git-tokens" aria-label="Tokens with the Edit role">
          {#each view.tokens as item (item.id)}
            <ShareRow kind="token" {item} {busy} copied="" oncopy={() => {}} onrevoke={() => (confirming = item)} />
          {/each}
        </ul>
      {:else}
        <p class="panel-meta git-note">No token with the Edit role yet.</p>
      {/if}
      <div class="git-row">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy || !view.loaded} onclick={() => web.create()}>
          {view.busy === "create" ? "Creating…" : "Create token for a repository"}
        </button>
      </div>
    {/if}
    {#if view.error}<p class="lp-text-error git-error" role="alert">{view.error}</p>{/if}
  </section>

  <section class="git-block" aria-labelledby="git-web-workflow">
    <h3 id="git-web-workflow" class="panel-section-title">Workflow</h3>
    <p class="git-lead">Save this as <code>{WORKFLOW_FILE}</code> in the repository.</p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -- a scrolling block is reached by keyboard to be read -->
    <pre class="git-code" tabindex="0" aria-label="The workflow file">{workflow}</pre>
    <div class="git-row">{@render copyButton(workflow, "workflow", "Copy workflow")}</div>
  </section>

  <section class="git-block" aria-labelledby="git-web-seed">
    <h3 id="git-web-seed" class="panel-section-title">Before the first run</h3>
    <p class="git-lead">Run these once, in a clone of the repository on your computer. The Action refuses to run until the LibrePaper state exists on GitHub.</p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -- a scrolling block is reached by keyboard to be read -->
    <pre class="git-code" tabindex="0" aria-label="The commands to run once">{seeding}</pre>
    <div class="git-row">{@render copyButton(seeding, "seeding", "Copy commands")}</div>
    <p class="panel-meta git-note">The merge line is needed only when the repository and the project grew separately and share no history.</p>
  </section>

  <section class="git-block" aria-labelledby="git-web-alone">
    <h3 id="git-web-alone" class="panel-section-title">Alone on this computer</h3>
    <p class="git-lead">To keep only a clone of this project, with no GitHub, run this.</p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -- a scrolling block is reached by keyboard to be read -->
    <pre class="git-code" tabindex="0" aria-label="The command for a clone">{aloneCommand}</pre>
    <div class="git-row">{@render copyButton(aloneCommand, "alone", "Copy command")}</div>
    <p class="panel-meta git-note"><a href={DOCS} target="_blank" rel="noopener noreferrer">Read the Git docs</a></p>
  </section>

  {#if copyFallback}
    <p class="panel-meta git-note" role="alert">Copy was blocked. Select this text and copy it by hand.</p>
    <textarea class="git-fallback" readonly rows="4" aria-label="Text to copy by hand" value={copyFallback} onclick={select}></textarea>
  {/if}
</div>

{#if confirming}
  <Modal open title="Revoke token?"
         confirm={{ label: "Revoke token", tone: "error", oncancel: () => (confirming = null), onclick: confirmRevoke }}
         onclose={() => (confirming = null)}>
    <p>This token will stop working. The GitHub Action that uses it can no longer sync this project.</p>
  </Modal>
{/if}

<style>
  .git-web { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 5); }
  .git-block + .git-block { padding-top: calc(var(--spacing) * 4); border-top: 1px solid var(--color-divider); }
  .git-lead { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-note { margin: calc(var(--spacing) * 1.5) 0 0; overflow-wrap: anywhere; }
  .git-push { margin: calc(var(--spacing) * 2) 0 0; font-weight: 500; overflow-wrap: anywhere; }
  .git-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-tokens { margin: calc(var(--spacing) * 2) 0 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .git-reveal { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .git-reveal p, .git-reveal .git-row { margin: 0; }
  .git-warning { font-size: 13px; font-weight: 500; }
  .git-field { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--spacing); }
  .git-code-input { width: 100%; min-width: 0; font-family: var(--font-mono, monospace); font-size: 12px; }
  .git-code { max-width: 100%; margin: calc(var(--spacing) * 2) 0 0; padding: calc(var(--spacing) * 2); overflow: auto; border-radius: var(--radius-base); background: var(--color-row-hover); font-family: var(--font-mono, monospace); font-size: 12px; line-height: 1.5; white-space: pre; }
  .git-web code { font-family: var(--font-mono, monospace); overflow-wrap: anywhere; }
  .git-error { margin: calc(var(--spacing) * 2) 0 0; overflow-wrap: anywhere; }
  .git-fallback { width: 100%; min-width: 0; border: 0; background: transparent; resize: none; color: var(--color-text-muted); font-size: var(--panel-meta-size); }
  a { color: var(--color-link); }
</style>
