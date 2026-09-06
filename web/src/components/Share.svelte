<script>
  import { onDestroy } from "svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import Modal from "./Modal.svelte";
  import { getPrivate, post } from "../lib/api.js";

  let { open = $bindable(false), slug, onclose, inline = false } = $props();
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
  // The expiry chosen for a link that does not exist yet, kept per role so
  // opening the dialog does not make Comment forget what Edit's select said.
  let until = $state({ reader: "180d", commenter: "180d", editor: "180d" });
  let labels = $state({ reader: "", commenter: "", editor: "" });
  let budgets = $state({ reader: "", commenter: "", editor: "" });

  const ROLES = [
    { id: "reader", label: "Read" },
    { id: "commenter", label: "Comment" },
    { id: "editor", label: "Edit" },
  ];
  const dateOf = (iso) => new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
  // The server answers a path, the way it does for a document's own url, so
  // the link handed to somebody is completed against this origin here.
  const fullUrl = (path) => new URL(path, location.origin).href;

  function rememberLinkSettings(answer) {
    for (const role of ROLES) {
      const link = answer?.links?.[role.id];
      labels[role.id] = link?.label || "";
      budgets[role.id] = link?.budget ?? "";
    }
  }

  async function load(documentSlug) {
    const request = ++generation;
    loading = true;
    copyFallback = "";
    error = "";
    feedback = "";
    try {
      const answer = await getPrivate(`/api/documents/${documentSlug}/share`);
      if (request !== generation) return;
      sharing = answer;
      rememberLinkSettings(answer);
    } catch (failure) {
      if (request === generation) error = failure.message || "Could not load sharing settings.";
    } finally {
      if (request === generation) loading = false;
    }
  }

  $effect(() => {
    const documentSlug = slug;
    if (open) {
      load(documentSlug);
      return () => { generation += 1; };
    }
  });

  async function change(body, message) {
    if (busy) return false;
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
      rememberLinkSettings(answer);
      feedback = message;
      return true;
    } catch (failure) {
      if (request === generation) error = failure.message || "Could not update access.";
      return false;
    } finally { busy = false; }
  }

  const linkChange = (role) => ({ link: {
    role,
    until: until[role],
    label: labels[role],
    budget: budgets[role] == null || budgets[role] === "" ? null : Number(budgets[role]),
  } });
  const createLink = (role) => change(linkChange(role), "Access link created.");
  const resetLink = (role) => change(linkChange(role), "Access link reset. The old link no longer works.");
  const revokeLink = (role) => change({ revoke: role }, "");
  const revokePerson = (person) => change({ revoke: person.login }, `Access removed for ${person.login}.`);

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

{#snippet content()}
  <div class="share-panel space-y-6">
    {#if loading}
      <p class="panel-muted" role="status">Loading sharing settings…</p>
    {:else if sharing}
      <div class="share-links space-y-6" aria-label="Share links">
        {#each ROLES as role (role.id)}
          {@const link = sharing.links?.[role.id] || null}
          {@const description = role.id === "reader" ? "Anyone with this link can read the document." : role.id === "commenter" ? "Anyone with this link can read and comment." : "Anyone with this link can read, comment and edit."}
          <section class="share-section space-y-2" aria-labelledby="share-{role.id}-heading">
            <h4 id="share-{role.id}-heading" class="panel-section-title">{role.label}</h4>
            <p class="panel-muted">{description}</p>
            {#if link?.key && !link.expired}
              <p class="panel-meta min-w-0 truncate">
                {link.label || "Unlabelled"} · {link.until ? `Expires ${dateOf(link.until)}` : "No expiry"}{link.budget == null ? "" : ` · ${link.budget} comments/hour`}
              </p>
            {:else if link}
              <p class="panel-muted">{link.expired ? "This link has expired." : "This is an older link format."}</p>
            {/if}
            <div class="share-fields">
              <label class="share-setting panel-meta">Label
                <input class="input share-input" aria-label="{role.label} link label" maxlength="80" placeholder={role.id === "editor" ? "CI" : "Reviewer"} bind:value={labels[role.id]} disabled={busy} />
              </label>
              <label class="share-setting panel-meta">Expires in
                <select class="select share-select" aria-label="{role.label} link expiry" bind:value={until[role.id]} disabled={busy}>
                  <option value="7d">7 days</option>
                  <option value="30d">30 days</option>
                  <option value="180d">6 months</option>
                  <option value="never">Never</option>
                </select>
              </label>
              {#if role.id !== "reader"}
                <label class="share-setting panel-meta">Comments/hour
                  <input class="input share-input" type="number" min="0" step="1" aria-label="{role.label} link budget" placeholder="Server default" bind:value={budgets[role.id]} disabled={busy} />
                </label>
              {/if}
            </div>
            <div class="flex flex-wrap justify-end gap-2">
              {#if link?.key && !link.expired}<button type="button" class="btn btn-sm text-primary-500" disabled={busy} aria-label="Copy {role.label} link" onclick={() => copy(fullUrl(link.url), role.label + " link copied.")}>{copied === fullUrl(link.url) ? "Copied" : "Copy link"}</button>{/if}
              <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={busy} aria-label="{link ? 'Replace' : 'Create'} {role.label} link" onclick={() => link ? resetLink(role.id) : createLink(role.id)}>{link ? "Replace link" : "Create link"}</button>
              {#if link}<button type="button" class="btn btn-sm" disabled={busy} aria-label="Revoke {role.label} link" onclick={() => revokeLink(role.id)}>Revoke</button>{/if}
            </div>
            {#if role.id === "editor" && sharing.edit_needs_signin}<p class="panel-muted">Editors must sign in.</p>{/if}
            {#if role.id === "commenter" && sharing.comment_needs_signin}<p class="panel-muted">Commenters must sign in.</p>{/if}
          </section>
        {/each}
      </div>

      {#if sharing.legacy}
        <section class="share-section space-y-2" aria-labelledby="legacy-heading">
          <h3 id="legacy-heading" class="panel-section-title">People (legacy)</h3>
          <div class="share-people" aria-label="Legacy people with access">
            {#each [...(sharing.legacy.editors || []).map((person) => ({ ...person, role: "Can edit" })), ...(sharing.legacy.commenters || []).map((person) => ({ ...person, role: "Can comment" }))] as person}
              <div class="share-person">
                <span class="min-w-0 flex-1 truncate" title={person.login}>{person.name || person.login}</span>
                <span class="panel-muted">{person.role}</span>
                <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={busy}
                  aria-label="Remove access for {person.login}" onclick={() => revokePerson(person)}>Remove</button>
              </div>
            {/each}
          </div>
        </section>
      {/if}
    {/if}
    {#if error}
      <p class="text-error-600-400" role="alert">{error}</p>
      {#if !sharing && !loading}<button class="btn btn-sm preset-outlined-surface-300-700" onclick={() => load(slug)}>Try again</button>{/if}
    {/if}
    {#if copyFallback}
      <textarea class="share-copy-fallback w-full" readonly rows="3" aria-label="Link to copy manually" value={copyFallback} onclick={(event) => event.currentTarget.select()}></textarea>
    {/if}
    {#if feedback}<p class="panel-muted" role="status">{feedback}</p>{/if}
  </div>
{/snippet}

{#snippet footer()}
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => (open = false)}>Done</button>
{/snippet}

{#if inline}
  <section class="panel share-sidebar" aria-label="Share document">
    <PanelHeader title="Share" />
    {@render content()}
  </section>
{:else}
  <Modal bind:open title="Share document" onclose={onclose} {footer}>
    {@render content()}
  </Modal>
{/if}

<style>
  .share-sidebar :global(select) { max-width: 100%; }
  .share-select { border: 0; background-color: var(--color-row-hover); font: inherit; border-radius: var(--radius-base); }
  .share-fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(8rem, 1fr)); gap: calc(var(--spacing) * 2); }
  .share-setting { display: grid; gap: var(--spacing); }
  .share-input, .share-select { width: 100%; min-width: 0; }
  .share-copy-fallback { border: 0; background: transparent; resize: none; color: var(--color-surface-400-600); font-size: var(--panel-meta-size); }
</style>
