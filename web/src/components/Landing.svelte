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
  import { problem } from "../lib/toast.svelte.js";
  import { SHELL_HEADERS, config as loadConfig, get, me as whoami, upload } from "../lib/api.js";
  import { FAVORITES, VIEWED, read, write } from "../lib/storage.js";
  import { day as isoDay } from "../lib/dates.js";

  let me = $state({});
  // Replaced by the server's own on load; this is only what the drop zone
  // says in the instant before that arrives. Keep it in step with
  // `Configuration::default` in `crates/librepaper/src/config.rs`.
  let config = $state({
    max_document: 4 * 1024 * 1024,
    extensions: [".html", ".htm", ".md", ".markdown", ".typ", ".tex"],
  });
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
  let fileError = $state("");
  let chosen = $state(null);
  let title = $state("");
  let busy = $state(false);
  let dragging = $state(false);
  let shared = $state(null);
  let confirming = $state(false);
  let confirmText = $state("");
  let sharing = $state(false);
  let pendingDeletion = [];
  let fileInput = $state(null);
  let titleInput = $state(null);

  const maxLabel = $derived(Math.round(config.max_document / (1024 * 1024)) + " MB");

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
      problem(error?.message || "Could not refresh projects.");
      return false;
    }
    return true;
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
      problem(`Could not delete ${failed.length} project${plural}; failed selections remain selected.`);
    }
    await showList();
  }

  /* ------------------------------------------------------------ file picking */

  function refuse(message) {
    chosen = null;
    title = "";
    fileError = message;
    return false;
  }

  // Checked here so a 30 MB mistake is caught before the upload, not after.
  function valid(file) {
    if (!file) return false;
    const dot = file.name.lastIndexOf(".");
    const extension = dot < 0 ? "" : file.name.slice(dot).toLowerCase();
    if (!config.extensions.includes(extension)) {
      return refuse(`${file.name} is not a document LibrePaper can serve. Only ${config.extensions.join(", ")} work.`);
    }
    if (file.size > config.max_document) {
      return refuse(`${file.name} is ${(file.size / (1024 * 1024)).toFixed(1)} MB; the limit is ${maxLabel}.`);
    }
    return true;
  }

  // The document usually names itself: <title> or the first heading in HTML,
  // the first setext or ATX H1 in Markdown. The filename is the fallback, and
  // the CLI does the same.
  function titleFrom(text, name) {
    if (/\.html?$/i.test(name)) {
      const parsed = new DOMParser().parseFromString(text, "text/html");
      const found = parsed.title.trim() || parsed.querySelector("h1")?.textContent.trim();
      if (found) return found;
    } else {
      const atx = text.match(/^#\s+(.+)$/m);
      if (atx) return atx[1].trim();
      const setext = text.match(/^(.+)\n=+\s*$/m);
      if (setext) return setext[1].trim();
    }
    return name.replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ").trim();
  }

  // The drop zone has done its job, so it gives way to the one remaining
  // decision. The title arrives selected, so typing replaces it and Enter
  // alone accepts it.
  async function choose(file) {
    if (!valid(file)) return;
    fileError = "";
    chosen = file;
    title = file.name;
    await Promise.resolve();
    titleInput?.select();
    const text = await file.text().catch(() => "");
    // The reader may already be typing by the time the file is read.
    if (title === file.name) {
      title = titleFrom(text, file.name);
      titleInput?.select();
    }
  }

  function drop(event) {
    event.preventDefault();
    dragging = false;
    const file = event.dataTransfer?.files?.[0];
    if (file) choose(file);
  }

  async function submit(event) {
    event.preventDefault();
    if (!chosen) {
      refuse("Choose a document to upload.");
      return;
    }
    busy = true;
    try {
      const form = new FormData();
      form.append("file", chosen, chosen.name);
      form.append("title", title);
      const response = await upload(form);
      if (!response.ok) {
        problem((await response.json().catch(() => ({}))).error || "upload failed");
        return;
      }
      const doc = await response.json();
      chosen = null;
      title = "";
      await showList();
      shared = new URL(doc.url, location.origin).href;
      sharing = true;
    } catch (error) {
      problem(error?.message || "upload failed");
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    loadConfig().then((found) => (config = found)).catch(() => {});
    whoami().then(async (who) => {
      me = who;
      if (who.can_publish) await showList();
    });
  });
</script>


<svelte:window ondragover={(event) => event.preventDefault()} ondrop={drop} />

<Nav {me} />

<Page width="wide">
  <Stack gap={8}>
    <header>
      <Hero />
      <p class="text-surface-600-400 text-center">
        Publish a document to start a project, share its link, and collect comments on it.
      </p>
    </header>

    <!-- The handle, not the name: this is about the allowlist, which is
         written in handles, and it is shown only to the person it refuses. -->
    {#if me.handle && !me.can_publish}
      <aside class="card preset-tonal-warning p-4">
        {me.handle} may not publish here; this deployment allows {me.publishers}.
      </aside>
    {/if}

    {#if me.can_publish}
      <form onsubmit={submit}>
        {#if !chosen}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="card preset-outlined-surface-300-700 flex flex-col items-center gap-3 border-dashed p-8 text-center transition-colors {dragging
              ? 'preset-tonal-primary'
              : ''}"
            ondragenter={(event) => { event.preventDefault(); dragging = true; }}
            ondragover={(event) => { event.preventDefault(); dragging = true; }}
            ondragleave={() => (dragging = false)}
            ondrop={drop}
          >
            <Icon name="upload" size={28} />
            <p class="text-lg">Drop a document here to start a project</p>
            <button type="button" class="btn preset-filled-primary-500" onclick={() => fileInput.click()}>
              Choose a file
            </button>
            <small class="text-surface-600-400">
              {config.extensions.join(" or ")} files, up to {maxLabel}
            </small>
            <input
              type="file"
              bind:this={fileInput}
              hidden
              accept={config.extensions.join(",") + ",text/html"}
              onchange={(event) => event.currentTarget.files[0] && choose(event.currentTarget.files[0])}
            />
          </div>
        {:else}
          <div class="card preset-outlined-surface-300-700 p-6">
            <Stack gap={3}>
              <p class="font-semibold">{chosen.name}</p>
              <label class="label">
                <span class="label-text">Title</span>
                <!-- Escape backs out of the choice rather than only clearing
                     the field. -->
                <input
                  class="input"
                  bind:this={titleInput}
                  bind:value={title}
                  onkeydown={(event) => {
                    if (event.key === "Escape") { event.preventDefault(); chosen = null; }
                  }}
                />
              </label>
              <Row gap={2} justify="end">
                <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (chosen = null)}>
                  Cancel
                </button>
                <button type="submit" class="btn preset-filled-primary-500" disabled={busy}>
                  {busy ? "Creating…" : "Create project"}
                </button>
              </Row>
            </Stack>
          </div>
        {/if}
        {#if fileError}
          <p class="text-error-500 mt-3 text-sm">{fileError}</p>
        {/if}
      </form>
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
                  <th class={DATE_COLUMNS.has(column) ? "col-when" : ""}>
                    <button
                      type="button"
                      class="cursor-pointer {sortBy === column ? 'text-primary-500 font-semibold' : ''}"
                      onclick={() => sortColumn(column)}
                    >
                      {name}
                      {#if sortBy === column}{ascending ? "▲" : "▼"}{/if}
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
                      ? "No projects yet."
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

<Modal bind:open={confirming} title="Delete?" description={confirmText}>
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (confirming = false)}>
      Cancel
    </button>
    <button type="button" class="btn preset-filled-error-500" onclick={reallyDelete}>Delete</button>
  {/snippet}
</Modal>

<Modal
  bind:open={sharing}
  title="Published"
  description="Share this link; anyone with it can comment, no account needed."
>
  {#snippet children()}
    <input class="input" readonly value={shared ?? ""} />
  {/snippet}
  {#snippet footer()}
    <a role="button" class="btn preset-outlined-surface-300-700" href={shared ?? "/"}>Open</a>
    <button type="button" class="btn preset-filled-primary-500" onclick={() => (sharing = false)}>Done</button>
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
