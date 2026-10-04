<script>
  import { untrack } from "svelte";
  import { Menu } from "@skeletonlabs/skeleton-svelte";
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
  import DataTable from "./DataTable.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import MenuIconButton from "./MenuIconButton.svelte";
  import Modal from "./Modal.svelte";
  import SettingsDialog from "./settings/SettingsDialog.svelte";
  import Hero from "./Hero.svelte";
  import Toasts from "./Toasts.svelte";
  import Sidebar from "./layout/Sidebar.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import Row from "./layout/Row.svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import { say } from "../lib/toast.svelte.js";
  import { SHELL_HEADERS, get, getPrivate, me as whoami, upload } from "../lib/api.js";
  import { day as isoDay, since } from "../lib/dates.js";
  import { PROJECT_TABS, PROJECT_TAB_IDS } from "../lib/panels.js";
  import { FORMATS, formatNamed, fillTemplate, matchTemplates } from "../lib/starter.js";
  import { BUILT_IN, templateFiles } from "../lib/templates.js";
  import { preparedProjects } from "../lib/offline-projects.js";
  import * as localBridge from "../lib/companion/client.js";

  let { initialSettings = "" } = $props();
  let settingsOpen = $state(untrack(() => initialSettings === "tools"));
  let settingsCategory = $state(untrack(() => initialSettings || "editor"));
  function openSettings(category = "editor") {
    settingsCategory = category;
    settingsOpen = true;
  }

  /** @type {{can_publish?: boolean; name?: string; handle?: string; publishers?: string}} */
  let me = $state({});
  // Whether /api/me answered at all. Without it the signed-out page is also
  // the page shown while the request is in flight and the page shown when the
  // server never answers, so a dead server is indistinguishable from being
  // signed out -- it just sits there.
  let reachedServer = $state(true);
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
  const compactSort = $derived(`${sortBy}:${ascending ? "asc" : "desc"}`);
  let naming = $state(false);
  let name = $state("");
  // The picker in the New project dialog. The format filter is the one thing
  // remembered between visits ("" is any); the rest starts fresh each time.
  let templateQuery = $state("");
  let formatFilter = $state("");
  let templateId = $state("blank");
  // The format chosen on the card when the filter is "any".
  let formatOverride = $state("");
  // The caller's own templates, as the server lists them: projects. One list
  // serves the picker and the Templates place, so there is one loader.
  /** @type {{slug: string; title: string; source_format: string; url: string}[]} */
  let templateRows = $state([]);
  // The same, in the shape of a built-in one, so the same filter and the same
  // cards serve both.
  const customTemplates = $derived(templateRows.map((row) => ({
    id: row.slug, slug: row.slug, name: row.title, description: "", keywords: [],
    formats: [row.source_format], custom: true, url: row.url,
  })));
  const allTemplates = $derived([
    ...BUILT_IN.map((template) => ({ ...template, slug: template.id, custom: false, url: "" })),
    ...customTemplates,
  ]);
  const shownTemplates = $derived(matchTemplates(allTemplates, templateQuery, formatFilter));
  const shownBuiltIn = $derived(shownTemplates.filter((each) => !each.custom));
  const shownCustom = $derived(shownTemplates.filter((each) => each.custom));
  const chosenTemplate = $derived(allTemplates.find((each) => each.id === templateId));
  function reconcileTemplateSelection() {
    const shown = shownTemplates;
    if (!shown.some((each) => each.id === templateId)) templateId = shown[0]?.id ?? "";
  }
  $effect(() => reconcileTemplateSelection());
  // A template that does not offer the filtered format falls back to its
  // first, rather than refusing the click.
  const format = $derived.by(() => {
    const offered = chosenTemplate?.formats ?? [];
    if (offered.includes(formatFilter)) return formatFilter;
    if (offered.includes(formatOverride)) return formatOverride;
    return offered[0] ?? FORMATS[0].id;
  });
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
  // Hiding a shared project is a personal listing preference. Keep each
  // request pending until the server confirms it so a failed write cannot
  // make the row disappear locally.
  let sharedVisibilityPending = $state(new Set());

  // Which columns each place has. The table itself is DataTable's; what
  // differs between the places is this list, which is data rather than three
  // copies of a table with the differences written into ternaries.
  //
  // A width here is where a column starts, not where it stays: every one of
  // them can be dragged, and what the reader drags it to is what it keeps.
  // Only the two columns that hold controls rather than content are fixed --
  // there is nothing in them to make room for.
  //
  // On a narrow window the two widest columns of metadata go. They are
  // dropped from the list rather than hidden in CSS, because a column that is
  // still in the table still has a width, and the table would keep its room.
  let narrow = $state(false);
  $effect(() => {
    const query = window.matchMedia("(max-width: 760px)");
    const watch = () => (narrow = query.matches);
    watch();
    query.addEventListener("change", watch);
    return () => query.removeEventListener("change", watch);
  });

  const columns = $derived.by(() => {
    const trash = place === "trash";
    // Typed loosely on purpose: the list is grown by `push` below, and a
    // literal inferred from its first two entries refuses every later one.
    const list = /** @type {Record<string, any>[]} */ ([
      { key: "mark", label: "", width: 44, resizable: false },
      { key: "title", label: "Project", sortable: true, min: 160 },
    ]);
    if (!narrow) list.push({ key: "owner", label: "Owner", sortable: true, width: 210, min: 120 });
    if (!trash && !narrow) list.push({ key: "files", label: "Files", sortable: true, width: 90, min: 60 });
    if (!narrow) {
      list.push({
        key: "updated",
        label: trash ? "Deleted" : place === "recent" ? "Opened" : "Updated",
        sortable: true,
        width: 130,
        min: 90,
        class: "col-when",
      });
    }
    list.push({
      key: "actions",
      label: "",
      // A narrow row has one trigger; its actions live in the menu.
      width: narrow ? 48 : trash ? 88 : 150,
      resizable: false,
      align: "right",
      class: "col-actions",
    });
    return list;
  });

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
    if (id === "templates") void loadTemplates();
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
  const pathRequests = new Map();
  const PATH_CONCURRENCY = 6;

  function requestPaths(doc) {
    if (pathRequests.has(doc.slug)) return pathRequests.get(doc.slug);
    const pending = get("/api/documents/" + doc.slug)
      .then((full) => Array.isArray(full.files) ? full.files : [])
      .catch(() => [])
      .then((files) => {
        // Merge into the current map, not the snapshot from when this batch
        // began; slower searches must not erase newer completions.
        paths = new Map(paths).set(doc.slug, files);
        return files;
      })
      .finally(() => pathRequests.delete(doc.slug));
    pathRequests.set(doc.slug, pending);
    return pending;
  }

  async function learnPaths() {
    const wanted = documents.filter((doc) => !paths.has(doc.slug) && !doc.offline_prepared);
    if (!wanted.length) return;
    let next = 0;
    const worker = async () => {
      while (next < wanted.length) await requestPaths(wanted[next++]);
    };
    await Promise.all(Array.from({ length: Math.min(PATH_CONCURRENCY, wanted.length) }, worker));
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
    shared: (doc) => !mine(doc) && !doc.shared_hidden,
    favorites: (doc) => doc.favorite,
    trash: () => true,
    templates: () => true,
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
      templates: "No templates yet. Open a project and choose File > Save as template.",
    }[place];
  }

  const needle = $derived(search.trim().toLowerCase());
  // Typing is the request. Nothing is fetched until there is something to
  // look for, and each project is asked once however long the search runs.
  $effect(() => {
    if (needle.length >= 2) void learnPaths();
  });
  const source = $derived(place === "trash" ? trashed : place === "templates" ? templateRows : documents);
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
  // A project of your own. What the onboarding strip watches: once there is
  // work here, it has done its job.
  const ownWork = $derived(documents.filter((doc) => mine(doc)).length);

  function sortColumn(column) {
    // Clicking the column already sorted reverses it; a new column starts in
    // the order that column is usually wanted in.
    if (sortBy === column) ascending = !ascending;
    else {
      sortBy = column;
      ascending = column === "title" || column === "owner";
    }
  }

  function chooseCompactSort(value) {
    const [column, direction] = value.split(":");
    sortBy = column;
    ascending = direction === "asc";
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

  async function setSharedVisibility(doc, hidden) {
    if (sharedVisibilityPending.has(doc.slug) || mine(doc)) return;
    sharedVisibilityPending = new Set(sharedVisibilityPending).add(doc.slug);
    try {
      const response = await fetch(`/api/documents/${doc.slug}/shared`, {
        method: hidden ? "DELETE" : "POST",
        headers: SHELL_HEADERS,
      });
      if (!response.ok) {
        const detail = (await response.json().catch(() => ({}))).error;
        throw new Error(detail || (hidden ? "That project could not be removed from Shared." : "That project could not be shown in Shared."));
      }
      doc.shared_hidden = hidden;
      say(hidden
        ? `${doc.title} was removed from your Shared list. You can show it again from All projects.`
        : `${doc.title} will appear in your Shared list again.`, {
          id: `landing:shared:${doc.slug}`,
          ...(!hidden ? {} : { action: { label: "Undo", onClick: () => void setSharedVisibility(doc, false) } }),
        });
    } catch (error) {
      say(error?.message || (hidden ? "That project could not be removed from Shared." : "That project could not be shown in Shared."), {
        kind: "problem",
        id: `landing:shared-error:${doc.slug}`,
      });
    } finally {
      const pending = new Set(sharedVisibilityPending);
      pending.delete(doc.slug);
      sharedVisibilityPending = pending;
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
    // A template is a project, so trashing one from the picker or from the
    // Templates place takes this path, and what it was listed in is stale.
    if (naming || place === "templates") await loadTemplates();
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
      if (place === "templates") await loadTemplates();
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

  // Skeleton closes the portalled menu on selection. Let that finish before
  // opening a focus-trapping dialog so focus does not race the new modal.
  function compactProjectAction(action, doc) {
    setTimeout(() => {
      if (action === "rename") askRename(doc);
      else if (action === "fork") askFork(doc);
      else if (action === "from-template") askNameFrom(doc);
      else if (action === "favorite") void star(doc);
      else if (action === "hide-shared") void setSharedVisibility(doc, true);
      else if (action === "show-shared") void setSharedVisibility(doc, false);
      else if (action === "trash") askDelete(doc);
      else if (action === "restore") void restore(doc);
      else if (action === "purge") askPurge([doc]);
    }, 0);
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

  // Making a project and filling it are two acts, not one. This asks for the
  // name -- the one thing nothing else can supply -- and for a template, which
  // is a format and a first set of files. A built-in template is plain files
  // filled in here; one of your own is a project, copied by the server like
  // any fork. Everything afterwards happens in the project's own file
  // explorer, which already knows how to take a file, a folder, or an archive.

  const FORMAT_KEY = "librepaper.template-format";

  function rememberedFormat() {
    try {
      const saved = localStorage.getItem(FORMAT_KEY) ?? "";
      return FORMATS.some((each) => each.id === saved) ? saved : "";
    } catch {
      return "";
    }
  }

  function chooseFormat(id) {
    formatFilter = id;
    try {
      localStorage.setItem(FORMAT_KEY, id);
    } catch {
      // Not remembering is a convenience lost, not an error.
    }
  }

  function chooseTemplate(id) {
    templateId = id;
    formatOverride = "";
  }

  // Your own templates, for the dialog and for the Templates place alike.
  async function loadTemplates() {
    try {
      const bySlug = new Map();
      let cursor = null;
      do {
        const query = new URLSearchParams();
        if (cursor?.after_updated && cursor?.after_id) {
          query.set("after_updated", cursor.after_updated);
          query.set("after_id", cursor.after_id);
        }
        const suffix = query.toString();
        const page = await getPrivate(`/api/templates${suffix ? `?${suffix}` : ""}`);
        if (!Array.isArray(page.templates)) throw new Error("refresh returned an invalid template listing");
        for (const template of page.templates) {
          if (template?.slug) bySlug.set(template.slug, template);
        }
        cursor = page.next_cursor;
      } while (cursor?.after_updated && cursor?.after_id);
      templateRows = [...bySlug.values()];
      if (!allTemplates.some((each) => each.id === templateId)) chooseTemplate("blank");
    } catch {
      // Signed out, or offline: the built-in templates are all there is.
      templateRows = [];
    }
  }

  function askName(chosen) {
    name = "";
    templateQuery = "";
    formatFilter = chosen || rememberedFormat();
    chooseTemplate("blank");
    nameError = "";
    naming = true;
    void loadTemplates();
  }

  // From a row in the Templates place: the dialog opens with that template
  // already chosen, under any format, so its card is not filtered away.
  function askNameFrom(doc) {
    askName();
    formatFilter = "";
    chooseTemplate(doc.slug);
  }

  function askDeleteTemplate(template) {
    askDelete({ slug: template.slug, title: template.name });
  }

  async function create(event) {
    event.preventDefault();
    if (busy) return;
    if (!chosenTemplate || !shownTemplates.some((each) => each.id === chosenTemplate.id)) {
      nameError = "Choose a visible template.";
      return;
    }
    const named = name.trim();
    if (!named) {
      nameError = "Give the project a name.";
      nameInput?.focus();
      return;
    }
    busy = true;
    nameError = "";
    try {
      let response;
      if (chosenTemplate?.custom) {
        response = await fetch(`/api/documents/${chosenTemplate.slug}/fork`, {
          method: "POST",
          headers: { ...SHELL_HEADERS, "Content-Type": "application/json" },
          body: JSON.stringify({ title: named }),
        });
      } else {
        // The same multipart publish a directory uses. The server names the
        // format from the main file's extension, and `main` says which file
        // that is now that a template has more than one.
        const files = fillTemplate(await templateFiles(templateId, format), format, { title: named, author: me.name });
        const form = new FormData();
        for (const file of files) form.append("file", new Blob([file.text], { type: "text/plain" }), file.path);
        form.append("title", named);
        form.append("main", `main.${formatNamed(format).extension}`);
        response = await upload(form);
      }
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
    localBridge.configure({ project: null, origin: location.origin, active: true });
    get("/api/config").then((answer) => {
      if (answer?.local_app?.address) localBridge.setAdvertisedAddress(answer.local_app.address);
    }).catch(() => {});
    // Not the shared me(), which folds a failed request into an empty account:
    // this page needs to tell the two apart in order to say which it is.
    get("/api/me")
      .catch(() => {
        reachedServer = false;
        return {};
      })
      .then(async (who) => {
        me = who;
        if (who.can_publish) {
          await showList();
          await showOfflineProjects();
          if (place === "trash") await showTrash();
          if (place === "templates") await loadTemplates();
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
      <button type="button" class="btn btn-sm lp-control-outline" onclick={() => openSettings()}>Settings</button>
      {#if me.can_publish}
        <button type="button" class="btn btn-sm nav-new lp-control-brand" aria-label="New project" onclick={() => askName()}>
          <Icon name="file-plus" size={16} />
          <span class="nav-new-label">New project</span>
        </button>
      {/if}
    {/snippet}
  </Nav>

  <main id="main" tabindex="-1" class="workspace">
    <!-- The rail. Nothing beside it: these are places, and the place you
         choose fills the page rather than a column. `shown.comments` false is
         what the reader's own collapsed column is, so the width here and the
         width there are the same width. -->
    <Sidebar tabs={PROJECT_TABS} panel={place} shown={{ comments: false }} compactLabels={narrow}
             resizable={false} label="Projects" onselectpanel={goTo}
             badges={trashed.length ? { trash: { counts: [{ tone: "warnings", of: trashed.length }] } } : {}}>
      {#snippet controls()}{/snippet}
    </Sidebar>

    <section class="projects">
      <h1 class="place-heading">{PLACE_NAMES[place]}</h1>

      {#if narrow && ["projects", "shared", "favorites", "templates"].includes(place)}
        <div class="compact-sort">
          <label for="project-sort">Sort</label>
          <select id="project-sort" class="select" value={compactSort}
                  onchange={(event) => chooseCompactSort(event.currentTarget.value)}>
            <option value="updated:desc">Updated: newest first</option>
            <option value="updated:asc">Updated: oldest first</option>
            <option value="title:asc">Title: A–Z</option>
            <option value="title:desc">Title: Z–A</option>
            {#if sortBy === "owner"}
              <option value={`owner:${ascending ? "asc" : "desc"}`}>Owner: {ascending ? "A–Z" : "Z–A"}</option>
            {:else if sortBy === "files"}
              <option value={`files:${ascending ? "asc" : "desc"}`}>Files: {ascending ? "fewest first" : "most first"}</option>
            {/if}
          </select>
        </div>
      {/if}

      <!-- Where a project comes from, for an account that has not made one
           yet. It is above the list rather than in it: a template is
           something to start from, and a row in this table is something you
           already have. It leaves once there is real work here. -->
      {#if place === "projects" && me.can_publish && ownWork === 0}
        <div class="starters">
          <span class="starters-say">Start something new</span>
          {#each FORMATS as choice}
            <button type="button" class="btn btn-sm lp-control-outline"
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
            <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={restoreSelected}>
              <Icon name="undo-2" size={16} /> Put back
            </button>
            <button type="button" class="btn btn-sm lp-tone-error"
                    onclick={() => askPurge(shown.filter((doc) => selected.has(doc.slug)))}>
              <Icon name="trash" size={16} /> Delete forever
            </button>
          {:else}
            <button type="button" class="btn btn-sm lp-tone-error" onclick={deleteSelected}>
              <Icon name="trash" size={16} /> Delete
            </button>
          {/if}
          <IconButton icon="x" label="Clear the selection" onclick={() => (selected = new Set())} />
        </div>
      {:else if place !== "trash"}
        <div class="toolbar">
          <span class="row-count">{shown.length}{place === "templates" ? (shown.length === 1 ? " template" : " templates") : shown.length === 1 ? " project" : " projects"}</span>
        </div>
      {:else}
        <div class="toolbar">
          <span class="row-count">Projects here are deleted for good seven days after they were put in the trash.</span>
        </div>
      {/if}

      <DataTable
        id="projects:{place}"
        {columns}
        rows={shown}
        rowKey={(doc) => doc.slug}
        rowClass={(doc) => (selected.has(doc.slug) ? "picked" : "")}
        className="projects-table"
        label={PLACE_NAMES[place]}
        {sortBy}
        {ascending}
        onsort={sortColumn}
      >
        {#snippet head(column)}
          {#if column.key === "mark" && selectable > 0}
            <!-- The same square the rows use, so the box at the head of the
                 column sits on the same centre as the ones under it. -->
            <span class="tickcell">
              <input
                type="checkbox"
                class="tickbox pick"
                aria-label="Select every project shown"
                checked={hereSelected === selectable}
                indeterminate={hereSelected > 0 && hereSelected < selectable}
                onchange={(event) => tickAll(event.currentTarget.checked)}
              />
            </span>
          {/if}
        {/snippet}

        {#snippet cell(column, doc)}
          {#if column.key === "mark"}
            <!-- Just the checkbox. One narrow cell, so a row that can be
                 selected is no wider than one that cannot and the title
                 starts at the same x in both. -->
            <span class="tickcell">
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
          {:else if column.key === "title"}
            {#if place === "trash"}
              <span class="title-line">{doc.title}</span>
            {:else}
              <a class="title-line" href="/docs/{doc.slug}">{doc.title}</a>
            {/if}
            <span class="title-note">
              {#if doc.offline_prepared}<span>Offline</span>{/if}
              <!-- The file the search found, when it was not the title.
                   Opening it opens the project on that file. -->
              {#if found(doc, needle).path}
                <a href="/docs/{doc.slug}?file={encodeURIComponent(found(doc, needle).path)}">
                  {found(doc, needle).path}
                </a>
              {/if}
              {#if place === "trash" && doc.purge_due && !narrow}
                <span>Deleted for good {since(doc.purge_due)}</span>
              {/if}
            </span>
          {:else if column.key === "owner"}
            <!-- Who it belongs to. Yours says so rather than saying nothing:
                 a column that is blank on most rows reads as a column that
                 failed to load. -->
            {#if doc.owner}
              <span class="owner-name">{mine(doc) ? "You" : doc.owner}</span>
            {/if}
          {:else if column.key === "files"}
            {doc.files ?? "—"}
          {:else if column.key === "updated"}
            <!-- Relative, with the date itself in the tooltip. Six copies of
                 today's date answer nothing; "12 min ago" answers the
                 question the column is here for. -->
            <span title={isoDay(place === "recent" ? doc.opened_at : doc.updated_at)}>
              {since(place === "recent" ? doc.opened_at : doc.updated_at)}
            </span>
          {:else if column.key === "actions"}
            {#if narrow}
              <Menu
                onSelect={(chosen) => compactProjectAction(chosen.value, doc)}
                positioning={{ placement: "bottom-end", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}
              >
                <MenuIconButton icon="more-horizontal" label={`Actions for ${doc.title}`}
                                projectSlug={doc.slug} class="project-actions-trigger" />
                <ExplorerMenu>
                  {#if place === "trash"}
                    <Menu.Item value="restore" class="menuitem project-action-item" data-project-action="restore">
                      <Icon name="undo-2" size="1rem" /><span class="menuitem-label">Put back</span>
                    </Menu.Item>
                    <Menu.Item value="purge" class="menuitem project-action-item" data-project-action="purge">
                      <Icon name="trash" size="1rem" /><span class="menuitem-label">Delete for good</span>
                    </Menu.Item>
                  {:else if place === "templates"}
                    <Menu.Item value="from-template" class="menuitem project-action-item" data-project-action="from-template">
                      <Icon name="file-plus" size="1rem" /><span class="menuitem-label">New project from template</span>
                    </Menu.Item>
                    <Menu.Item value="rename" class="menuitem project-action-item" data-project-action="rename">
                      <Icon name="pencil" size="1rem" /><span class="menuitem-label">Rename</span>
                    </Menu.Item>
                    <Menu.Item value="trash" class="menuitem project-action-item" data-project-action="trash">
                      <Icon name="trash" size="1rem" /><span class="menuitem-label">Move to trash</span>
                    </Menu.Item>
                  {:else}
                    {#if mine(doc)}
                      <Menu.Item value="rename" class="menuitem project-action-item" data-project-action="rename">
                        <Icon name="pencil" size="1rem" /><span class="menuitem-label">Rename</span>
                      </Menu.Item>
                    {/if}
                    {#if !mine(doc) && place === "shared"}
                      <Menu.Item value="hide-shared" class="menuitem project-action-item" data-project-action="hide-shared" disabled={sharedVisibilityPending.has(doc.slug)}>
                        <Icon name="eye-off" size="1rem" /><span class="menuitem-label">Remove from Shared</span>
                      </Menu.Item>
                    {:else if !mine(doc) && place === "projects" && doc.shared_hidden}
                      <Menu.Item value="show-shared" class="menuitem project-action-item" data-project-action="show-shared" disabled={sharedVisibilityPending.has(doc.slug)}>
                        <Icon name="eye" size="1rem" /><span class="menuitem-label">Show in Shared</span>
                      </Menu.Item>
                    {/if}
                    <Menu.Item value="fork" class="menuitem project-action-item" data-project-action="fork" disabled={forking === doc.slug}>
                      <Icon name="git-fork" size="1rem" /><span class="menuitem-label">Fork</span>
                    </Menu.Item>
                    <Menu.Item value="favorite" class="menuitem project-action-item" data-project-action="favorite">
                      <Icon name="star" size="1rem" filled={doc.favorite} />
                      <span class="menuitem-label">{doc.favorite ? "Remove from favorites" : "Add to favorites"}</span>
                    </Menu.Item>
                    {#if mine(doc)}
                      <Menu.Item value="trash" class="menuitem project-action-item" data-project-action="trash">
                        <Icon name="trash" size="1rem" /><span class="menuitem-label">Move to trash</span>
                      </Menu.Item>
                    {/if}
                  {/if}
                </ExplorerMenu>
              </Menu>
            {:else if place === "templates"}
              <span class="project-actions">
                <IconButton icon="file-plus" title="New project from template" label="New project from {doc.title}" onclick={() => askNameFrom(doc)} />
                <IconButton icon="pencil" title="Rename" label="Rename {doc.title}" onclick={() => askRename(doc)} />
                <IconButton icon="trash" colour="lp-text-muted lp-hover-error"
                            title="Move to trash" label="Move {doc.title} to the trash" onclick={() => askDelete(doc)} />
              </span>
            {:else if place === "trash"}
              <span class="trash-actions">
                <!-- The tooltip says the action; the accessible name says the
                     action and which project, because a row of identical
                     icons read aloud is otherwise four unplaced verbs. -->
                <IconButton icon="undo-2" title="Put back" label="Put {doc.title} back in your projects" onclick={() => restore(doc)} />
                <IconButton icon="trash" title="Delete for good" label="Delete {doc.title} for good" onclick={() => askPurge([doc])} />
              </span>
            {:else}
              <!-- What can be done to a project without opening it. Renaming
                   keeps the slug, so every link already shared still arrives;
                   forking makes a project of your own, which is why it is
                   offered on somebody else's too. Deleting is one of these
                   rather than something the selection toolbar alone can do:
                   throwing one project away should not need a mode. -->
              <span class="project-actions">
                {#if mine(doc)}
                  <IconButton icon="pencil" title="Rename" label="Rename {doc.title}" onclick={() => askRename(doc)} />
                {/if}
                {#if !mine(doc) && place === "shared"}
                  <IconButton icon="eye-off" title="Remove from Shared" label="Remove {doc.title} from Shared" disabled={sharedVisibilityPending.has(doc.slug)} onclick={() => setSharedVisibility(doc, true)} />
                {:else if !mine(doc) && place === "projects" && doc.shared_hidden}
                  <IconButton icon="eye" title="Show in Shared" label="Show {doc.title} in Shared" disabled={sharedVisibilityPending.has(doc.slug)} onclick={() => setSharedVisibility(doc, false)} />
                {/if}
                <IconButton icon="git-fork" title="Fork" label="Fork {doc.title}"
                            disabled={forking === doc.slug} onclick={() => askFork(doc)} />
                {#if mine(doc)}
                  <IconButton icon="trash" colour="lp-text-muted lp-hover-error"
                              title="Move to trash" label="Move {doc.title} to the trash" onclick={() => askDelete(doc)} />
                {/if}
                <IconButton
                  icon="star"
                  tone="plain"
                  size="btn-icon-sm"
                  colour={doc.favorite ? "lp-text-warning" : "lp-text-muted"}
                  filled={doc.favorite}
                  pressed={doc.favorite}
                  label={doc.favorite ? "Remove from favorites" : "Add to favorites"}
                  onclick={() => star(doc)}
                />
              </span>
            {/if}
          {/if}
        {/snippet}

        {#snippet empty()}{nothingHere()}{/snippet}
      </DataTable>
    </section>
  </main>
{:else}
  <!-- The other page: what this is, and a way in. -->
  <Nav {me}>
    {#snippet tools()}
      <button type="button" class="btn btn-sm lp-control-outline" onclick={() => openSettings()}>Settings</button>
    {/snippet}
  </Nav>
  <Page width="measure">
    <Stack gap={8}>
      <header>
        <Hero />
        <p class="lp-text-secondary text-center">
          Start a project, write it with whoever you like, and share its link to collect comments.
        </p>
      </header>
      {#if !reachedServer}
        <aside class="card lp-tone-warning p-4">
          This page could not reach the server, so it cannot tell whether you
          are signed in. Your projects will appear once the server answers.
        </aside>
      {/if}
      <!-- The handle, not the name: this is about the allowlist, which is
           written in handles, and it is shown only to the person it refuses. -->
      {#if me.handle && !me.can_publish}
        <aside class="card lp-tone-warning p-4">
          {me.handle} may not publish here; this deployment allows {me.publishers}.
        </aside>
      {/if}
    </Stack>
  </Page>
{/if}

<SettingsDialog bind:open={settingsOpen} bind:category={settingsCategory} account={me} />

<!-- A name and a template, because a project is a directory and a main file
     and nothing here can guess either. More files come next, in the project. -->
{#snippet templateCard(template)}
  <div class="tcard" class:picked={templateId === template.id}>
    <button type="button" class="tcard-pick" aria-pressed={templateId === template.id}
            onclick={() => chooseTemplate(template.id)}>
      <span class="tcard-name" class:with-actions={template.custom}>{template.name}</span>
      {#if template.description}<span class="tcard-say">{template.description}</span>{/if}
      <span class="tcard-formats">
        {#each template.formats as id}<span class="tcard-badge">{formatNamed(id).name}</span>{/each}
      </span>
    </button>
    {#if template.custom}
      <span class="tcard-actions">
        <IconButton icon="pencil" title="Edit" label="Edit {template.name}" onclick={() => (location.href = template.url)} />
        <IconButton icon="trash" title="Move to trash" label="Move {template.name} to the trash" onclick={() => askDeleteTemplate(template)} />
      </span>
    {/if}
  </div>
{/snippet}

<Modal
  bind:open={naming}
  title="New project"
  wide
  onclose={() => (nameError = "")}
  confirm={{ form: "new-project", label: busy ? "Creating…" : "Create project", disabled: busy, oncancel: () => (naming = false) }}
>
  {#snippet children()}
    <form id="new-project" onsubmit={create}>
      <Stack gap={3}>
        <label class="label">
          <span class="label-text">Project name</span>
          <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
          <input class="input" autofocus bind:this={nameInput} bind:value={name} />
        </label>
        <label class="label">
          <span class="label-text">Search templates</span>
          <input class="input" type="search" placeholder="Search templates" bind:value={templateQuery} />
        </label>
        <div class="tformats" role="group" aria-label="Format">
          {#each [{ id: "", name: "Any" }, ...FORMATS] as choice}
            <button type="button" class="btn btn-sm {formatFilter === choice.id ? 'lp-control-tonal-brand' : 'lp-control-outline'}"
                    aria-pressed={formatFilter === choice.id} onclick={() => chooseFormat(choice.id)}>{choice.name}</button>
          {/each}
        </div>
        <div class="tlist">
          {#if !shownTemplates.length}
            <p class="lp-text-secondary text-sm">No template matches.</p>
          {/if}
          {#if shownBuiltIn.length}
            <div class="tgrid">
              {#each shownBuiltIn as template (template.id)}{@render templateCard(template)}{/each}
            </div>
          {/if}
          {#if shownCustom.length}
            <h3 class="tgroup">Your templates</h3>
            <div class="tgrid">
              {#each shownCustom as template (template.id)}{@render templateCard(template)}{/each}
            </div>
          {/if}
        </div>
        {#if chosenTemplate && !chosenTemplate.custom && !formatFilter && chosenTemplate.formats.length > 1}
          <label class="label">
            <span class="label-text">Format</span>
            <select class="select" value={format} onchange={(event) => (formatOverride = event.currentTarget.value)}>
              {#each chosenTemplate.formats as id}
                <option value={id}>{formatNamed(id).name}</option>
              {/each}
            </select>
          </label>
        {/if}
        {#if nameError}<p class="lp-text-error text-sm">{nameError}</p>{/if}
      </Stack>
    </form>
  {/snippet}
</Modal>

<Modal open={Boolean(renaming)} title="Rename project"
       description="The name changes; the link does not, so anything already shared still works."
       onclose={() => { renaming = null; renameError = ""; }}
       confirm={{ form: "rename-project", label: "Rename", oncancel: () => (renaming = null) }}>
  {#snippet children()}
    <form id="rename-project" onsubmit={reallyRename}>
      <label class="label">
        <span class="label-text">Name</span>
        <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
        <input class="input" autofocus bind:this={renameInput} bind:value={renameTo} />
      </label>
      {#if renameError}<p class="lp-text-error text-sm">{renameError}</p>{/if}
    </form>
  {/snippet}
</Modal>

<!-- Naming the copy, not confirming it. The suggestion is already in the box
     and already selected, so the whole dialog is one keystroke if the
     numbered name will do and one sentence if it will not. -->
<Modal open={Boolean(copying)} title="Make a copy"
       description="The copy is a project of its own: its own link, its own history, nothing pointing back."
       onclose={() => { copying = null; copyError = ""; }}
       confirm={{ form: "copy-project", label: forking ? "Copying..." : "Make a copy", disabled: Boolean(forking), oncancel: () => (copying = null) }}>
  {#snippet children()}
    <form id="copy-project" onsubmit={fork}>
      <label class="label">
        <span class="label-text">Name</span>
        <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
        <input class="input" autofocus bind:this={copyInput} bind:value={copyTo} />
      </label>
      {#if copyError}<p class="lp-text-error text-sm">{copyError}</p>{/if}
    </form>
  {/snippet}
</Modal>

<Modal bind:open={confirming}
       title={confirmKind === "purge" ? "Delete for good?" : "Move to the trash?"}
       description={confirmText}
       confirm={{
         label: confirmKind === "purge" ? "Delete for good" : "Move to trash",
         tone: "error",
         onclick: confirmKind === "purge" ? reallyPurge : reallyDelete,
       }}></Modal>

<Toasts />

<style>
  /* The place, in the bar, where a project's title goes when one is open. */
  .nav-place { font-weight: 600; }
  .project-search { width: 16rem; max-width: 32vw; }
  /* The bar decides the height of its own row: the button takes the height
     of the search field beside it rather than a size of its own, so the two
     read as one row of controls however either is restyled later. */
  .nav-new { align-self: stretch; }

  .projects { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; overflow-y: auto; padding: calc(var(--spacing) * 6) calc(var(--spacing) * 6) calc(var(--spacing) * 10); gap: calc(var(--spacing) * 4); }
  .place-heading { font-size: var(--text-2xl); font-weight: 600; margin: 0; }

  .starters { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); }
  .starters-say { color: var(--color-text-secondary); font-size: var(--text-sm); margin-right: calc(var(--spacing)); }

  /* The template picker. Cards fill the width in as many columns as fit, and
     the list scrolls on its own so the name and the buttons stay in view. */
  .tformats { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 2); }
  .tlist { max-height: 40vh; overflow-y: auto; display: flex; flex-direction: column; gap: calc(var(--spacing) * 3); }
  .tgrid { display: grid; grid-template-columns: repeat(auto-fill, minmax(11rem, 1fr)); gap: calc(var(--spacing) * 2); }
  .tgroup { font-size: var(--text-sm); font-weight: 600; color: var(--color-text-secondary); margin: 0; }
  .tcard { position: relative; display: flex; border: 1px solid var(--color-border); border-radius: 9px; background: var(--color-raised); }
  .tcard.picked { border-color: var(--color-brand); background: var(--color-row-selected); }
  .tcard-pick { display: flex; flex-direction: column; align-items: flex-start; gap: calc(var(--spacing)); flex: 1 1 auto; min-width: 0; padding: calc(var(--spacing) * 3); text-align: left; background: transparent; border: 0; border-radius: inherit; color: var(--color-text); cursor: pointer; }
  .tcard-pick:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 2px; }
  .tcard-name { font-weight: 600; overflow-wrap: anywhere; }
  .tcard-name.with-actions { padding-right: 4rem; }
  .tcard-say { font-size: var(--text-xs); color: var(--color-text-secondary); }
  .tcard-formats { display: flex; flex-wrap: wrap; gap: calc(var(--spacing)); margin-top: auto; }
  .tcard-badge { font-size: var(--text-xs); padding: 0 calc(var(--spacing) * 1.5); border-radius: 999px; background: var(--color-subtle); color: var(--color-text-secondary); }
  .tcard-actions { position: absolute; top: calc(var(--spacing)); right: calc(var(--spacing)); display: inline-flex; }

  .toolbar { display: flex; align-items: center; gap: calc(var(--spacing) * 2); min-height: 2rem; }
  .toolbar .row-count { color: var(--color-text-secondary); font-size: var(--text-sm); }
  .selection-count { font-size: var(--text-sm); font-weight: 600; }
  .compact-sort { display: none; }

  /* Rows, not boxes. The separators are the faintest thing that still reads
     as a row, because everything on this page was outlined before and the
     effect was that nothing on it was more important than anything else. */
  :global(.projects-table th) { border: 0; font-size: var(--text-xs); text-transform: uppercase; letter-spacing: .04em; color: var(--color-text-secondary); font-weight: 600; }
  :global(.projects-table td) { border: 0; border-top: 1px solid var(--color-pane-edge); vertical-align: middle; }
  :global(.projects-table tbody tr:hover) { background: var(--color-subtle); }
  :global(.projects-table tbody tr.picked) { background: var(--color-row-selected); }

  /* The one column that has to say more than its width allows. Every other
     width is in the column list at the head of this file. */
  :global(.projects-table .col-when) { white-space: nowrap; }

  /* The checkbox occupies one square. It used to share that square with a
     two-letter format mark and fade in over it; the mark is gone -- which
     language a project's source is written in is not what anybody comes to
     this list to read -- so the box is simply always there.

     Not `.mark`: Skeleton has a utility of that name, the highlighter pen,
     which fills whatever wears it with tertiary-500 and gives it a radius and
     padding. A utility beats a scoped component rule for the properties it
     sets, so the square came out as a green lozenge behind the box, and the
     checkbox itself inherited its text colour -- `input { color: inherit }`
     in Tailwind's base. A component class has to be a name no utility has. */
  /* The type size is pinned here because the box below is sized in em and
     the head of a column is smaller type than its rows: without this the
     select-all box comes out narrower than the boxes it selects. */
  .tickcell { position: relative; display: inline-grid; place-items: center; width: 1.75rem; height: 1.75rem; font-size: var(--text-sm); }
  .tickcell .pick { position: absolute; inset: 0; margin: auto; }

  /* The box the browser draws, at the size of the text beside it. Skeleton's
     .checkbox painted a full brand-green outline on every row, which made the
     one column nobody reads the loudest thing in a list whose point is the
     titles.

     The size is in em, so it tracks the row's type rather than sitting at
     whatever the browser's own default happens to be -- a control that reads
     as part of a line of text is about as tall as that text. It also has to
     be said: the box is absolutely positioned into its square with `inset: 0`,
     and a replaced element told to fill a box with no size of its own fills
     all 28px of it. */
  .tickbox { appearance: auto; width: 1em; height: 1em; margin: 0; cursor: pointer; }

  .title-line { display: block; font-weight: 500; color: var(--color-link); text-decoration: none; }
  .title-line:hover { text-decoration: underline; }
  /* The second line of a row: who it is shared with, the file a search found,
     when the trash will take it. Empty on most rows, and collapsed when it
     is, so a row with nothing to add is one line tall. */
  .title-note { display: flex; gap: calc(var(--spacing) * 2); font-size: var(--text-xs); color: var(--color-text-secondary); }
  .title-note:empty { display: none; }

  .owner-name { font-size: var(--text-sm); }
  .trash-actions { display: inline-flex; align-items: center; gap: calc(var(--spacing)); }
  .project-actions { display: inline-flex; align-items: center; gap: calc(var(--spacing)); }
  :global(.project-actions-trigger) { display: inline-flex; width: 2.75rem; height: 2.75rem; align-items: center; justify-content: center; padding: 0; font-size: 1.5rem; line-height: 1; }
  :global(.project-action-item) { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }

  /* The metadata columns a phone has no room for: the listing is read there
     to find a project, and neither how many files it has, who owns it, nor
     when it changed belongs in the compact row. */
  @media (max-width: 760px) {
    .compact-sort { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }
    .compact-sort label { font-size: var(--text-sm); font-weight: 600; }
    .compact-sort select { width: auto; min-width: 0; max-width: 100%; }
    :global(.projects-table td.col-actions) { padding-inline: 0; padding-block: var(--spacing); overflow: visible; }
    /* The landing rail is navigation, not the Reader's collapsible panel.
       Put its five destinations in a compact row below the list, while the
       list keeps the remaining workspace height and its own vertical scroll. */
    .workspace { flex-direction: column; overflow-x: hidden; }
    .workspace :global(.sidebar),
    .workspace :global(.sidebar.collapsed) { order: 2; flex: 0 0 auto; width: 100%; border-right: 0; }
    .workspace :global(.sidebar .sidebar-activity) { flex-direction: row; align-items: center; gap: calc(var(--spacing) * 2); min-height: var(--librepaper-activity); padding-inline: calc(var(--spacing) * 2); }
    .workspace :global(.sidebar .activity-sections) { display: flex; flex: 1 1 auto; flex-direction: row; justify-content: space-between; overflow-x: auto; overflow-y: hidden; }
    .workspace :global(.sidebar .activity-bottom) { flex: none; flex-direction: row; margin-top: 0; }

    .projects { width: 100%; max-width: 100%; padding-inline: calc(var(--spacing) * 3); }
    .project-search { width: min(9rem, 32vw); }
    .nav-new { width: 2rem; padding-inline: 0; }
    .nav-new-label { display: none; }

    /* One 44px menu trigger keeps the action column small and the title row
       free of duplicate metadata. */
  }
</style>
