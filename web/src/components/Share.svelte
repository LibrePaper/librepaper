<script>
  // Who a document is shared with, and -- for its owner -- the changes to
  // that. Three parts, top to bottom: who may read it at all, the people named
  // on it, and the links that carry a role.
  //
  // Somebody named on the document sees the same dialog with nothing to click:
  // an editor cannot share, because the owner is the one whose quota and whose
  // name are on the document. A reader who arrived by link never opens it.
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import Modal from "./Modal.svelte";
  import Row from "./layout/Row.svelte";
  import Stack from "./layout/Stack.svelte";
  import { getPrivate, post } from "../lib/api.js";
  import { linkFor } from "../lib/storage.js";
  import { problem, done } from "../lib/toast.svelte.js";

  let { open = $bindable(false), slug, onvisibility } = $props();

  let sharing = $state(null);
  let busy = $state(false);
  // A key is shown once, in the row it belongs to, and the row says so.
  let fresh = $state({ id: "", key: "" });
  let who = $state("");
  let asRole = $state("commenter");
  let linkRole = $state("commenter");
  let linkLabel = $state("");

  const owner = $derived(Boolean(sharing?.can_share));
  const VISIBILITIES = [
    { id: "link", says: "Anyone with the link" },
    { id: "private", says: "Only people named below" },
    { id: "listed", says: "Anyone, and listed on the front page" },
  ];
  // The last is absent under --no-listing: an operator who wants no public
  // listing is not offered a switch that would do nothing.
  const choices = $derived(VISIBILITIES.filter((each) => each.id !== "listed" || sharing?.listing));

  async function load() {
    try {
      sharing = await getPrivate(`/api/documents/${slug}/share`);
    } catch (error) {
      problem(error.message || "could not read who this is shared with");
    }
  }

  // Every change is the same round trip, and what comes back is the whole
  // picture rather than a patch of it, so the dialog can never drift from what
  // the server actually holds.
  async function change(body) {
    busy = true;
    try {
      const answer = await post(`/api/documents/${slug}/share`, body);
      if (answer.key) fresh = { id: answer.key_id, key: answer.key };
      const before = sharing?.visibility;
      sharing = answer;
      if (answer.visibility !== before) onvisibility?.(answer.visibility);
    } catch (error) {
      problem(error.message || "that change was refused");
    } finally {
      busy = false;
    }
  }

  function addPerson(event) {
    event.preventDefault();
    const login = who.trim();
    if (!login) return;
    who = "";
    change({ grant: { login, role: asRole } });
  }

  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      done("copied");
    } catch {
      problem("this browser would not let the page copy; select the link instead");
    }
  }

  // The link a new key belongs to, which is the document's URL with the key in
  // the fragment -- the one part of a URL a browser never sends anywhere.
  const freshLink = $derived(
    fresh.key ? `${new URL(`/docs/${slug}`, location.origin).href}#k=${encodeURIComponent(fresh.key)}` : "",
  );

  const stamp = (value) => (value || "").slice(0, 10);

  $effect(() => {
    if (open && !sharing) load();
  });
</script>

<Modal bind:open title="Share">
  {#snippet children()}
    {#if !sharing}
      <p class="text-surface-600-400 text-sm">Reading who this is shared with…</p>
    {:else}
      <Stack gap={6}>
        <!-- Copying the link is the first thing in the dialog, because it is
             the thing that is wanted most of the time. The key goes back into
             it, so a link copied here is the link that was shared. -->
        <Row gap={2}>
          <input class="input" readonly value={linkFor(slug)} aria-label="Link to this document" />
          <IconButton icon="link" label="Copy the link" onclick={() => copy(linkFor(slug))} />
        </Row>

        <section>
          <h3 class="h6 mb-2">Who may read it</h3>
          <Stack gap={2}>
            {#each choices as choice}
              <label class="flex items-center gap-2">
                <input
                  type="radio"
                  class="radio"
                  name="visibility"
                  value={choice.id}
                  checked={sharing.visibility === choice.id}
                  disabled={!owner || busy}
                  onchange={() => change({ visibility: choice.id })}
                />
                <span class="text-sm">{choice.says}</span>
              </label>
            {/each}
          </Stack>
        </section>

        <section>
          <h3 class="h6 mb-2">People</h3>
          <Stack gap={2}>
            <!-- The owner is first and cannot be removed here; handing the
                 document on is `komodoc transfer`, which is confirmed the way
                 destroy is rather than being a row in a list. -->
            <Row gap={2} justify="between">
              <!-- A document published without signing in belongs to the
                   browser that published it, which has no name to print.
                   Signing in gives it one, and takes the document with it. -->
              <span class="text-sm">
                {sharing.owner?.login
                  ? `@${sharing.owner.login}`
                  : sharing.owner?.visitor
                    ? "this browser"
                    : "nobody in particular"}
              </span>
              <span class="text-surface-600-400 text-sm">owner</span>
            </Row>
            {#each [["editors", "editor"], ["commenters", "commenter"]] as [field, role]}
              {#each sharing[field] ?? [] as grant}
                <Row gap={2} justify="between">
                  <span class="text-sm">@{grant.login}</span>
                  <Row gap={2}>
                    <span class="text-surface-600-400 text-sm">{role}</span>
                    {#if owner}
                      <IconButton
                        icon="trash"
                        tone="plain"
                        size="btn-icon-sm"
                        label="Stop sharing with @{grant.login}"
                        disabled={busy}
                        onclick={() => change({ revoke: grant.login })}
                      />
                    {/if}
                  </Row>
                </Row>
              {/each}
            {/each}
            {#if !sharing.editors?.length && !sharing.commenters?.length}
              <p class="text-surface-600-400 text-sm">Nobody else is named on this document.</p>
            {/if}

            {#if owner}
              <form class="flex gap-2 pt-2" onsubmit={addPerson}>
                <input class="input" placeholder="GitHub login" bind:value={who} />
                <select class="select w-40" bind:value={asRole} aria-label="Role">
                  <option value="commenter">commenter</option>
                  <option value="editor">editor</option>
                </select>
                <button type="submit" class="btn preset-filled-primary-500" disabled={busy}>Add</button>
              </form>
              <!-- What the deployment's switches allow, said before somebody
                   is refused rather than after. -->
              <small class="text-surface-600-400">
                Editing here is open to {sharing.publishers}; commenting to {sharing.commenters_policy}.
              </small>
            {/if}
          </Stack>
        </section>

        {#if owner}
          <section>
            <h3 class="h6 mb-2">Links</h3>
            <Stack gap={2}>
              {#each sharing.links ?? [] as link}
                <Row gap={2} justify="between">
                  <span class="text-sm">
                    {link.label || "unlabelled"}
                    <span class="text-surface-600-400">
                      · {link.role} ·
                      {link.expired ? "expired" : link.until ? `until ${stamp(link.until)}` : "no expiry"}
                    </span>
                  </span>
                  <Row gap={2}>
                    {#if fresh.id === link.id}
                      <IconButton
                        icon="link"
                        label="Copy this link"
                        onclick={() => copy(freshLink)}
                      />
                    {/if}
                    <IconButton
                      icon="trash"
                      tone="plain"
                      size="btn-icon-sm"
                      label="Revoke this link"
                      disabled={busy}
                      onclick={() => change({ revoke: link.id })}
                    />
                  </Row>
                </Row>
                {#if fresh.id === link.id}
                  <!-- Said plainly, because it is true and there is no second
                       chance: the document keeps only this key's digest. -->
                  <input class="input" readonly value={freshLink} aria-label="The new link" />
                  <small class="text-surface-600-400">
                    Copy it now. This is the only time it can be shown.
                  </small>
                {/if}
              {:else}
                <p class="text-surface-600-400 text-sm">No links yet.</p>
              {/each}

              <Row gap={2} justify="start">
                <select class="select w-40" bind:value={linkRole} aria-label="What the link carries">
                  <option value="commenter">commenter</option>
                  {#if sharing.link_editor}<option value="editor">editor</option>{/if}
                </select>
                <input class="input" placeholder="Label, such as reviewer 2" bind:value={linkLabel} />
                <button
                  type="button"
                  class="btn preset-outlined-surface-300-700"
                  disabled={busy || (linkRole === "commenter" && !sharing.link_commenter)}
                  onclick={() => {
                    const label = linkLabel;
                    linkLabel = "";
                    change({ link: { role: linkRole, label } });
                  }}
                >
                  New link
                </button>
              </Row>
              {#if !sharing.link_commenter && !sharing.link_editor}
                <small class="text-surface-600-400">
                  A link names nobody, so it can carry a role only where this deployment lets
                  people take part without signing in.
                </small>
              {/if}
            </Stack>
          </section>
        {:else}
          <!-- The read-only dialog: who else is in the room, and nothing to
               click. An editor cannot share. -->
          <small class="text-surface-600-400">
            <Icon name="lock" size={14} /> Only the owner can change who this is shared with.
          </small>
        {/if}
      </Stack>
    {/if}
  {/snippet}
  {#snippet footer()}
    <button type="button" class="btn preset-filled-primary-500" onclick={() => (open = false)}>
      Done
    </button>
  {/snippet}
</Modal>
