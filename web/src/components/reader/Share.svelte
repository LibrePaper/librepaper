<script>
  import { onDestroy } from "svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import Modal from "../Modal.svelte";
  import Icon from "../Icon.svelte";
  import { getPrivate, post } from "../../lib/api.js";

  let { open = $bindable(false), slug, onclose, inline = false, canShare = false, bundleReady = false, bundleFailed = false, bundleBlocked = "", onrefreshbundle = () => {} } = $props();
  let sharing = $state(null);
  let busy = $state(false);
  let loading = $state(false);
  let error = $state("");
  let feedback = $state("");
  let generation = 0;
  let copied = $state("");
  let copyFallback = $state("");
  let copyTimer;
  onDestroy(() => clearTimeout(copyTimer));
  // A permission is either showing what it is or asking what it should be,
  // never both, and only one of the three is ever asking: the panel is a
  // narrow column, and two open forms in it is the clutter this replaced.
  // `{ role, mode }`, where mode is "create" or "settings".
  let editing = $state(null);
  let draft = $state({ until: "", budget: "" });
  // `{ role, kind }` while a rotation or a revocation waits to be confirmed.
  // Both invalidate a URL somebody may already be holding, which is not a
  // thing to do on the way past a menu item.
  let confirming = $state(null);
  let bundleError = $state("");
  let bundleBusy = $state(false);

  const ROLES = [
    { id: "reader", label: "Read", says: "Anyone with this link can view." },
    { id: "commenter", label: "Comment", says: "Anyone with this link can view and comment." },
    { id: "editor", label: "Edit", says: "Signed-in users with this link can edit." },
  ];
  const EXPIRIES = [["7d", "7 days"], ["30d", "30 days"], ["180d", "6 months"], ["never", "Never"]];
  const dateOf = (iso) => new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
  // The server answers a path, the way it does for a document's own url, so
  // the link handed to somebody is completed against this origin here.
  const fullUrl = (path) => new URL(path, location.origin).href;

  const linkOf = (role) => sharing?.links?.[role] || null;
  // A link the server sent a row for exists; whether it still works is what
  // `expired` says. Not `key`: the catalogue stores a link's digest and
  // cannot recover its key, so every response but the one that minted the
  // link leaves `key` and `url` empty. Reading existence off the key made a
  // settings save -- which mints nothing -- look like a deletion.
  const live = (link) => Boolean(link) && !link.expired;
  // Whether this response is still carrying the URL itself, which is the only
  // moment there is anything to copy.
  const copyable = (link) => Boolean(link?.url);
  const needsSignin = (role) => (role === "editor" && sharing?.edit_needs_signin) || (role === "commenter" && sharing?.comment_needs_signin);
  // What a link's row says about itself once it exists: when it stops working,
  // and its budget only where one was chosen.
  const metaOf = (link) => [link.until ? `Expires ${dateOf(link.until)}` : "No expiry", link.budget == null ? "" : `${link.budget} comments/hour`].filter(Boolean).join(" · ");

  // One line under the heading for the rendering readers are served. There is
  // no publishing to do and nothing to approve: the shared view follows the
  // draft on its own, so the only states worth a line are the ones somebody
  // might act on -- it cannot be checked, or it is refused. A shared view that
  // is fine says nothing: it needs nothing from anybody.
  const renderStatus = $derived(
    !bundleReady ? (bundleFailed ? "Could not check the shared view." : "Checking the shared view…")
    : bundleBlocked ? bundleBlocked
    : ""
  );

  async function load(documentSlug) {
    const request = ++generation;
    loading = true;
    copyFallback = "";
    error = "";
    feedback = "";
    try {
      const answer = canShare ? await getPrivate(`/api/documents/${documentSlug}/share`) : { links: {} };
      if (request !== generation) return;
      sharing = answer;
    } catch (failure) {
      if (request === generation) error = failure.message || "Could not load sharing settings.";
    } finally {
      if (request === generation) loading = false;
    }
  }

  async function refreshBundle() {
    if (bundleBusy) return;
    bundleBusy = true;
    bundleError = "";
    try {
      await onrefreshbundle();
    } catch (failure) {
      bundleError = failure.message || "Could not check the shared view.";
    } finally { bundleBusy = false; }
  }

  $effect(() => {
    const documentSlug = slug;
    if (open) {
      load(documentSlug);
      return () => { generation += 1; editing = null; confirming = null; };
    }
  });

  async function change(body, message) {
    if (busy || !canShare) return false;
    const documentSlug = slug;
    const request = generation;
    busy = true;
    error = "";
    feedback = "";
    try {
      const answer = await post(`/api/documents/${documentSlug}/share`, body);
      if (documentSlug !== slug || request !== generation) return false;
      copyFallback = "";
      sharing = answer;
      feedback = message;
      return true;
    } catch (failure) {
      if (request === generation) error = failure.message || "Could not update access.";
      return false;
    } finally { busy = false; }
  }

  function startCreate(role) {
    draft = { until: "180d", budget: "" };
    editing = { role, mode: "create" };
  }

  function startSettings(role) {
    const link = linkOf(role);
    // An existing expiry is a day, and the control offers durations from now,
    // so leaving it alone is its own choice rather than a date that happens to
    // read the same. An empty value is what the route reads as "unchanged".
    draft = { until: link?.until ? "" : "never", budget: link?.budget ?? "" };
    editing = { role, mode: "settings" };
  }

  const budgetOf = (value) => (value == null || value === "" ? null : Number(value));
  const draftFields = (role) => ({ role, until: draft.until, budget: budgetOf(draft.budget) });

  async function commit() {
    if (!editing) return;
    const { role, mode } = editing;
    const done = mode === "create"
      ? await change({ link: draftFields(role) }, "")
      : await change({ settings: draftFields(role) }, "");
    if (done) editing = null;
  }

  // A rotation keeps the link's memo, its expiry and its budget -- the route
  // preserves what the request leaves out -- and changes only the key, which
  // is the whole point of asking for it.
  async function replaceLink(role) {
    const link = linkOf(role);
    await change({ link: { role, budget: link?.budget ?? null } }, "Link replaced. The old link no longer works.");
  }

  const revokeLink = (role) => change({ revoke: role }, "Access revoked.");

  const askFirst = (role, kind) => (confirming = { role, kind });

  async function confirmAction() {
    if (!confirming) return;
    const { role, kind } = confirming;
    confirming = null;
    if (editing?.role === role) editing = null;
    if (kind === "replace") await replaceLink(role);
    else await revokeLink(role);
  }

  async function copy(value, label = "Link copied.") {
    try {
      await navigator.clipboard.writeText(value);
      copied = value;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = ""), 1800);
      feedback = label;
      copyFallback = "";
      error = "";
    } catch { copyFallback = value; error = "Copy was blocked. Select this link and copy it manually."; }
  }
</script>

{#snippet form(role)}
  {@const creating = editing.mode === "create"}
  <div class="share-form">
    <label class="share-field panel-meta">Expires
      <select class="select share-select" aria-label="{role.label} link expiry" bind:value={draft.until} disabled={busy}>
        {#if !creating && linkOf(role.id)?.until}<option value="">Unchanged ({dateOf(linkOf(role.id).until)})</option>{/if}
        {#each EXPIRIES as [value, says] (value)}<option {value}>{says}</option>{/each}
      </select>
    </label>
    <!-- Only a link that can write has anything to spend. -->
    {#if role.id !== "reader"}
      <label class="share-field panel-meta">Comments/hour
        <input class="input share-input" type="number" min="0" step="1" aria-label="{role.label} link budget" placeholder="Server default" bind:value={draft.budget} disabled={busy} />
      </label>
    {/if}
    <div class="share-form-actions">
      <button type="button" class="share-action" disabled={busy} onclick={() => (editing = null)}>Cancel</button>
      <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={busy} onclick={commit}>{creating ? "Create link" : "Save"}</button>
    </div>
  </div>
{/snippet}

{#snippet content()}
  <div class="share-panel">
    {#if renderStatus}<p class="share-status panel-meta" role="status">{renderStatus}</p>{/if}
    {#if bundleFailed}<button type="button" class="share-action" disabled={bundleBusy} onclick={refreshBundle}>{bundleBusy ? "Checking…" : "Try again"}</button>{/if}
    {#if bundleError}<p class="text-error-600-400" role="alert">{bundleError}</p>{/if}
    {#if loading}
      <p class="panel-muted" role="status">Loading sharing settings…</p>
    {:else if sharing && canShare}
      <div class="share-links" aria-label="Share links">
        {#each ROLES as role (role.id)}
          {@const link = linkOf(role.id)}
          <section class="share-section" aria-labelledby="share-{role.id}-heading">
            <h4 id="share-{role.id}-heading" class="panel-section-title">{role.label}</h4>
            <p class="panel-meta">{role.says}{#if needsSignin(role.id)}<br />Sign-in required.{/if}</p>
            {#if editing?.role === role.id}
              {@render form(role)}
            {:else if live(link)}
              <p class="panel-meta">{metaOf(link)}</p>
              <!-- The URL is shown once, when it is made: the server keeps a
                   digest of it and cannot say it again. Rather than a copy
                   button that would copy nothing, the row says so and offers
                   the one thing that does produce a URL. -->
              {#if !copyable(link)}<p class="panel-meta">The link is shown only when it is made. Replace it to get a new URL.</p>{/if}
              <div class="share-row">
                {#if copyable(link)}<button type="button" class="share-icon" disabled={busy} aria-label="Copy {role.label} link" title="Copy link" onclick={() => copy(fullUrl(link.url), role.label + " link copied.")}><Icon name={copied === fullUrl(link.url) ? "check" : "copy"} size={16} /></button>{/if}
                <button type="button" class="share-icon" disabled={busy} aria-label="Edit {role.label} link settings" title="Edit settings" onclick={() => startSettings(role.id)}><Icon name="sliders" size={16} /></button>
                <button type="button" class="share-icon" disabled={busy} aria-label="Replace {role.label} link" title="Replace link" onclick={() => askFirst(role.id, "replace")}><Icon name="refresh-cw" size={16} /></button>
                <button type="button" class="share-icon share-destructive" disabled={busy} aria-label="Revoke {role.label} link" title="Revoke link" onclick={() => askFirst(role.id, "revoke")}><Icon name="trash" size={16} /></button>
              </div>
            {:else}
              <!-- Only the lapse is worth saying. That there is no link is
                   already said by the button offering to make one. -->
              {#if link?.expired}<p class="panel-meta">Link expired</p>{/if}
              <div class="share-row">
                <button type="button" class="share-action" disabled={busy} aria-label="Create {role.label} link" onclick={() => startCreate(role.id)}>Create link</button>
              </div>
            {/if}
          </section>
        {/each}
      </div>
    {/if}
    {#if error}
      <p class="text-error-600-400" role="alert">{error}</p>
      {#if !sharing && !loading}<button class="btn btn-sm preset-outlined-surface-300-700" onclick={() => load(slug)}>Try again</button>{/if}
    {/if}
    {#if copyFallback}
      <textarea class="share-copy-fallback w-full" readonly rows="3" aria-label="Link to copy manually" value={copyFallback} onclick={(event) => event.currentTarget.select()}></textarea>
    {/if}
    {#if feedback}<p class="panel-meta" role="status">{feedback}</p>{/if}
  </div>
{/snippet}

{#if inline}
  <section class="panel share-sidebar" aria-label="Share document">
    <PanelHeader title="Sharing" />
    {@render content()}
  </section>
{:else}
  <Modal bind:open title="Share document" onclose={onclose}
         confirm={{ label: "Done", cancel: false, onclick: () => (open = false) }}>
    {@render content()}
  </Modal>
{/if}

{#if confirming}
  {@const role = ROLES.find((entry) => entry.id === confirming.role)}
  <Modal open title={confirming.kind === "replace" ? `Replace the ${role.label} link?` : `Revoke ${role.label} access?`}
         onclose={() => (confirming = null)}
         confirm={{ label: confirming.kind === "replace" ? "Replace link" : "Revoke link",
                    tone: "error", oncancel: () => (confirming = null), onclick: confirmAction }}>
    <p>{confirming.kind === "replace"
      ? "The link is replaced by a new one. Anyone holding the current link loses access until you give them the new one."
      : "The link stops working. Anyone holding it loses access, and you can create a new link later."}</p>
  </Modal>
{/if}

<style>
  .share-sidebar :global(select) { max-width: 100%; }
  .share-status { margin-bottom: calc(var(--spacing) * 3); }
  /* Whitespace and a hairline between permissions, not three cards: a card
     apiece in a column this narrow reads as three panels. */
  .share-section { padding-block: calc(var(--spacing) * 2.5); }
  .share-section + .share-section { border-top: 1px solid var(--color-divider); }
  .share-section > p { margin-block: calc(var(--spacing) * .5) 0; }
  .share-row { display: flex; align-items: center; gap: calc(var(--spacing) * 1); margin-top: calc(var(--spacing) * .5); }
  /* A link that exists carries no filled button: copying is the ordinary act,
     and the accent is kept for creating access. */
  .share-action { margin-inline-start: calc(var(--spacing) * -1); padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-primary-600-400); font: inherit; cursor: pointer; }
  .share-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .share-action:disabled { opacity: .5; cursor: default; }
  /* Everything a link offers is one flat row of icons under its expiry: four
     verbs, none of them worth a word each in a column this narrow. */
  .share-icon { display: grid; place-items: center; padding: .25rem; border: 0; border-radius: .25rem; background: none; color: var(--panel-muted); cursor: pointer; }
  .share-icon:hover:not(:disabled) { background: var(--color-row-hover); color: var(--color-primary-600-400); }
  .share-icon:disabled { opacity: .5; cursor: default; }
  .share-icon.share-destructive:hover:not(:disabled) { color: var(--color-error-600-400); }
  .share-form { display: grid; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 2); }
  .share-field { display: grid; gap: var(--spacing); }
  .share-input, .share-select { width: 100%; min-width: 0; }
  .share-select { border: 0; background-color: var(--color-row-hover); font: inherit; border-radius: var(--radius-base); }
  .share-form-actions { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 2); }
  .share-copy-fallback { border: 0; background: transparent; resize: none; color: var(--color-surface-400-600); font-size: var(--panel-meta-size); }
</style>
