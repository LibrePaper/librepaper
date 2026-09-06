<script>
  import { onDestroy } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import Modal from "./Modal.svelte";
  import { getPrivate, post } from "../lib/api.js";
  import { linkFor } from "../lib/storage.js";

  let { open = $bindable(false), slug, onvisibility } = $props();
  let sharing = $state(null);
  let busy = $state(false);
  let loading = $state(false);
  let error = $state("");
  let feedback = $state("");
  let tab = $state("people");
  let who = $state("");
  let personRole = $state("commenter");
  let linkRole = $state("commenter");
  let linkLabel = $state("");
  let creatingLink = $state(false);
  let copied = $state("");
  let copyTimer;
  onDestroy(() => clearTimeout(copyTimer));
  // Keys are returned only when minted. Keep this session's new links available
  // to copy without persisting their secrets in browser storage.
  let fresh = $state({});
  let generation = 0;
  const owner = $derived(Boolean(sharing?.can_share));
  const roles = $derived([
    ...(sharing?.link_commenter ? [{ id: "commenter", label: "Can comment" }] : []),
    ...(sharing?.link_editor ? [{ id: "editor", label: "Can edit" }] : []),
  ]);
  const access = {
    private: { label: "Restricted", detail: "Only people you add and people with an access link can open this document." },
    link: { label: "Anyone with the link", detail: "Anyone with the document link can open it." },
    listed: { label: "Public", detail: "Anyone can find this document on the project list and open it." },
  };
  const accessDetail = $derived(access[sharing?.visibility]?.detail || "");
  const people = $derived([
    ...(sharing?.editors || []).map((person) => ({ ...person, role: "Can edit" })),
    ...(sharing?.commenters || []).map((person) => ({ ...person, role: "Can comment" })),
  ]);
  const shown = (person) => person?.provider === "github" ? `@${person.name}` : person?.name || "This browser";
  const initial = (person) => (person?.name || "?").slice(0, 1).toUpperCase();
  const roleName = (role) => role === "editor" ? "Can edit" : "Can comment";
  const keyLink = (key) => `${new URL(`/docs/${slug}`, location.origin).href}#k=${encodeURIComponent(key)}`;

  async function load(documentSlug) {
    const request = ++generation;
    loading = true;
    error = "";
    feedback = "";
    try {
      const answer = await getPrivate(`/api/documents/${documentSlug}/share`);
      if (request !== generation) return;
      sharing = answer;
      linkRole = answer.link_commenter ? "commenter" : "editor";
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
    if (busy || !owner) return false;
    const documentSlug = slug;
    const request = generation;
    busy = true;
    error = "";
    feedback = "";
    try {
      const answer = await post(`/api/documents/${documentSlug}/share`, body);
      if (documentSlug !== slug) return false;
      // Closing the dialog must not discard the only copy of a newly minted key.
      if (answer.key) fresh = { ...fresh, [answer.key_id]: answer.key };
      if (body.revoke) {
        const { [body.revoke]: removed, ...remaining } = fresh;
        fresh = remaining;
      }
      if (request !== generation) return false;
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

  async function addPerson(event) {
    event.preventDefault();
    const login = who.trim();
    if (login && await change({ grant: { login, role: personRole } }, `Access added for ${login}.`)) who = "";
  }
  async function createLink(event) {
    event.preventDefault();
    if (!roles.some((role) => role.id === linkRole)) return;
    if (await change({ link: { role: linkRole, label: linkLabel.trim() } }, "Access link created. Copy it before leaving this page.")) {
      linkLabel = "";
      creatingLink = false;
    }
  }
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

<Modal bind:open title="Share document" wide>
  <div class="share-panel space-y-5">
    {#if loading}
      <p class="text-surface-600-400" role="status">Loading sharing settings…</p>
    {:else if sharing}
      <section class="share-access space-y-2" aria-labelledby="access-heading">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h3 id="access-heading" class="font-semibold">General access</h3>
          {#if owner}
            <select class="select share-field w-auto" aria-label="General access" value={sharing.visibility} disabled={busy}
              onchange={async (event) => {
                const field = event.currentTarget;
                if (!await change({ visibility: field.value }, "General access updated.")) field.value = sharing.visibility;
              }}>
              <option value="private">Restricted</option>
              <option value="link">Anyone with the link</option>
              {#if sharing.listing}<option value="listed">Public</option>{/if}
            </select>
          {:else}<span>{access[sharing.visibility]?.label}</span>{/if}
        </div>
        <p class="text-surface-600-400">{accessDetail}</p>
      </section>

      <Tabs value={tab} onValueChange={({ value }) => { tab = value; error = ""; feedback = ""; }}>
        <Tabs.List class="share-tabs" aria-label="Sharing options">
          <Tabs.Trigger class="share-tab" value="people">People</Tabs.Trigger>
          {#if owner}<Tabs.Trigger class="share-tab" value="links">Access links</Tabs.Trigger>{/if}
        </Tabs.List>
        <Tabs.Content value="people" class="space-y-4 pt-4">
          {#if owner}
            <form class="space-y-2" onsubmit={addPerson}>
              <label class="font-medium" for="share-person">Add a person</label>
              <input id="share-person" class="input share-field" placeholder="GitHub username" autocomplete="off" bind:value={who} disabled={busy} />
              <div class="flex flex-wrap items-center gap-2">
                <select class="select share-field w-auto" aria-label="Person's access" bind:value={personRole} disabled={busy}>
                  <option value="commenter">Can comment</option><option value="editor">Can edit</option>
                </select>
                <button class="btn btn-sm preset-filled-primary-500 ml-auto" type="submit" disabled={busy || !who.trim()}>Add person</button>
              </div>
            </form>
          {/if}
          <div class="share-people" aria-label="People with access">
            <div class="share-person">
              <span class="share-avatar" aria-hidden="true">{initial(sharing.owner)}</span>
              <span class="min-w-0 flex-1 truncate">{shown(sharing.owner)}</span>
              <span class="text-surface-600-400">Owner</span>
            </div>
            {#each people as person}
              <div class="share-person">
                <span class="share-avatar" aria-hidden="true">{initial(person)}</span>
                <span class="min-w-0 flex-1 truncate" title={shown(person)}>{shown(person)}</span>
                <span class="text-surface-600-400">{person.role}</span>
                {#if owner}
                  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={busy}
                    aria-label="Remove access for {shown(person)}" onclick={() => change({ revoke: person.login }, `Access removed for ${shown(person)}.`)}>Remove</button>
                {/if}
              </div>
            {/each}
          </div>
          {#if !owner}<p class="text-surface-600-400">Only the owner can change access.</p>{/if}
        </Tabs.Content>
        {#if owner}
          <Tabs.Content value="links" class="space-y-4 pt-4">
            <p class="text-surface-600-400">Give a group access without adding each person. Anyone who receives an access link gets its permissions.</p>
            {#if creatingLink}
              <form class="share-access space-y-3" onsubmit={createLink}>
                <div class="space-y-1">
                  <label for="share-link-label" class="font-medium">Link name <span class="text-surface-600-400 font-normal">(optional)</span></label>
                  <input id="share-link-label" class="input share-field" placeholder="e.g. Reviewer 2" bind:value={linkLabel} disabled={busy} />
                </div>
                <div class="flex flex-wrap items-center gap-2">
                  <select class="select share-field w-auto" aria-label="Access link permissions" bind:value={linkRole} disabled={busy}>
                    {#each roles as role}<option value={role.id}>{role.label}</option>{/each}
                  </select>
                  <div class="ml-auto flex gap-2">
                    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={busy} onclick={() => (creatingLink = false)}>Cancel</button>
                    <button type="submit" class="btn btn-sm preset-filled-primary-500" disabled={busy}>Create link</button>
                  </div>
                </div>
              </form>
            {:else if roles.length}
              <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={busy} onclick={() => (creatingLink = true)}>Create access link</button>
            {:else}<p class="text-surface-600-400">Access links are not available on this server.</p>{/if}
            {#each sharing.links || [] as link (link.id)}
              <article class="share-access space-y-2">
                <div class="flex items-center justify-between gap-2">
                  <div class="min-w-0">
                    <h3 class="truncate font-medium">{link.label || "Unnamed link"}</h3>
                    <p class="text-surface-600-400">{roleName(link.role)} · {link.expired ? "Expired" : link.until ? `Expires ${link.until.slice(0, 10)}` : "No expiration"}</p>
                  </div>
                  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" aria-label="Revoke {link.label || 'unnamed link'}" disabled={busy}
                    onclick={() => change({ revoke: link.id }, "Access link revoked.")}>Revoke</button>
                </div>
                {#if fresh[link.id] && !link.expired}
                  <div class="flex gap-2">
                    <input class="input share-field min-w-0 flex-1" readonly value={keyLink(fresh[link.id])} aria-label="Access link for {link.label || 'unnamed link'}" onclick={(event) => event.currentTarget.select()} />
                    <button type="button" class="btn btn-sm preset-filled-primary-500" onclick={() => copy(keyLink(fresh[link.id]), "Access link copied.")}>{copied === keyLink(fresh[link.id]) ? "Copied" : "Copy"}</button>
                  </div>
                  <p class="text-surface-600-400">Save this link now. It cannot be retrieved after you leave this page.</p>
                {:else if !link.expired}<p class="text-surface-600-400">The full link is shown only when created. Create a new one if you no longer have it.</p>{/if}
              </article>
            {:else}<p class="text-surface-600-400">No access links yet.</p>{/each}
          </Tabs.Content>
        {/if}
      </Tabs>
    {/if}
    {#if error}
      <p class="text-error-600-400" role="alert">{error}</p>
      {#if !sharing && !loading}<button class="btn btn-sm preset-outlined-surface-300-700" onclick={() => load(slug)}>Try again</button>{/if}
    {/if}
    {#if feedback}<p class="text-surface-600-400" role="status">{feedback}</p>{/if}
  </div>
  {#snippet footer()}
    <div class="share-panel w-full space-y-2 border-t border-surface-200-800 pt-3">
      <label for="share-document-link" class="font-medium">Document link</label>
      <div class="flex gap-2">
        <input id="share-document-link" class="input share-field min-w-0 flex-1" readonly value={linkFor(slug)} onclick={(event) => event.currentTarget.select()} />
        <button type="button" class="btn btn-sm preset-filled-primary-500" onclick={() => copy(linkFor(slug))}>{copied === linkFor(slug) ? "Copied" : "Copy link"}</button>
      </div>
      <div class="flex items-center justify-between gap-3">
        <p class="text-surface-600-400">{sharing?.visibility === "private" ? "Copying this link does not grant additional access." : "Share this URL to open the document."}</p>
        <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => (open = false)}>Done</button>
      </div>
    </div>
  {/snippet}
</Modal>
