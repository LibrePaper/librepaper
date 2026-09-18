<script>
  // The landing page: what you may publish, and the projects you have. A
  // project is a document and the directory around it -- its chapters and its
  // figures -- and it is found here by any of them.
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import Modal from "./Modal.svelte";
  import Hero from "./Hero.svelte";
  import Toasts from "./Toasts.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import Row from "./layout/Row.svelte";
  import { say } from "../lib/toast.svelte.js";
  import { SHELL_HEADERS, get, me as whoami, upload } from "../lib/api.js";
  import { FAVORITES, VIEWED, read, write } from "../lib/storage.js";
  import { day as isoDay } from "../lib/dates.js";
  import { FORMATS, formatNamed, starterDocument } from "../lib/starter.js";
  import { preparedProjects } from "../lib/offline-projects.js";

  let me = $state({});
  let documents = $state([]);
  let counts = $state(new Map());
  // Every path in each project, by slug. What a search matches besides the
  // title, and what the Files column counts.
  let paths = $state(new Map());
  let favorites = $state(new Set(read(FAVORITES, [])));
  let viewed = $state(read(VIEWED, {}));
  let selected = $state(new Set());
  let search = $state("");
  let tab = $state("all");
  let sortBy = $state("updated");
  let ascending = $state(false);
  let naming = $state(false);
  let name = $state("");
  let format = $state(FORMATS[0].id);
  let nameError = $state("");
  let busy = $state(false);
  let confirming = $state(false);
  let confirmText = $state("");
  let pendingDeletion = [];
  let nameInput = $state(null);

  // The columns a narrow screen drops; see `.col-when` at the foot of this
  // file for why these two and not the others.
  const DATE_COLUMNS = new Set(["updated", "viewed"]);

  /* ------------------------------------------------------------- the listing */

  // "3 days ago" rather than a date: for something you did yourself, how long
  // ago is the question, not when.
  function sinceWhen(stamp) {
    const days = Math.floor((Date.now() - new Date(stamp)) / 86400000);
    if (days <= 0) return "today";
    if (days === 1) return "yesterday";
    if (days < 30) return `${days} days ago`;
    return isoDay(stamp);
  }

  // One comparison per column. Documents never opened sort as if they were
  // opened at the beginning of time, so they gather at one end rather than
  // scattering through the list.
  function comparing(column, up) {
    const key = {
      title: (doc) => doc.title.toLowerCase(),
      files: (doc) => paths.get(doc.slug)?.length ?? -1,
      comments: (doc) => counts.get(doc.slug) ?? -1,
      updated: (doc) => doc.updated_at,
      viewed: (doc) => viewed[doc.slug] || "",
    }[column];
    return (a, b) => {
      const left = key(a);
      const right = key(b);
      const order = left < right ? -1 : left > right ? 1 : 0;
      return up ? order : -order;
    };
  }

  // A project is found by its title or by any file in it. The file that
  // matched is shown under the title, and opens the project on that file,
  // because "where is the figure I called that" is the question a search for
  // a path is asking.
  function found(doc, needle) {
    if (!needle) return { matches: true, path: "" };
    if (doc.title.toLowerCase().includes(needle)) return { matches: true, path: "" };
    const path = (paths.get(doc.slug) || []).find((each) => each.toLowerCase().includes(needle));
    return { matches: Boolean(path), path: path || "" };
  }

  const needle = $derived(search.trim().toLowerCase());
  const shown = $derived.by(() => {
    return documents
      .filter((doc) => (tab === "all" || favorites.has(doc.slug)) && found(doc, needle).matches)
      .sort(comparing(sortBy, ascending));
  });

  const hereSelected = $derived(shown.filter((doc) => selected.has(doc.slug)).length);

  function sortColumn(column) {
    // Clicking the column already sorted reverses it; a new column starts in
    // the order that column is usually wanted in.
    if (sortBy === column) ascending = !ascending;
    else {
      sortBy = column;
      ascending = column === "title";
    }
  }

  function star(slug) {
    const next = new Set(favorites);
    next.has(slug) ? next.delete(slug) : next.add(slug);
    favorites = next;
    write(FAVORITES, [...next]);
  }

  // A document shared with you by name is in your list, and is not yours to
  // delete. Only what you own can be selected, so the Delete button is never
  // offered for somebody else's document.
  const mine = (doc) => !doc.role || doc.role === "owner";

  function tick(slug, on) {
    const next = new Set(selected);
    on ? next.add(slug) : next.delete(slug);
    selected = next;
  }

  function tickAll(on) {
    const next = new Set(selected);
    for (const doc of shown.filter(mine)) (on ? next.add(doc.slug) : next.delete(doc.slug));
    selected = next;
  }

  async function showList() {
    try {
      // The server caps each page at 200 rows. Keep the old listing visible
      // until every page has arrived, so a failed refresh does not erase it.
      const bySlug = new Map();
      let cursor = null;
      do {
        const query = new URLSearchParams();
        if (cursor?.after_updated && cursor?.after_slug) {
          query.set("after_updated", cursor.after_updated);
          query.set("after_slug", cursor.after_slug);
        }
        const suffix = query.toString();
        const listing = await fetch(`/api/list${suffix ? `?${suffix}` : ""}`, {
          method: "POST",
          headers: SHELL_HEADERS,
        });
        if (!listing.ok) throw new Error(`refresh failed (${listing.status})`);
        const page = await listing.json();
        if (!Array.isArray(page.documents)) throw new Error("refresh returned an invalid listing");
        for (const doc of page.documents) {
          if (doc?.slug) bySlug.set(doc.slug, doc);
        }
        cursor = page.next_cursor;
      } while (cursor?.after_updated && cursor?.after_slug);

      documents = [...bySlug.values()];
      // Counts and directories live in each project's room rather than in the
      // index, so they are fetched separately and the list picks them up when
      // they land.
      const counted = new Map();
      const listed = new Map();
      await Promise.all(
        documents.map((doc) =>
          get(`/api/documents/${doc.slug}`)
            .then((full) => {
              counted.set(doc.slug, full.comment_count);
              listed.set(doc.slug, Array.isArray(full.files) ? full.files : []);
            })
            .catch(() => {}),
        ),
      );
      counts = counted;
      paths = listed;
    } catch (error) {
      if (navigator.onLine !== false) say(error?.message || "The project list could not be refreshed, so what is shown here may be out of date.", { kind: "problem", id: "landing:refresh" });
      return false;
    }
    return true;
  }

  async function showOfflineProjects() {
    try {
      const local = await preparedProjects();
      const bySlug = new Map(documents.map((document_) => [document_.slug, document_]));
      for (const record of local) {
        if (bySlug.has(record.identity.slug)) continue;
        bySlug.set(record.identity.slug, {
          ...record.document,
          updated_at: record.preparedAt,
          offline_prepared: true,
        });
      }
      documents = [...bySlug.values()];
    } catch { /* the online project list remains useful without local storage */ }
  }

  async function deleteSelected() {
    const slugs = [...selected];
    if (!slugs.length) return;
    const plural = slugs.length === 1 ? "" : "s";
    confirmText = `This permanently removes ${slugs.length} project${plural}, every file in ${slugs.length === 1 ? "it" : "them"}, and every comment. It cannot be undone.`;
    pendingDeletion = slugs;
    confirming = true;
  }

  async function reallyDelete() {
    const slugs = pendingDeletion;
    confirming = false;
    pendingDeletion = [];
    const results = await Promise.all(
      slugs.map(async (slug) => {
        try {
          const response = await fetch(`/api/documents/${slug}/delete`, {
            method: "POST",
            headers: SHELL_HEADERS,
          });
          return { slug, ok: response.ok };
        } catch {
          return { slug, ok: false };
        }
      }),
    );
    const failed = results.filter((result) => !result.ok).map((result) => result.slug);
    const succeeded = results.filter((result) => result.ok).map((result) => result.slug);
    const keep = new Set(favorites);
    for (const slug of succeeded) keep.delete(slug);
    favorites = keep;
    write(FAVORITES, [...keep]);
    // Preserve selections made while the requests were in flight. Only the
    // original successful deletions are removed; failed originals stay ready
    // for retry alongside any newly selected projects.
    const stillSelected = new Set(selected);
    for (const slug of succeeded) stillSelected.delete(slug);
    for (const slug of failed) stillSelected.add(slug);
    selected = stillSelected;
    if (failed.length) {
      const plural = failed.length === 1 ? "" : "s";
      say(`${failed.length} project${plural} could not be deleted and ${failed.length === 1 ? "is" : "are"} still here. ${failed.length === 1 ? "It stays" : "They stay"} selected, so Delete will try again.`, { kind: "problem", id: "landing:delete" });
    }
    await showList();
  }

  /* --------------------------------------------------------- a new project */

  // Making a project and filling it are two acts, not one. This asks only for
  // the name -- the one thing nothing else can supply -- and the format, which
  // decides what the main file is called. Everything afterwards happens in the
  // project's own file explorer, which already knows how to take a file, a
  // folder, or an archive, and where to put it.

  function askName() {
    name = "";
    format = FORMATS[0].id;
    nameError = "";
    naming = true;
  }

  async function create(event) {
    event.preventDefault();
    if (busy) return;
    const named = name.trim();
    if (!named) {
      nameError = "Give the project a name.";
      nameInput?.focus();
      return;
    }
    busy = true;
    nameError = "";
    try {
      // The same multipart publish a directory uses, with a directory of one
      // file. The server names the format from that file's extension, so
      // there is one way in rather than a second route for empty projects.
      const starter = starterDocument(named, format);
      const form = new FormData();
      form.append("file", new Blob([starter.text], { type: "text/plain" }), starter.path);
      form.append("title", named);
      const response = await upload(form);
      if (!response.ok) {
        nameError = (await response.json().catch(() => ({}))).error || "The project could not be created.";
        return;
      }
      const doc = await response.json();
      // Straight into the project: the point of making one is to work in it,
      // and the list behind is not where the next thing happens.
      location.href = doc.url;
    } catch (error) {
      nameError = error?.message || "The project could not be created.";
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    void showOfflineProjects();
    whoami().then(async (who) => {
      me = who;
      if (who.can_publish) {
        await showList();
        await showOfflineProjects();
      }
    });
  });
</script>


<Nav {me} />

<Page width="wide">
  <Stack gap={8}>
    <header>
      <Hero />
      <p class="text-surface-600-400 text-center">
        Start a project, write it with whoever you like, and share its link to collect comments.
      </p>
    </header>

    <!-- The handle, not the name: this is about the allowlist, which is
         written in handles, and it is shown only to the person it refuses. -->
    {#if me.handle && !me.can_publish}
      <aside class="card preset-tonal-warning p-4">
        {me.handle} may not publish here; this deployment allows {me.publishers}.
      </aside>
    {/if}

    {#if documents.length || me.can_publish}
      <Stack gap={3}>
        <Row gap={3} wrap justify="between">
          <!-- Which projects, and which of those. The two are a filter over
               one list rather than two lists. -->
          <div class="btn-group preset-outlined-surface-300-700 flex-row p-1">
            <button
              type="button"
              class="btn btn-sm {tab === 'all' ? 'preset-filled-primary-500' : ''}"
              aria-pressed={tab === "all"}
              onclick={() => (tab = "all")}
            >
              All
            </button>
            <button
              type="button"
              class="btn btn-sm {tab === 'favorites' ? 'preset-filled-primary-500' : ''}"
              aria-pressed={tab === "favorites"}
              onclick={() => (tab = "favorites")}
            >
              Favorites
            </button>
          </div>

          <Row gap={2}>
            {#if me.can_publish}
              <button type="button" class="btn preset-filled-primary-500" onclick={askName}>
                <Icon name="file-plus" size={18} />
                New project
              </button>
            {/if}
            {#if selected.size}
              <span class="text-surface-600-400 text-sm">{selected.size} selected</span>
              <button type="button" class="btn btn-sm preset-filled-error-500" onclick={deleteSelected}>
                Delete
              </button>
            {/if}
            <input class="input w-72" type="search" placeholder="Search projects by title or file"
                   aria-label="Search projects by title or file" bind:value={search} />
          </Row>
        </Row>

        <div class="table-wrap">
          <table class="table">
            <thead>
              <tr>
                <th class="w-8">
                  <input
                    type="checkbox"
                    class="checkbox"
                    aria-label="Select every project shown"
                    checked={shown.length > 0 && hereSelected === shown.length}
                    indeterminate={hereSelected > 0 && hereSelected < shown.length}
                    onchange={(event) => tickAll(event.currentTarget.checked)}
                  />
                </th>
                <th class="w-8"></th>
                {#each [["title", "Project"], ["files", "Files"], ["comments", "Comments"], ["updated", "Updated"], ["viewed", "Opened"]] as [column, name]}
                  <th class={DATE_COLUMNS.has(column) ? "col-when" : ""}
                      aria-sort={sortBy === column ? (ascending ? "ascending" : "descending") : "none"}>
                    <button
                      type="button"
                      class="cursor-pointer {sortBy === column ? 'text-primary-500 font-semibold' : ''}"
                      onclick={() => sortColumn(column)}
                    >
                      {name}
                      {#if sortBy === column}<span aria-hidden="true">{ascending ? "▲" : "▼"}</span>{/if}
                    </button>
                  </th>
                {/each}
              </tr>
            </thead>
            <tbody class="[&>tr]:hover:preset-tonal-primary">
              {#each shown as doc (doc.slug)}
                <tr>
                  <td>
                    {#if mine(doc)}
                      <input
                        type="checkbox"
                        class="checkbox"
                        aria-label="Select {doc.title}"
                        checked={selected.has(doc.slug)}
                        onchange={(event) => tick(doc.slug, event.currentTarget.checked)}
                      />
                    {/if}
                  </td>
                  <td>
                    <IconButton
                      icon="star"
                      tone="plain"
                      size="btn-icon-sm"
                      colour={favorites.has(doc.slug) ? "text-tertiary-600" : "text-surface-400-600"}
                      filled={favorites.has(doc.slug)}
                      pressed={favorites.has(doc.slug)}
                      label={favorites.has(doc.slug) ? "Remove from favorites" : "Add to favorites"}
                      onclick={() => star(doc.slug)}
                    />
                  </td>
                  <td>
                    <Row gap={2}>
                      <a class="anchor" href="/docs/{doc.slug}">{doc.title}</a>
                      {#if doc.offline_prepared}<span class="text-surface-600-400 text-xs">Offline</span>{/if}
                      <!-- The file the search found, when it was not the
                           title. Opening it opens the project on that file. -->
                      {#if found(doc, needle).path}
                        <a class="anchor text-surface-600-400 text-xs"
                           href="/docs/{doc.slug}?file={encodeURIComponent(found(doc, needle).path)}">
                          {found(doc, needle).path}
                        </a>
                      {/if}
                      <!-- What you hold on somebody else's document. Your own
                           say nothing: everything unmarked here is yours. -->
                      {#if !mine(doc)}
                        <span class="badge preset-tonal-secondary text-xs" title="Shared with you">
                          {doc.role}
                        </span>
                      {/if}
                    </Row>
                  </td>
                  <td>{paths.has(doc.slug) ? paths.get(doc.slug).length : "—"}</td>
                  <td>{counts.has(doc.slug) ? counts.get(doc.slug) : "—"}</td>
                  <td class="col-when whitespace-nowrap">{isoDay(doc.updated_at)}</td>
                  <td class="col-when whitespace-nowrap {viewed[doc.slug] ? '' : 'text-surface-400-600'}">
                    {viewed[doc.slug] ? sinceWhen(viewed[doc.slug]) : "never"}
                  </td>
                </tr>
              {:else}
                <tr>
                  <td colspan="7" class="text-surface-600-400 h-48 text-center align-middle">
                    {documents.length === 0
                      ? me.can_publish
                        ? "No projects yet. Make one, then drop your files into it."
                        : "No projects yet."
                      : tab === "favorites" && !needle
                        ? "No favorites yet. Star a project to keep it here."
                        : "No projects match that search."}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </Stack>
    {/if}
  </Stack>
</Page>

<!-- A name and a format, because a project is a directory and a main file and
     nothing here can guess either. The files come next, in the project. -->
<Modal
  bind:open={naming}
  title="New project"
  description="Name it, then drop your files into its explorer."
  onclose={() => (nameError = "")}
>
  {#snippet children()}
    <form id="new-project" onsubmit={create}>
      <Stack gap={3}>
        <label class="label">
          <span class="label-text">Name</span>
          <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
          <input class="input" autofocus bind:this={nameInput} bind:value={name} placeholder="A paper you can change" />
        </label>
        <label class="label">
          <span class="label-text">Format</span>
          <select class="select" bind:value={format}>
            {#each FORMATS as choice}
              <option value={choice.id}>{choice.name}</option>
            {/each}
          </select>
          <small class="text-surface-600-400">
            The main file is main.{formatNamed(format).extension}; you can add, rename and replace it later.
          </small>
        </label>
        {#if nameError}<p class="text-error-500 text-sm">{nameError}</p>{/if}
      </Stack>
    </form>
  {/snippet}
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (naming = false)}>Cancel</button>
    <button type="submit" form="new-project" class="btn preset-filled-primary-500" disabled={busy}>
      {busy ? "Creating…" : "Create project"}
    </button>
  {/snippet}
</Modal>

<Modal bind:open={confirming} title="Delete?" description={confirmText}>
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (confirming = false)}>
      Cancel
    </button>
    <button type="button" class="btn preset-filled-error-500" onclick={reallyDelete}>Delete</button>
  {/snippet}
</Modal>

<Toasts />

<style>
  /* The two date columns are what a phone has no room for: the listing is
     read there to find a project, and the day it changed is not how anyone
     finds one. Hiding them is what lets the table fit the screen -- the
     alternative is a sideways scroll with no scrollbar drawn to say so, which
     is a column the reader never learns is there. Sorting by them still
     works; the button is in the header on a wider screen. */
  @media (max-width: 640px) {
    .col-when { display: none; }
  }
</style>
