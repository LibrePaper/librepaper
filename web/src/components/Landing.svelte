<script>
  // The landing page, which for anyone signed in is not a landing page at all
  // but the other half of the workspace.
  //
  // A project is a document and the directory around it -- its chapters and
  // its figures -- and this is where they are all kept. The page is built out
  // of the same parts the reader is built out of, on purpose: the same bar
  // across the top, the same rail of places down the left at the same three
  // rem, the same pressed-state marker on the place you are in. Opening a
  // project replaces what is written in those fixtures and moves none of
  // them, so going into one reads as going further in rather than going
  // somewhere else.
  //
  // A visitor who is not signed in gets the other thing entirely: the mark,
  // the sentence, and a way in. That is the only part of this file that is a
  // landing page.
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import Modal from "./Modal.svelte";
  import Hero from "./Hero.svelte";
  import Toasts from "./Toasts.svelte";
  import Avatar from "./Avatar.svelte";
  import Sidebar from "./layout/Sidebar.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import Row from "./layout/Row.svelte";
  import { say } from "../lib/toast.svelte.js";
  import { SHELL_HEADERS, get, me as whoami, upload } from "../lib/api.js";
  import { day as isoDay, since } from "../lib/dates.js";
  import { PROJECT_TABS, PROJECT_TAB_IDS } from "../lib/panels.js";
  import { FORMATS, formatNamed, starterDocument } from "../lib/starter.js";
  import { preparedProjects } from "../lib/offline-projects.js";

  let me = $state({});
  let documents = $state([]);
  let trashed = $state([]);
  // Every path in each project, by slug. What a search matches besides the
  // title. Filled only while somebody is searching.
  let paths = $state(new Map());
  let selected = $state(new Set());
  let search = $state("");
  // Which place in the rail is showing. Read from the address so a link to
  // "my favourites" is a link, and so going back from a project returns to
  // the place it was opened from rather than to the top of everything.
  let place = $state(placeFromUrl());
  let sortBy = $state("updated");
  let ascending = $state(false);
  let naming = $state(false);
  let name = $state("");
  let format = $state(FORMATS[0].id);
  let nameError = $state("");
  let busy = $state(false);
  let confirming = $state(false);
  let confirmText = $state("");
  // The trash confirms twice over: once to put a project there, once to empty
  // it. The sentence differs, and so does what the button does, so the dialog
  // carries which of the two it is asking.
  let confirmKind = $state("trash");
  let pendingDeletion = [];
  let nameInput = $state(null);
  // The project being renamed, and the name being typed for it.
  let renaming = $state(null);
  let renameTo = $state("");
  let renameError = $state("");
  let renameInput = $state(null);
  // The project a copy is being asked about, and the name being typed for the
  // copy. Naming it up front rather than afterwards: a fork exists to become
  // its own paper, and "Thesis (copy)" is a name nobody meant to keep.
  let copying = $state(null);
  let copyTo = $state("");
  let copyError = $state("");
  let copyInput = $state(null);
  // The project a copy is being made of. A fork reads and rewrites a whole
  // directory, so it is slow enough to need saying that it is happening.
  let forking = $state("");

  // The columns a narrow screen drops; see `.col-when` at the foot of this
  // file for why these and not the others.
  const DATE_COLUMNS = new Set(["updated"]);

  function placeFromUrl() {
    const asked = new URLSearchParams(location.search).get("in") || "";
    return PROJECT_TAB_IDS.includes(asked) ? asked : "projects";
  }

  // Changing place is a navigation, so it goes in the address bar -- but not
  // in the history, which would make Back walk the rail backwards instead of
  // leaving the page. The rail is where you are, not how you got here.
  function goTo(id) {
    place = id;
    selected = new Set();
    const url = new URL(location.href);
    if (id === "projects") url.searchParams.delete("in");
    else url.searchParams.set("in", id);
    history.replaceState(null, "", url);
    if (id === "trash") void showTrash();
  }

  /* ------------------------------------------------------------- the listing */

  // One comparison per column. Documents never opened sort as if they were
  // opened at the beginning of time, so they gather at one end rather than
  // scattering through the list.
  function comparing(column, up) {
    const key = {
      title: (doc) => doc.title.toLowerCase(),
      owner: (doc) => (doc.owner || "").toLowerCase(),
      files: (doc) => doc.files ?? -1,
      comments: (doc) => doc.comments ?? -1,
      updated: (doc) => doc.updated_at,
      opened: (doc) => doc.opened_at || "",
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
  //
  // The paths behind the second half of that are fetched only once somebody
  // is actually searching, and once per project per visit. Opening this page
  // to look at your projects -- which is what opening it usually is -- costs
  // one request; looking for a file costs a request per project, while you
  // are looking.
  function found(doc, needle) {
    if (!needle) return { matches: true, path: "" };
    if (doc.title.toLowerCase().includes(needle)) return { matches: true, path: "" };
    const path = (paths.get(doc.slug) || []).find((each) => each.toLowerCase().includes(needle));
    return { matches: Boolean(path), path: path || "" };
  }

  // Ask each project what is in it, once. A failure is silent and leaves that
  // project findable by its title alone, which is all it was before.
  async function learnPaths() {
    const wanted = documents.filter((doc) => !paths.has(doc.slug) && !doc.offline_prepared);
    if (!wanted.length) return;
    const listed = new Map(paths);
    await Promise.all(
      wanted.map((doc) =>
        get("/api/documents/" + doc.slug)
          .then((full) => listed.set(doc.slug, Array.isArray(full.files) ? full.files : []))
          .catch(() => listed.set(doc.slug, [])),
      ),
    );
    paths = listed;
  }

  // A document shared with you by name is in your list, and is not yours to
  // delete. Only what you own can be selected, so the Delete button is never
  // offered for somebody else's document.
  const mine = (doc) => !doc.role || doc.role === "owner";

  // What each place holds. Every one of them is the same list with a
  // different predicate and a different order, which is what lets the rail be
  // navigation without any of these being a separate request.
  const belongs = {
    projects: () => true,
    recent: (doc) => Boolean(doc.opened_at),
    shared: (doc) => !mine(doc),
    favorites: (doc) => doc.favorite,
    trash: () => true,
  };

  const PLACE_NAMES = Object.fromEntries(PROJECT_TABS.map((tab) => [tab.id, tab.says]));

  function nothingHere() {
    if (needle) return "No projects match that search.";
    return {
      projects: me.can_publish
        ? "No projects yet. Make one, then drop your files into it."
        : "No projects yet.",
      recent: "Nothing opened yet. A project you open shows up here.",
      shared: "Nothing shared with you yet. A project opened from somebody's link lands here.",
      favorites: "No favorites yet. Star a project to keep it here.",
      trash: "The trash is empty.",
    }[place];
  }

  const needle = $derived(search.trim().toLowerCase());
  // Typing is the request. Nothing is fetched until there is something to
  // look for, and each project is asked once however long the search runs.
  $effect(() => {
    if (needle.length >= 2) void learnPaths();
  });
  const source = $derived(place === "trash" ? trashed : documents);
  const shown = $derived.by(() => {
    const list = source.filter((doc) => belongs[place](doc) && found(doc, needle).matches);
    // Recent has one order and it is the whole point of the place; the trash
    // is ordered by when things went. Everywhere else the reader chooses.
    if (place === "recent") return list.sort(comparing("opened", false));
    if (place === "trash") return list;
    return list.sort(comparing(sortBy, ascending));
  });

  const hereSelected = $derived(shown.filter((doc) => selected.has(doc.slug)).length);
  const selectable = $derived(shown.filter(mine).length);
  // A project of your own that is not one of the starters. What the onboarding
  // strip watches: once there are real projects here, it has done its job.
  const ownWork = $derived(documents.filter((doc) => mine(doc) && !doc.example).length);

  function sortColumn(column) {
    // Clicking the column already sorted reverses it; a new column starts in
    // the order that column is usually wanted in.
    if (sortBy === column) ascending = !ascending;
    else {
      sortBy = column;
      ascending = column === "title" || column === "owner";
    }
  }

  // A star is the account's, not the browser's, so it is a request and not a
  // local write. Set first and reconciled after: the star is the feedback for
  // pressing it, and waiting a round trip to draw it makes the listing feel
  // like it did not hear.
  async function star(doc) {
    const wanted = !doc.favorite;
    doc.favorite = wanted;
    try {
      const response = await fetch(`/api/documents/${doc.slug}/favorite`, {
        method: wanted ? "POST" : "DELETE",
        headers: SHELL_HEADERS,
      });
      if (!response.ok) throw new Error("that could not be saved");
    } catch {
      doc.favorite = !wanted;
      say("That star could not be saved.", { kind: "problem", id: "landing:favorite" });
    }
  }

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
      // The counts arrive with the listing now. They used to be a request per
      // project -- forty projects meant forty-one requests to open this page
      // -- because neither number was in the catalogue. Both are now.
    } catch (error) {
      if (navigator.onLine !== false) say(error?.message || "The project list could not be refreshed, so what is shown here may be out of date.", { kind: "problem", id: "landing:refresh" });
      return false;
    }
    return true;
  }

  // The trash is fetched only when it is opened. It is the one place whose
  // contents no other place needs, and the one nobody visits most days.
  async function showTrash() {
    try {
      const response = await fetch("/api/trash", { headers: SHELL_HEADERS });
      if (!response.ok) throw new Error(`the trash could not be read (${response.status})`);
      const page = await response.json();
      trashed = Array.isArray(page.documents) ? page.documents : [];
    } catch (error) {
      say(error?.message || "The trash could not be read.", { kind: "problem", id: "landing:trash" });
    }
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

  /* ----------------------------------------------------------- the trash */

  // Deleting has never destroyed anything immediately: the project is marked
  // and a purge is queued a week out, and until it runs everything is still
  // here. So this says what it does, and the trash says how long is left.
  async function deleteSelected() {
    const slugs = [...selected];
    if (!slugs.length) return;
    const plural = slugs.length === 1 ? "" : "s";
    confirmText = `${slugs.length} project${plural} will move to the trash, where ${slugs.length === 1 ? "it stays" : "they stay"} for seven days. Until then you can put ${slugs.length === 1 ? "it" : "them"} back.`;
    confirmKind = "trash";
    pendingDeletion = slugs;
    confirming = true;
  }

  // The same confirmation for one row as for a selection of them: deleting is
  // deleting, and the sentence that says where the project goes is the point
  // of the step.
  function askDelete(doc) {
    confirmText = `${doc.title} will move to the trash, where it stays for seven days. Until then you can put it back.`;
    confirmKind = "trash";
    pendingDeletion = [doc.slug];
    confirming = true;
  }

  async function reallyDelete() {
    const slugs = pendingDeletion;
    const wasSelected = new Set(slugs.filter((slug) => selected.has(slug)));
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
    // Preserve selections made while the requests were in flight. Only the
    // original successful deletions are removed; failed originals stay ready
    // for retry alongside any newly selected projects.
    const stillSelected = new Set(selected);
    for (const slug of succeeded) stillSelected.delete(slug);
    // Only what the selection toolbar deleted goes back into the selection. A
    // row that failed on its own trash icon must not switch the page into
    // selecting mode to say so; the message says it.
    for (const slug of failed) if (wasSelected.has(slug)) stillSelected.add(slug);
    selected = stillSelected;
    if (failed.length) {
      const plural = failed.length === 1 ? "" : "s";
      say(`${failed.length} project${plural} could not be deleted and ${failed.length === 1 ? "is" : "are"} still here.`, { kind: "problem", id: "landing:delete" });
    }
    if (succeeded.length) {
      const plural = succeeded.length === 1 ? "" : "s";
      say(`${succeeded.length} project${plural} moved to the trash.`, { id: "landing:trashed" });
    }
    await showList();
  }

  async function restore(doc) {
    try {
      const response = await fetch(`/api/documents/${doc.slug}/untrash`, {
        method: "POST",
        headers: SHELL_HEADERS,
      });
      if (!response.ok) {
        const said = (await response.json().catch(() => ({}))).error;
        throw new Error(said || "That project could not be restored.");
      }
      trashed = trashed.filter((each) => each.slug !== doc.slug);
      say(`${doc.title} is back in your projects.`, { id: "landing:restored" });
      await showList();
    } catch (error) {
      say(error?.message || "That project could not be restored.", { kind: "problem", id: "landing:restore" });
    }
  }

  // Everything the trash does to a selection, it does to one row too: the row
  // buttons and the toolbar call the same two functions with a list of one.
  function askPurge(docs) {
    const slugs = docs.map((doc) => doc.slug);
    if (!slugs.length) return;
    const many = slugs.length > 1;
    confirmText = many
      ? `${slugs.length} projects will be deleted for good. This cannot be undone.`
      : `${docs[0].title} will be deleted for good. This cannot be undone.`;
    confirmKind = "purge";
    pendingDeletion = slugs;
    confirming = true;
  }

  async function reallyPurge() {
    const slugs = pendingDeletion;
    confirming = false;
    pendingDeletion = [];
    const gone = await eachOf(slugs, "purge");
    trashed = trashed.filter((each) => !gone.has(each.slug));
    selected = new Set([...selected].filter((slug) => !gone.has(slug)));
    const failed = slugs.length - gone.size;
    if (failed) {
      say(`${failed} project${failed === 1 ? "" : "s"} could not be deleted.`, { kind: "problem", id: "landing:purge" });
    }
    if (gone.size) {
      say(`${gone.size} project${gone.size === 1 ? "" : "s"} deleted for good.`, { id: "landing:purged" });
    }
  }

  async function restoreSelected() {
    const slugs = [...selected];
    if (!slugs.length) return;
    const back = await eachOf(slugs, "untrash");
    trashed = trashed.filter((each) => !back.has(each.slug));
    selected = new Set([...selected].filter((slug) => !back.has(slug)));
    const failed = slugs.length - back.size;
    if (failed) {
      say(`${failed} project${failed === 1 ? "" : "s"} could not be restored.`, { kind: "problem", id: "landing:restore" });
    }
    if (back.size) {
      say(`${back.size} project${back.size === 1 ? "" : "s"} back in your projects.`, { id: "landing:restored" });
      await showList();
    }
  }

  // The slugs the server accepted. One request per project, because that is
  // the API the trash has; the caller decides what a failure reads as.
  async function eachOf(slugs, verb) {
    const done = new Set();
    await Promise.all(
      slugs.map(async (slug) => {
        try {
          const response = await fetch(`/api/documents/${slug}/${verb}`, {
            method: "POST",
            headers: SHELL_HEADERS,
          });
          if (response.ok) done.add(slug);
        } catch { /* counted as a failure by its absence */ }
      }),
    );
    return done;
  }

  /* ------------------------------------------------- renaming and copying */

  function askRename(doc) {
    renaming = doc;
    renameTo = doc.title;
    renameError = "";
  }

  // The name changes and the address does not. Every link anybody has been
  // given points at the slug, and a project that moved because it was renamed
  // would break each of them to no purpose.
  async function reallyRename(event) {
    event.preventDefault();
    const doc = renaming;
    const title = renameTo.trim();
    if (!doc) return;
    if (!title) {
      renameError = "Give the project a name.";
      renameInput?.focus();
      return;
    }
    if (title === doc.title) {
      renaming = null;
      return;
    }
    try {
      const response = await fetch(`/api/documents/${doc.slug}/rename`, {
        method: "POST",
        headers: { ...SHELL_HEADERS, "Content-Type": "application/json" },
        body: JSON.stringify({ title }),
      });
      if (!response.ok) {
        renameError = (await response.json().catch(() => ({}))).error || "That name could not be saved.";
        return;
      }
      doc.title = title;
      renaming = null;
    } catch (error) {
      renameError = error?.message || "That name could not be saved.";
    }
  }

  // A copy is a project of its own from the moment it exists: its own link,
  // its own comments, its own history. Nothing about it points back, because
  // what it was copied from may be somebody else's and may go away.
  //
  // Which is also why it is named here. The copy is about to be a paper of
  // its own, so the dialog opens on a suggestion and selects it: type over it
  // and the copy has a real name, press Enter and it has the numbered one.
  function copyName(title) {
    const taken = new Set(documents.map((doc) => doc.title));
    for (let n = 1; ; n += 1) {
      const suggestion = `${title} (copy ${n})`;
      if (!taken.has(suggestion)) return suggestion;
    }
  }

  function askFork(doc) {
    if (forking) return;
    copying = doc;
    copyTo = copyName(doc.title);
    copyError = "";
  }

  async function fork(event) {
    event.preventDefault();
    const doc = copying;
    const title = copyTo.trim();
    if (!doc || forking) return;
    if (!title) {
      copyError = "Give the copy a name.";
      copyInput?.focus();
      return;
    }
    forking = doc.slug;
    try {
      const response = await fetch(`/api/documents/${doc.slug}/fork`, {
        method: "POST",
        headers: { ...SHELL_HEADERS, "Content-Type": "application/json" },
        body: JSON.stringify({ title }),
      });
      if (!response.ok) {
        copyError = (await response.json().catch(() => ({}))).error || "That project could not be copied.";
        return;
      }
      const made = await response.json();
      copying = null;
      say(`${made.title} is in your projects.`, { id: "landing:forked" });
      await showList();
    } catch (error) {
      copyError = error?.message || "That project could not be copied.";
    } finally {
      forking = "";
    }
  }

  /* --------------------------------------------------------- a new project */

  // Making a project and filling it are two acts, not one. This asks only for
  // the name -- the one thing nothing else can supply -- and the format, which
  // decides what the main file is called. Everything afterwards happens in the
  // project's own file explorer, which already knows how to take a file, a
  // folder, or an archive, and where to put it.

  function askName(chosen) {
    name = "";
    format = chosen || FORMATS[0].id;
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

  // The suggested name arrives selected, so the first keystroke replaces it.
  // autofocus puts the caret in the box; this is what makes the box a
  // suggestion rather than something to delete first.
  $effect(() => {
    if (copying && copyInput) copyInput.select();
  });

  $effect(() => {
    void showOfflineProjects();
    whoami().then(async (who) => {
      me = who;
      if (who.can_publish) {
        await showList();
        await showOfflineProjects();
        if (place === "trash") await showTrash();
      }
    });
  });
</script>

{#if me.name}
  <!-- The workspace half. The trail says where in the account this is, which
       is the same slot a project's title takes when one is open: going in
       replaces the last segment and nothing else in the bar. -->
  <Nav {me}>
    {#snippet children()}
      <span class="nav-place">{PLACE_NAMES[place]}</span>
    {/snippet}
    {#snippet tools()}
      <input class="input project-search" type="search" placeholder="Search projects"
             aria-label="Search projects by title or file" bind:value={search} />
      {#if me.can_publish}
        <button type="button" class="btn btn-sm preset-filled-primary-500" onclick={() => askName()}>
          <Icon name="file-plus" size={16} />
          New project
        </button>
      {/if}
    {/snippet}
  </Nav>

  <main id="main" tabindex="-1" class="workspace">
    <!-- The rail. Nothing beside it: these are places, and the place you
         choose fills the page rather than a column. `shown.comments` false is
         what the reader's own collapsed column is, so the width here and the
         width there are the same width. -->
    <Sidebar tabs={PROJECT_TABS} panel={place} shown={{ comments: false }}
             resizable={false} label="Projects" onselectpanel={goTo}
             badges={trashed.length ? { trash: { counts: [{ tone: "warnings", of: trashed.length }] } } : {}}>
      {#snippet controls()}{/snippet}
    </Sidebar>

    <section class="projects">
      <h1 class="place-heading">{PLACE_NAMES[place]}</h1>

      <!-- Where a project comes from, for an account that has not made one
           yet. It is above the list rather than in it: a template is
           something to start from, and a row in this table is something you
           already have. It leaves once there is real work here. -->
      {#if place === "projects" && me.can_publish && ownWork === 0}
        <div class="starters">
          <span class="starters-say">Start something new</span>
          {#each FORMATS as choice}
            <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                    onclick={() => askName(choice.id)}>{choice.name}</button>
          {/each}
        </div>
      {/if}

      <!-- One toolbar or the other, never both. Selecting is a mode, and the
           dangerous verb in it is not something to leave sitting beside
           "New project" while nothing is selected. -->
      {#if selected.size}
        <div class="toolbar selecting" role="group" aria-label="What to do with the selected projects">
          <span class="selection-count">{selected.size} selected</span>
          {#if place === "trash"}
            <!-- The trash has both verbs: everything selected goes back, or
                 everything selected goes for good. -->
            <button type="button" class="btn btn-sm preset-tonal-primary" onclick={restoreSelected}>
              Put back
            </button>
            <button type="button" class="btn btn-sm preset-tonal-error"
                    onclick={() => askPurge(shown.filter((doc) => selected.has(doc.slug)))}>
              <Icon name="trash" size={16} /> Delete forever
            </button>
          {:else}
            <button type="button" class="btn btn-sm preset-tonal-error" onclick={deleteSelected}>
              <Icon name="trash" size={16} /> Delete
            </button>
          {/if}
          <IconButton icon="x" label="Clear the selection" onclick={() => (selected = new Set())} />
        </div>
      {:else if place !== "trash"}
        <div class="toolbar">
          <span class="row-count">{shown.length}{shown.length === 1 ? " project" : " projects"}</span>
        </div>
      {:else}
        <div class="toolbar">
          <span class="row-count">Projects here are deleted for good seven days after they were put in the trash.</span>
        </div>
      {/if}

      <div class="table-wrap">
        <table class="table projects-table">
          <thead>
            <tr>
              <th class="col-mark">
                {#if selectable > 0}
                  <input
                    type="checkbox"
                    class="tickbox"
                    aria-label="Select every project shown"
                    checked={selectable > 0 && hereSelected === selectable}
                    indeterminate={hereSelected > 0 && hereSelected < selectable}
                    onchange={(event) => tickAll(event.currentTarget.checked)}
                  />
                {/if}
              </th>
              {#each place === "trash" ? [["title", "Project"], ["owner", "Owner"], ["updated", "Deleted"]] : [["title", "Project"], ["owner", "Owner"], ["comments", "Comments"], ["files", "Files"], ["updated", place === "recent" ? "Opened" : "Updated"]] as [column, label]}
                <th class="{DATE_COLUMNS.has(column) ? 'col-when' : ''} {column === 'files' ? 'col-files' : ''}"
                    aria-sort={sortBy === column ? (ascending ? "ascending" : "descending") : "none"}>
                  <button
                    type="button"
                    class="cursor-pointer {sortBy === column ? 'text-primary-500 font-semibold' : ''}"
                    onclick={() => sortColumn(column)}
                  >
                    {label}
                    {#if sortBy === column}<span aria-hidden="true">{ascending ? "▲" : "▼"}</span>{/if}
                  </button>
                </th>
              {/each}
              <th class="col-star"><span class="sr-only">Favorite</span></th>
            </tr>
          </thead>
          <tbody>
            {#each shown as doc (doc.slug)}
              <tr class:picked={selected.has(doc.slug)}>
                <!-- Just the checkbox. One narrow cell, so a row that can be
                     selected is no wider than one that cannot and the title
                     starts at the same x in both. -->
                <td class="col-mark">
                  <span class="mark">
                    {#if mine(doc)}
                      <input
                        type="checkbox"
                        class="tickbox pick"
                        aria-label="Select {doc.title}"
                        checked={selected.has(doc.slug)}
                        onchange={(event) => tick(doc.slug, event.currentTarget.checked)}
                      />
                    {/if}
                  </span>
                </td>
                <td class="col-title">
                  {#if place === "trash"}
                    <span class="title-line">{doc.title}</span>
                  {:else}
                    <a class="title-line" href="/docs/{doc.slug}">{doc.title}</a>
                  {/if}
                  <span class="title-note">
                    {#if doc.offline_prepared}<span>Offline</span>{/if}
                    <!-- The file the search found, when it was not the
                         title. Opening it opens the project on that file. -->
                    {#if found(doc, needle).path}
                      <a href="/docs/{doc.slug}?file={encodeURIComponent(found(doc, needle).path)}">
                        {found(doc, needle).path}
                      </a>
                    {/if}
                    {#if place === "trash" && doc.purge_due}
                      <span>Deleted for good {since(doc.purge_due)}</span>
                    {/if}
                  </span>
                </td>
                <!-- Who it belongs to. Yours says so rather than saying
                     nothing: a column that is blank on most rows reads as a
                     column that failed to load. -->
                <td class="col-owner">
                  {#if doc.owner}
                    <span class="owner">
                      <Avatar name={doc.owner} key={doc.owner_id || doc.owner} size={5} title="" />
                      <span class="owner-name">{mine(doc) ? "You" : doc.owner}</span>
                      {#if !mine(doc)}
                        <span class="badge preset-tonal-secondary text-xs" title="What you may do here">{doc.role}</span>
                      {/if}
                      <!-- And whoever else has been in. Three faces and a
                           count: the question this answers is "am I working
                           on this with anyone", which four faces answer no
                           better than three. Only shown on your own projects,
                           because on somebody else's it would tell you who
                           else holds the link you came in on. -->
                      {#if doc.people?.length}
                        <span class="people" title={doc.people.map((person) => person.name).join(", ")}>
                          {#each doc.people.slice(0, 3) as person (person.id)}
                            <Avatar name={person.name} key={person.id} size={5} title="" />
                          {/each}
                          {#if doc.people.length > 3}<span class="people-more">+{doc.people.length - 3}</span>{/if}
                        </span>
                      {/if}
                    </span>
                  {/if}
                </td>
                {#if place !== "trash"}
                  <td>{doc.comments || "—"}</td>
                  <td class="col-files">{doc.files ?? "—"}</td>
                {/if}
                <!-- Relative, with the date itself in the tooltip. Six copies
                     of today's date answer nothing; "12 min ago" answers the
                     question the column is here for. -->
                <td class="col-when whitespace-nowrap" title={isoDay(place === "recent" ? doc.opened_at : doc.updated_at)}>
                  {since(place === "recent" ? doc.opened_at : doc.updated_at)}
                </td>
                <td class="col-star">
                  {#if place === "trash"}
                    <span class="trash-actions">
                      <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => restore(doc)}>Put back</button>
                      <IconButton icon="trash" label="Delete {doc.title} for good" onclick={() => askPurge([doc])} />
                    </span>
                  {:else}
                    <!-- What can be done to a project without opening it.
                         Renaming keeps the slug, so every link already shared
                         still arrives; forking makes a project of your own,
                         which is why it is offered on somebody else's too. Deleting is one of
                         these rather than something the selection toolbar
                         alone can do: throwing one project away should not
                         need a mode. -->
                    {#if mine(doc)}
                      <IconButton icon="pencil" label="Rename {doc.title}" onclick={() => askRename(doc)} />
                    {/if}
                    <IconButton icon="git-fork" label="Fork {doc.title}"
                                disabled={forking === doc.slug} onclick={() => askFork(doc)} />
                    {#if mine(doc)}
                      <IconButton icon="trash" colour="text-surface-400-600 hover:text-error-500"
                                  label="Move {doc.title} to the trash" onclick={() => askDelete(doc)} />
                    {/if}
                    <IconButton
                      icon="star"
                      tone="plain"
                      size="btn-icon-sm"
                      colour={doc.favorite ? "text-tertiary-600" : "text-surface-400-600"}
                      filled={doc.favorite}
                      pressed={doc.favorite}
                      label={doc.favorite ? "Remove from favorites" : "Add to favorites"}
                      onclick={() => star(doc)}
                    />
                  {/if}
                </td>
              </tr>
            {:else}
              <tr>
                <td colspan="7" class="text-surface-600-400 h-48 text-center align-middle">
                  {nothingHere()}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
  </main>
{:else}
  <!-- The other page: what this is, and a way in. -->
  <Nav {me} />
  <Page width="measure">
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
    </Stack>
  </Page>
{/if}

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

<Modal open={Boolean(renaming)} title="Rename project"
       description="The name changes; the link does not, so anything already shared still works."
       onclose={() => { renaming = null; renameError = ""; }}>
  {#snippet children()}
    <form id="rename-project" onsubmit={reallyRename}>
      <label class="label">
        <span class="label-text">Name</span>
        <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
        <input class="input" autofocus bind:this={renameInput} bind:value={renameTo} />
      </label>
      {#if renameError}<p class="text-error-500 text-sm">{renameError}</p>{/if}
    </form>
  {/snippet}
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (renaming = null)}>Cancel</button>
    <button type="submit" form="rename-project" class="btn preset-filled-primary-500">Rename</button>
  {/snippet}
</Modal>

<!-- Naming the copy, not confirming it. The suggestion is already in the box
     and already selected, so the whole dialog is one keystroke if the
     numbered name will do and one sentence if it will not. -->
<Modal open={Boolean(copying)} title="Make a copy"
       description="The copy is a project of its own: its own link, its own history, nothing pointing back."
       onclose={() => { copying = null; copyError = ""; }}>
  {#snippet children()}
    <form id="copy-project" onsubmit={fork}>
      <label class="label">
        <span class="label-text">Name</span>
        <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
        <input class="input" autofocus bind:this={copyInput} bind:value={copyTo} />
      </label>
      {#if copyError}<p class="text-error-500 text-sm">{copyError}</p>{/if}
    </form>
  {/snippet}
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (copying = null)}>Cancel</button>
    <button type="submit" form="copy-project" class="btn preset-filled-primary-500" disabled={Boolean(forking)}>
      {forking ? "Copying..." : "Make a copy"}
    </button>
  {/snippet}
</Modal>

<Modal bind:open={confirming}
       title={confirmKind === "purge" ? "Delete for good?" : "Move to the trash?"}
       description={confirmText}>
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (confirming = false)}>
      Cancel
    </button>
    <button type="button" class="btn preset-filled-error-500"
            onclick={confirmKind === "purge" ? reallyPurge : reallyDelete}>
      {confirmKind === "purge" ? "Delete for good" : "Move to trash"}
    </button>
  {/snippet}
</Modal>

<Toasts />

<style>
  /* The place, in the bar, where a project's title goes when one is open. */
  .nav-place { font-weight: 600; }
  .project-search { width: 16rem; max-width: 32vw; }

  .projects { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; overflow-y: auto; padding: calc(var(--spacing) * 6) calc(var(--spacing) * 6) calc(var(--spacing) * 10); gap: calc(var(--spacing) * 4); }
  .place-heading { font-size: var(--text-2xl); font-weight: 600; margin: 0; }

  .starters { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); }
  .starters-say { color: var(--color-surface-600-400); font-size: var(--text-sm); margin-right: calc(var(--spacing)); }

  .toolbar { display: flex; align-items: center; gap: calc(var(--spacing) * 2); min-height: 2rem; }
  .toolbar .row-count { color: var(--color-surface-600-400); font-size: var(--text-sm); }
  .selection-count { font-size: var(--text-sm); font-weight: 600; }

  /* Rows, not boxes. The separators are the faintest thing that still reads
     as a row, because everything on this page was outlined before and the
     effect was that nothing on it was more important than anything else. */
  .projects-table :global(th) { border: 0; font-size: var(--text-xs); text-transform: uppercase; letter-spacing: .04em; color: var(--color-surface-600-400); font-weight: 600; }
  .projects-table :global(td) { border: 0; border-top: 1px solid var(--color-pane-edge); vertical-align: middle; }
  .projects-table :global(tbody tr:hover) { background: var(--color-surface-100-900); }
  .projects-table :global(tbody tr.picked) { background: var(--color-primary-100-900); }

  .col-mark { width: 2.75rem; }
  .col-star { width: 1px; white-space: nowrap; text-align: right; }
  .col-owner { white-space: nowrap; }

  /* The checkbox occupies one square. It used to share that square with a
     two-letter format mark and fade in over it; the mark is gone -- which
     language a project's source is written in is not what anybody comes to
     this list to read -- so the box is simply always there. */
  .mark { position: relative; display: inline-grid; place-items: center; width: 1.75rem; height: 1.75rem; }
  .mark .pick { position: absolute; inset: 0; margin: auto; }

  /* Native boxes, sized down and mostly faded. Skeleton's .checkbox drew a
     full primary outline on every row, which made the one column nobody
     reads the loudest thing in a list whose point is the titles. The accent is
     ink rather than the brand green, because a browser tints the *edge* of an
     unchecked box with it: at this size the edge is most of what the box is,
     and a column of them read as a column of green rings. */
  .tickbox { appearance: auto; width: .9rem; height: .9rem; accent-color: var(--color-surface-800); cursor: pointer; opacity: .45; transition: opacity 120ms ease; }
  .tickbox:hover, .tickbox:checked, .tickbox:indeterminate, .tickbox:focus-visible { opacity: 1; }
  .projects-table :global(tbody tr:hover .tickbox) { opacity: .8; }

  .title-line { display: block; font-weight: 500; color: var(--color-primary-600-400); text-decoration: none; }
  .title-line:hover { text-decoration: underline; }
  /* The second line of a row: who it is shared with, the file a search found,
     when the trash will take it. Empty on most rows, and collapsed when it
     is, so a row with nothing to add is one line tall. */
  .title-note { display: flex; gap: calc(var(--spacing) * 2); font-size: var(--text-xs); color: var(--color-surface-600-400); }
  .title-note:empty { display: none; }

  .owner { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); }
  .owner-name { font-size: var(--text-sm); }
  .people { display: inline-flex; align-items: center; gap: 2px; margin-left: calc(var(--spacing)); }
  .people-more { font-size: var(--text-xs); color: var(--color-surface-600-400); }
  .trash-actions { display: inline-flex; align-items: center; gap: calc(var(--spacing)); }

  /* The two columns a phone has no room for: the listing is read there to
     find a project, and neither how many files it has nor who owns it is how
     anyone finds one. The day it changed stays, because on a narrow screen
     that is the only thing left distinguishing one row from another. Sorting
     by the hidden ones still works from a wider window. */
  @media (max-width: 760px) {
    .col-files, .col-owner { display: none; }
    .projects { padding-inline: calc(var(--spacing) * 3); }
    .project-search { width: 9rem; }
  }
</style>
