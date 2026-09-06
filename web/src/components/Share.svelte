<script>
  import { onDestroy } from "svelte";
  import Modal from "./Modal.svelte";
  import { getPrivate, post } from "../lib/api.js";
  import { linkFor } from "../lib/storage.js";

  let { open = $bindable(false), slug, onvisibility, inline = false } = $props();
  let sharing = $state(null);
  let busy = $state(false);
  let loading = $state(false);
  let error = $state("");
  let feedback = $state("");
  let generation = 0;
  let copied = $state("");
  let copyTimer;
  onDestroy(() => clearTimeout(copyTimer));
  // The expiry chosen for a link that does not exist yet, kept per role so
  // opening the dialog does not make Comment forget what Edit's select said.
  let until = $state({ reader: "180d", commenter: "180d", editor: "180d" });

  const ROLES = [
    { id: "reader", label: "Read" },
    { id: "commenter", label: "Comment" },
    { id: "editor", label: "Edit" },
  ];
  const access = {
    private: { label: "Only people with a link", detail: "Only people who hold a live link may open this document." },
    link: { label: "Anyone with the URL", detail: "Anyone with the document URL can open it." },
    listed: { label: "Anyone, and listed on the front page", detail: "Anyone can find this document on the project list and open it." },
  };
  const accessDetail = $derived(access[sharing?.visibility]?.detail || "");
  const bareUrl = $derived(new URL(`/docs/${slug}`, location.origin).href);
  const dateOf = (iso) => (iso || "").slice(0, 10);
  // The server answers a path, the way it does for a document's own url, so
  // the link handed to somebody is completed against this origin here.
  const fullUrl = (path) => new URL(path, location.origin).href;

  async function load(documentSlug) {
    const request = ++generation;
    loading = true;
    error = "";
    feedback = "";
    try {
      const answer = await getPrivate(`/api/documents/${documentSlug}/share`);
      if (request !== generation) return;
      sharing = answer;
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
      const previous = sharing.visibility;
      sharing = answer;
      if (answer.visibility !== previous) onvisibility?.(answer.visibility);
      feedback = message;
      return true;
    } catch (failure) {
      if (request === generation) error = failure.message || "Could not update access.";
      return false;
    } finally { busy = false; }
  }

  const createLink = (role) => change({ link: { role, until: until[role] } }, "Access link created.");
  const resetLink = (role) => change({ link: { role, until: until[role] } }, "Access link reset. The old link no longer works.");
  const revokeLink = (role) => change({ revoke: role }, "Access link turned off.");
  const revokePerson = (person) => change({ revoke: person.login }, `Access removed for ${person.login}.`);

  async function copy(value, label = "Link copied.") {
    try {
      await navigator.clipboard.writeText(value);
      copied = value;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = ""), 1800);
      feedback = label;
      error = "";
    } catch { error = "Copy was blocked. Select the link field and copy it manually."; }
  }
</script>

{#snippet content()}
  <div class="share-panel space-y-5">
    {#if loading}
      <p class="text-surface-600-400" role="status">Loading sharing settings…</p>
    {:else if sharing}
      <section class="share-access space-y-2" aria-labelledby="access-heading">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h3 id="access-heading" class="font-semibold">General access</h3>
          <select class="select share-field w-auto" aria-label="General access" value={sharing.visibility} disabled={busy}
            onchange={async (event) => {
              const field = event.currentTarget;
              if (!await change({ visibility: field.value }, "General access updated.")) field.value = sharing.visibility;
            }}>
            <option value="private">{access.private.label}</option>
            <option value="link">{access.link.label}</option>
            {#if sharing.listing}<option value="listed">{access.listed.label}</option>{/if}
          </select>
        </div>
        <p class="text-surface-600-400">{accessDetail}</p>
      </section>

      <div class="space-y-4">
        {#each ROLES as role (role.id)}
          {@const link = sharing.links?.[role.id] || null}
          {@const openToAll = role.id === "reader" && sharing.visibility !== "private"}
          <section class="share-access space-y-2" aria-labelledby="share-{role.id}-heading">
            <h3 id="share-{role.id}-heading" class="font-medium">{role.label}</h3>
            {#if openToAll}
              <div class="flex gap-2">
                <input class="input share-field min-w-0 flex-1" readonly value={bareUrl} aria-label="{role.label} link" onclick={(event) => event.currentTarget.select()} />
                <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={busy} onclick={() => copy(bareUrl)}>{copied === bareUrl ? "Copied" : "Copy"}</button>
              </div>
              <p class="text-surface-600-400">Everyone with the URL can read.</p>
            {:else if link}
              {#if link.key}
                <div class="flex gap-2">
                  <input class="input share-field min-w-0 flex-1" readonly value={fullUrl(link.url)} aria-label="{role.label} link" onclick={(event) => event.currentTarget.select()} />
                  <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={busy} onclick={() => copy(fullUrl(link.url))}>{copied === fullUrl(link.url) ? "Copied" : "Copy"}</button>
                </div>
                <p class="text-surface-600-400">{link.expired ? "Expired" : link.until ? `Expires ${dateOf(link.until)}` : "No expiry"}</p>
              {:else}
                <p class="text-surface-600-400">Legacy link, reset to get a new one.</p>
              {/if}
              <div class="flex gap-2">
                <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={busy} onclick={() => resetLink(role.id)}>Reset</button>
                <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={busy} onclick={() => revokeLink(role.id)}>Turn off</button>
              </div>
            {:else}
              <div class="flex flex-wrap items-center gap-2">
                <select class="select share-field w-auto" aria-label="{role.label} link expiry" bind:value={until[role.id]} disabled={busy}>
                  <option value="7d">7 days</option>
                  <option value="30d">30 days</option>
                  <option value="180d">6 months</option>
                  <option value="never">Never</option>
                </select>
                <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={busy} onclick={() => createLink(role.id)}>Create</button>
              </div>
            {/if}
            {#if role.id === "editor" && sharing.edit_needs_signin}<p class="text-surface-600-400">Editors must sign in.</p>{/if}
            {#if role.id === "commenter" && sharing.comment_needs_signin}<p class="text-surface-600-400">Commenters must sign in.</p>{/if}
          </section>
        {/each}
      </div>

      {#if sharing.legacy}
        <section class="share-access space-y-2" aria-labelledby="legacy-heading">
          <h3 id="legacy-heading" class="font-semibold">People (legacy)</h3>
          <div class="share-people" aria-label="Legacy people with access">
            {#each [...(sharing.legacy.editors || []).map((person) => ({ ...person, role: "Can edit" })), ...(sharing.legacy.commenters || []).map((person) => ({ ...person, role: "Can comment" }))] as person}
              <div class="share-person">
                <span class="min-w-0 flex-1 truncate" title={person.login}>{person.name || person.login}</span>
                <span class="text-surface-600-400">{person.role}</span>
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
    {#if feedback}<p class="text-surface-600-400" role="status">{feedback}</p>{/if}
  </div>
{/snippet}

{#snippet footer()}
    <div class="share-panel w-full space-y-2 border-t border-surface-200-800 pt-3">
      <label for="share-document-link" class="font-medium">Document link</label>
      <div class="flex gap-2">
        <input id="share-document-link" class="input share-field min-w-0 flex-1" readonly value={linkFor(slug)} onclick={(event) => event.currentTarget.select()} />
        <button type="button" class="btn btn-sm preset-filled-primary-500" onclick={() => copy(linkFor(slug))}>{copied === linkFor(slug) ? "Copied" : "Copy link"}</button>
      </div>
      <div class="flex items-center justify-between gap-3">
        <p class="text-surface-600-400">{sharing?.visibility === "private" ? "Copying this link does not grant additional access." : "Share this URL to open the document."}</p>
        {#if !inline}<button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => (open = false)}>Done</button>{/if}
      </div>
    </div>
{/snippet}

{#if inline}
  <section class="panel share-sidebar space-y-4 pt-4" aria-label="Share document">
    <h3 class="text-sm font-semibold">Share</h3>
    {@render content()}
    {@render footer()}
  </section>
{:else}
  <Modal bind:open title="Share document" wide {footer}>
    {@render content()}
  </Modal>
{/if}

<style>
  .share-sidebar :global(select) { max-width: 100%; }
</style>
