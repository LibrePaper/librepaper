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
  import TagSwatches from "./TagSwatches.svelte";
  import NewProject from "./NewProject.svelte";
  import SettingsDialog from "./settings/SettingsDialog.svelte";
  import Hero from "./Hero.svelte";
  import Toasts from "./Toasts.svelte";
  import Sidebar from "./layout/Sidebar.svelte";
  import CompactPanelMenu from "./layout/CompactPanelMenu.svelte";
  import { barHiddenAfter, watchScrolls, revealOnNavFocus } from "../lib/bar-scroll.js";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import Row from "./layout/Row.svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import { say } from "../lib/toast.svelte.js";
  import { SHELL_HEADERS, get, getPrivate, me as whoami, signInHref } from "../lib/api.js";
  import { day as isoDay, since } from "../lib/dates.js";
  import { PROJECT_TABS, PROJECT_TAB_IDS } from "../lib/panels.js";
  import { tagColour } from "../lib/tag-colours.js";
  import { FORMATS } from "../lib/starter.js";
  import { localProjects } from "../lib/offline-projects.js";
  import { provide } from "../lib/support/providers.js";
  import * as localBridge from "../lib/desktop/client.js";
  import { createPreferences } from "../lib/reader/preferences.svelte.js";
  import { warmReader } from "../lib/warm-reader.js";

  let { initialSettings = "" } = $props();
  const preferences = createPreferences();
  let settingsOpen = $state(untrack(() => initialSettings === "tools"));
  let settingsCategory = $state(untrack(() => initialSettings || "editor"));
  function openSettings(category = "editor") {
    settingsCategory = category;
    settingsOpen = true;
  }

  // The desktop app asks for Settings from its menu and from Ctrl or Cmd+,.
  $effect(() => {
    const open = () => openSettings();
    window.addEventListener("librepaper:open-settings", open);
    return () => window.removeEventListener("librepaper:open-settings", open);
  });
  function focusOnMount(node) {
    node.focus();
  }

  /** @type {{id?: string; provider?: string; can_publish?: boolean; name?: string; handle?: string; publishers?: string}} */
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
  // The tag the list is filtered by, when the place is "tag". Read from the
  // address as the place is, so a link to one tag is a link to it.
  let tagId = $state(new URLSearchParams(location.search).get("tag") || "");
  // Every tag the account has, Favorites included, as the listing last said.
  let tags = $state([]);
  let managing = $state(false);
  let tagToDelete = $state(null);
  // The tag being made: null while its dialog is closed, otherwise the
  // projects it is to be put on. An empty list makes a tag nothing has yet.
  let tagFor = $state(null);
  let tagName = $state("");
  let tagShade = $state("blue");
  let tagError = $state("");
  let sortBy = $state("updated");
  let ascending = $state(false);
  const compactSort = $derived(`${sortBy}:${ascending ? "asc" : "desc"}`);
  // The New project page. It replaces the rail and the list while the address
  // says `?new=<tab>`, so it can be linked to and Back closes it.
  let newTab = $state(newTabFromUrl());
  // The tab last used this visit, which the nav button reopens.
  let lastNewTab = "";
  // Whether this page view was pushed by us, so Cancel can step back instead
  // of leaving a dead entry behind.
  let newPushed = false;
  let newFormat = $state("");
  let newTemplate = $state("");
  // The limits the server publishes; empty until /api/config answers.
  let rules = $state({});
  // The caller's own templates, as the server lists them: projects. One list
  // serves the New project page and the Templates place, so there is one loader.
  /** @type {{slug: string; title: string; source_format: string; url: string}[]} */
  let templateRows = $state([]);
  const takenTitles = $derived(documents.filter((doc) => mine(doc)).map((doc) => doc.title));
  let confirming = $state(false);
  let confirmText = $state("");
  // The trash confirms twice over: once to put a project there, once to empty
  // it. The sentence differs, and so does what the button does, so the dialog
  // carries which of the two it is asking.
  let confirmKind = $state("trash");
  let pendingDeletion = [];
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
  // Below 1024px Owner and Files go, because the four fixed columns would
  // otherwise leave the title one word wide. On a narrow window the date goes
  // too. They are dropped from the list rather than hidden in CSS, because a
  // column that is still in the table still has a width, and the table would
  // keep its room. Nothing can be dragged on a narrow window either, so the
  // title takes whatever the checkbox and the menu leave.
  let narrow = $state(false);
  $effect(() => {
    const query = window.matchMedia("(max-width: 760px)");
    const watch = () => (narrow = query.matches);
    watch();
    query.addEventListener("change", watch);
    return () => query.removeEventListener("change", watch);
  });
  let cramped = $state(false);
  $effect(() => {
    const query = window.matchMedia("(max-width: 1023px)");
    const watch = () => (cramped = query.matches);
    watch();
    query.addEventListener("change", watch);
    return () => query.removeEventListener("change", watch);
  });
  // The phone bar, as in a project. Its search field opens on demand there.
  let barHidden = $state(false);
  let searching = $state(false);
  // The bar steps aside while the list scrolls down on a phone, as in a project.
  $effect(() => watchScrolls("#main", (delta, top, bottom) => { barHidden = narrow ? barHiddenAfter(barHidden, delta, top, bottom) : false; }));
  $effect(() => revealOnNavFocus(() => { barHidden = false; }));
  $effect(() => { narrow; place; barHidden = false; });
  $effect(() => { if (!narrow) searching = false; });

  const columns = $derived.by(() => {
    const trash = place === "trash";
    // Typed loosely on purpose: the list is grown by `push` below, and a
    // literal inferred from its first two entries refuses every later one.
    const list = /** @type {Record<string, any>[]} */ ([
      { key: "mark", label: "", width: 44, resizable: false },
      { key: "title", label: "Project", sortable: true, min: 160, resizable: !narrow },
    ]);
    if (!narrow && !cramped) list.push({ key: "owner", label: "Owner", sortable: true, width: 210, min: 120 });
    if (!trash && !narrow && !cramped) list.push({ key: "files", label: "Files", sortable: true, width: 90, min: 60 });
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
      width: narrow ? 48 : trash ? 88 : 190,
      resizable: false,
      align: "right",
      class: "col-actions",
    });
    return list;
  });

  function placeFromUrl() {
    const params = new URLSearchParams(location.search);
    const asked = params.get("in") || "";
    if (PROJECT_TAB_IDS.includes(asked)) return asked;
    // A tag is a place of its own, named in the address by its id rather than
    // by one of the rail's words.
    return params.get("tag") ? "tag" : "projects";
  }

  // Changing place is a navigation, so it goes in the address bar -- but not
  // in the history, which would make Back walk the rail backwards instead of
  // leaving the page. The rail is where you are, not how you got here.
  function goTo(id) {
    place = id;
    selected = new Set();
    const url = new URL(location.href);
    url.searchParams.delete("tag");
    if (id === "projects") url.searchParams.delete("in");
    else url.searchParams.set("in", id);
    history.replaceState(null, "", url);
    if (id === "trash") void showTrash();
    if (id === "templates") void loadTemplates();
  }

  function goToTag(id) {
    place = "tag";
    tagId = id;
    selected = new Set();
    const url = new URL(location.href);
    url.searchParams.delete("in");
    url.searchParams.set("tag", id);
    history.replaceState(null, "", url);
  }

  // The rail's Tags menu. A tag is a place, so choosing one goes there; New and
  // Manage open their dialogs after the menu has closed, as the row menu does.
  function railTagChosen(value) {
    if (value === "new-tag") setTimeout(() => askNewTag([]), 0);
    else if (value === "manage-tags") setTimeout(() => (managing = true), 0);
    else goToTag(value.slice(4));
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
  // A tag name matches the same way a title does. It is not shown as the
  // reason for the match, because the chips on the row already say it.
  function found(doc, needle) {
    if (!needle) return { matches: true, path: "" };
    if (doc.title.toLowerCase().includes(needle)) return { matches: true, path: "" };
    const path = (paths.get(doc.slug) || []).find((each) => each.toLowerCase().includes(needle)) || "";
    const tagged = tagsOf(doc).some((tag) => tag.name.toLowerCase().includes(needle));
    return { matches: tagged || Boolean(path), path };
  }

  // Ask each project what is in it, once. A failure is silent and leaves that
  // project findable by its title alone, which is all it was before.
  const pathRequests = new Map();
  const PATH_CONCURRENCY = 6;

  function requestPaths(doc) {
    if (pathRequests.has(doc.slug)) return pathRequests.get(doc.slug);
    const pending = get("/api/documents/" + doc.slug + "?files=1")
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
    const wanted = documents.filter((doc) => !paths.has(doc.slug) && !doc.local_copy);
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
    favorites: (doc) => hasTag(doc, "favorites"),
    tag: (doc) => hasTag(doc, tagId),
    trash: () => true,
    templates: () => true,
  };

  // A row's tags. A project from this browser's copies, or a template, has none
  // and so is in no tag. Ids rather than objects, because the row stores ids.
  const hasTag = (doc, id) => (doc.tags || []).includes(id);
  const tagsOf = (doc) => tags.filter((tag) => hasTag(doc, tag.id));
  // The same, without Favorites: the tags a row shows as chips.
  const userTagsOf = (doc) => userTags.filter((tag) => hasTag(doc, tag.id));
  // Favorites first, then the rest by name, as the server lists them.
  const sortTags = (list) => [...list].sort((a, b) => (b.builtin ? 1 : 0) - (a.builtin ? 1 : 0)
    || a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));

  const PLACE_NAMES = Object.fromEntries(PROJECT_TABS.map((tab) => [tab.id, tab.says]));
  // Favorites is a built-in tag and has its own place, so only the others are
  // listed as groupings in the rail and the menus.
  const userTags = $derived(tags.filter((tag) => !tag.builtin));
  const activeTag = $derived(tags.find((tag) => tag.id === tagId));
  const placeName = $derived(place === "tag" ? (activeTag?.name || "Projects") : PLACE_NAMES[place]);

  function nothingHere() {
    if (needle) return "No projects match that search.";
    if (place === "tag") return `Nothing is tagged ${activeTag?.name ?? ""} yet. Add this tag from a project's menu.`;
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
  // The selection as projects, which is what the Tag control acts on. The list
  // it is picked from may be filtered, so this is the whole of what is ticked.
  const selectedDocs = $derived(documents.filter((doc) => selected.has(doc.slug)));
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
  // A star is the Favorites tag, so starring is tagging and nothing else.
  function star(doc) {
    return setTag(doc, "favorites", !hasTag(doc, "favorites"));
  }

  // Requests for one project and tag go out one at a time, in the order they
  // were asked for. The row changes at once; the answers then arrive in order,
  // so what the server last confirmed is known exactly, and once nothing is in
  // flight the row shows that. A failure therefore never leaves a tag the
  // server does not have, nor hides one it does.
  const tagQueues = new Map();

  function putTag(doc, id, on) {
    const now = doc.tags || [];
    doc.tags = on ? [...new Set([...now, id])] : now.filter((each) => each !== id);
  }

  // Resolves to whether the server took this one request.
  function queueTag(doc, id, on) {
    const key = `${doc.slug}:${id}`;
    let queue = tagQueues.get(key);
    if (!queue) {
      queue = { tail: Promise.resolve(), confirmed: hasTag(doc, id), pending: 0 };
      tagQueues.set(key, queue);
    }
    queue.pending++;
    putTag(doc, id, on);
    const result = queue.tail.then(async () => {
      try {
        const response = await fetch(`/api/documents/${doc.slug}/tags/${id}`, {
          method: on ? "PUT" : "DELETE",
          headers: SHELL_HEADERS,
        });
        if (!response.ok) return false;
        queue.confirmed = on;
        return true;
      } catch {
        return false;
      }
    }).then((saved) => {
      queue.pending--;
      if (queue.pending === 0) {
        putTag(doc, id, queue.confirmed);
        tagQueues.delete(key);
      }
      return saved;
    });
    queue.tail = result;
    return result;
  }

  // Putting a tag on a project or taking it off is the same request whatever
  // the tag is, the star included.
  async function setTag(doc, id, on) {
    if (await queueTag(doc, id, on)) return;
    if (id === "favorites") say("That star could not be saved.", { kind: "problem", id: "landing:favorite" });
    else say("That tag could not be saved.", { kind: "problem", id: `landing:tag:${doc.slug}` });
  }

  // The same change to several projects. Each goes through its own queue, and
  // the ones the server refuses are counted rather than quietly left out.
  async function setTagOnMany(docs, id, on) {
    const changed = docs.filter((doc) => hasTag(doc, id) !== on);
    const saved = await Promise.all(changed.map((doc) => queueTag(doc, id, on)));
    const failed = saved.filter((each) => !each).length;
    if (failed) {
      say(`${failed} project${failed === 1 ? "" : "s"} could not be tagged.`, { kind: "problem", id: "landing:tag-many" });
    }
  }

  // The two things the rail's tag list offers beneath the tags.
  function tagAction(action) {
    if (action === "new") askNewTag([]);
    else managing = true;
  }

  // What a tag menu asks for. A tag that every project has is taken off all of
  // them; otherwise it is put on every one, so the check mark says what the
  // next press does. A menu closes before a dialog opens, hence the timeout.
  function tagChosen(value, docs) {
    if (value === "new-tag") {
      setTimeout(() => askNewTag(docs), 0);
      return;
    }
    const id = value.slice(4);
    if (docs.length === 1) return setTag(docs[0], id, !hasTag(docs[0], id));
    const allHave = docs.every((doc) => hasTag(doc, id));
    return setTagOnMany(docs, id, !allHave);
  }

  /* ---------------------------------------------------------------- tags */

  function askNewTag(docs) {
    tagFor = docs;
    tagName = "";
    tagShade = "blue";
    tagError = "";
  }

  // Making a tag puts it on the projects it was asked from, so the one step
  // from a menu leaves the project tagged, which is what it was for.
  async function createTag(event) {
    event.preventDefault();
    const name = tagName.trim();
    const docs = tagFor;
    if (!docs) return;
    if (!name) {
      tagError = "Give the tag a name.";
      return;
    }
    try {
      const response = await fetch("/api/tags", {
        method: "POST",
        headers: { ...SHELL_HEADERS, "Content-Type": "application/json" },
        body: JSON.stringify({ name, colour: tagShade }),
      });
      if (!response.ok) {
        tagError = (await response.json().catch(() => ({}))).error || "That tag could not be created.";
        return;
      }
      const { tag } = await response.json();
      tags = sortTags([...tags, tag]);
      tagFor = null;
      if (docs.length) await setTagOnMany(docs, tag.id, true);
    } catch (error) {
      tagError = error?.message || "That tag could not be created.";
    }
  }

  // Edits to one tag go out one at a time, like tag requests on a project:
  // the name or colour changes at once, and once nothing is in flight the tag
  // shows what the server last confirmed. A colour picked and picked back
  // before the first save lands is two saves, not one lost click.
  const tagEdits = new Map();

  function putTagField(id, field, value) {
    tags = sortTags(tags.map((each) => (each.id === id ? { ...each, [field]: value } : each)));
  }

  function editTag(id, field, value, failure, problemId) {
    const current = tags.find((each) => each.id === id);
    if (!current || current[field] === value) return;
    let queue = tagEdits.get(id);
    if (!queue) {
      queue = { tail: Promise.resolve(), confirmed: { name: current.name, colour: current.colour }, pending: 0 };
      tagEdits.set(id, queue);
    }
    queue.pending++;
    putTagField(id, field, value);
    queue.tail = queue.tail.then(async () => {
      try {
        const response = await fetch(`/api/tags/${id}`, {
          method: "PATCH",
          headers: { ...SHELL_HEADERS, "Content-Type": "application/json" },
          body: JSON.stringify({ [field]: value }),
        });
        if (!response.ok) return (await response.json().catch(() => ({}))).error || failure;
        const { tag: saved } = await response.json();
        queue.confirmed[field] = saved[field];
        return "";
      } catch {
        return failure;
      }
    }).then((problem) => {
      queue.pending--;
      if (queue.pending === 0) {
        putTagField(id, "name", queue.confirmed.name);
        putTagField(id, "colour", queue.confirmed.colour);
        tagEdits.delete(id);
      }
      if (problem) say(problem, { kind: "problem", id: problemId });
    });
  }

  function renameTag(tag, input) {
    const name = input.value.trim();
    if (!name) {
      input.value = tags.find((each) => each.id === tag.id)?.name ?? tag.name;
      say("Give the tag a name.", { kind: "problem", id: "landing:tag-rename" });
      return;
    }
    editTag(tag.id, "name", name, "That tag could not be renamed.", "landing:tag-rename");
  }

  function recolourTag(tag, colour) {
    editTag(tag.id, "colour", colour, "That colour could not be saved.", "landing:tag-colour");
  }

  // Deleting a tag takes it off every project and leaves the projects
  // themselves alone, which the confirmation says before anybody presses it.
  async function reallyDeleteTag() {
    const tag = tagToDelete;
    tagToDelete = null;
    if (!tag) return;
    try {
      const response = await fetch(`/api/tags/${tag.id}`, { method: "DELETE", headers: SHELL_HEADERS });
      if (!response.ok) throw new Error((await response.json().catch(() => ({}))).error || "That tag could not be deleted.");
      tags = tags.filter((each) => each.id !== tag.id);
      for (const doc of documents) {
        if (hasTag(doc, tag.id)) doc.tags = doc.tags.filter((id) => id !== tag.id);
      }
      if (place === "tag" && tagId === tag.id) goTo("projects");
      say(`${tag.name} was deleted. Its projects are still here.`, { id: "landing:tag-deleted" });
    } catch (error) {
      say(error?.message || "That tag could not be deleted.", { kind: "problem", id: "landing:tag-delete" });
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
      let latestTags = [];
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
        // Every page carries the whole tag list, so the last one read is the
        // list, and there is nothing to merge.
        if (Array.isArray(page.tags)) latestTags = page.tags;
        cursor = page.next_cursor;
      } while (cursor?.after_updated && cursor?.after_slug);

      documents = [...bySlug.values()];
      tags = sortTags(latestTags);
      // A tag in the address that the account does not have is not a place,
      // so the page shows Projects instead of an empty list under a name that
      // means nothing.
      if (place === "tag" && !activeTag) goTo("projects");
      // The counts arrive with the listing now. They used to be a request per
      // project -- forty projects meant forty-one requests to open this page
      // -- because neither number was in the catalogue. Both are now.
    } catch (error) {
      if (navigator.onLine !== false) say(error?.message || "The project list could not be refreshed, so what is shown here may be out of date.", { kind: "problem", id: "landing:refresh" });
      // A failed listing still offers the copies kept in this browser.
      await showLocalProjects();
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

  async function showLocalProjects() {
    try {
      const local = await localProjects();
      if (local.length > 0) void warmReader();
      const bySlug = new Map(documents.map((document_) => [document_.slug, document_]));
      for (const record of local) {
        if (bySlug.has(record.identity.slug)) continue;
        bySlug.set(record.identity.slug, {
          ...record.document,
          updated_at: record.preparedAt,
          local_copy: true,
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
    if (newTab || place === "templates") await loadTemplates();
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
      else if (action === "new-tag" || action.startsWith("tag:")) tagChosen(action, [doc]);
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

  // Your own templates, for the New project page and the Templates place alike.
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
    } catch {
      // Signed out, or offline: the built-in templates are all there is.
      templateRows = [];
    }
  }

  function newTabFromUrl() {
    const asked = new URLSearchParams(location.search).get("new") || "";
    return ["blank", "template", "upload", "link"].includes(asked) ? asked : "";
  }

  // Opening the page is a navigation that Back undoes, so it is pushed. The
  // other parameters of the address stay as they are.
  function openNew(tab = lastNewTab || "blank", { format = "", template = "" } = {}) {
    newFormat = format;
    newTemplate = template;
    const url = new URL(location.href);
    url.searchParams.set("new", tab);
    history.pushState(null, "", url);
    newPushed = true;
    newTab = tab;
    void loadTemplates();
  }

  // Switching tabs is not a step in the history, only a note of where you are.
  function newTabChanged(tab) {
    lastNewTab = tab;
    const url = new URL(location.href);
    url.searchParams.set("new", tab);
    history.replaceState(null, "", url);
  }

  function closeNew() {
    if (newPushed) {
      newPushed = false;
      history.back();
      return;
    }
    const url = new URL(location.href);
    url.searchParams.delete("new");
    history.replaceState(null, "", url);
    newTab = "";
  }

  // From a row in the Templates place: the page opens on that template.
  function askNameFrom(doc) {
    openNew("template", { template: doc.slug });
  }

  function askDeleteTemplate(template) {
    askDelete({ slug: template.slug, title: template.name });
  }

  // The suggested name arrives selected, so the first keystroke replaces it.
  // autofocus puts the caret in the box; this is what makes the box a
  // suggestion rather than something to delete first.
  $effect(() => {
    if (copying && copyInput) copyInput.select();
  });

  // Back closes the New project page and Forward brings it back.
  $effect(() => {
    const reread = () => {
      newTab = newTabFromUrl();
      newPushed = Boolean(newTab);
      if (newTab) void loadTemplates();
    };
    window.addEventListener("popstate", reread);
    return () => window.removeEventListener("popstate", reread);
  });

  $effect(() => provide("account", () => ({ id: me.id, signedIn: Boolean(me.provider) })));

  $effect(() => {
    localBridge.configure({ project: null, origin: location.origin });
    get("/api/config").then((answer) => { rules = answer ?? {}; }).catch(() => {});
    // Not the shared me(), which folds a failed request into an empty account:
    // this page needs to tell the two apart in order to say which it is.
    get("/api/me")
      .catch(() => {
        reachedServer = false;
        void showLocalProjects();
        return {};
      })
      .then(async (who) => {
        // The desktop app has no front page: a stranger is sent straight to sign-in.
        if (localBridge.desktop() && reachedServer && !who.name && who.providers?.length) {
          location.replace(signInHref("/"));
          return;
        }
        me = who;
        if (who.provider) void warmReader();
        // Only a publisher's listing carries tags, so anybody else has no tag
        // to show and the address goes back to Projects.
        if (!who.can_publish && place === "tag") goTo("projects");
        if (who.can_publish) {
          if (newTab) void loadTemplates();
          await showList();
          if (place === "trash") await showTrash();
          if (place === "templates") await loadTemplates();
        }
      });
  });
</script>

{#snippet tagItems(isOn)}
  <!-- The account's tags as check items for one project or for the selection.
       A check says the project has the tag, and picking the item changes that.
       Tag items carry no data-project-action: the action list is the list of
       what a row can do to the project, and a tag is not one of those. -->
  <div class="tags-heading" role="presentation">Tags</div>
  {#each userTags as tag (tag.id)}
    <Menu.Item value={"tag:" + tag.id} class="menuitem project-action-item" data-project-tag={tag.id}>
      <span class="menuitem-check" aria-hidden="true">{isOn(tag) ? "✓" : ""}</span>
      <span class="menuitem-swatch" style:background={tagColour(tag.colour)} aria-hidden="true"></span>
      <span class="menuitem-label">{tag.name}</span>
    </Menu.Item>
  {/each}
  <Menu.Item value="new-tag" class="menuitem project-action-item" data-project-tag="new">
    <Icon name="plus" size="1rem" /><span class="menuitem-label">New tag...</span>
  </Menu.Item>
{/snippet}

{#if me.name}
  <!-- The workspace half. The trail says where in the account this is, which
       is the same slot a project's title takes when one is open: going in
       replaces the last segment and nothing else in the bar. -->
  <Nav {me} phoneMenu={narrow} hidden={barHidden && !newTab}>
    {#snippet children()}
      <span class="nav-place">{newTab ? "New project" : placeName}</span>
    {/snippet}
    {#snippet tools()}
      {#if narrow}
        <CompactPanelMenu tabs={PROJECT_TABS} panel={place} open pressed={false} {me} onselect={goTo} onsettings={() => openSettings()}
                          counts={trashed.length ? { trash: trashed.length } : {}}
                          tags={userTags} activeTag={place === "tag" ? tagId : ""} ontag={goToTag} ontagaction={tagAction} />
        {#if searching}
          <input class="input project-search" type="search" placeholder="Search projects"
                 aria-label="Search projects by title or file" bind:value={search} use:focusOnMount
                 onkeydown={(event) => { if (event.key === "Escape") { search = ""; searching = false; } }} />
          <IconButton icon="x" label="Close search" onclick={() => { search = ""; searching = false; }} />
        {:else}
          <IconButton icon="search" label="Search projects" onclick={() => (searching = true)} />
        {/if}
      {:else}
        <input class="input project-search" type="search" placeholder="Search projects"
               aria-label="Search projects by title or file" bind:value={search} />
        <button type="button" class="btn btn-sm nav-action lp-control-tonal-brand" onclick={() => openSettings()}>Settings</button>
      {/if}
      {#if me.can_publish}
        <button type="button" class="btn btn-sm nav-new nav-action lp-control-tonal-brand" aria-label="New project" onclick={() => openNew()}>
          <Icon name="file-plus" size={16} />
          <span class="nav-new-label">New project</span>
        </button>
      {/if}
    {/snippet}
  </Nav>

  {#if newTab}
    <main id="main" tabindex="-1" class="new-page">
      <NewProject {me} {rules} {templateRows} {takenTitles} tab={newTab}
                  initialFormat={newFormat} initialTemplate={newTemplate}
                  ontab={newTabChanged} oncancel={closeNew}
                  ondeletetemplate={askDeleteTemplate} />
    </main>
  {:else}
  <main id="main" tabindex="-1" class="workspace" class:bar-hidden={barHidden}>
    <!-- The rail. Nothing beside it: these are places, and the place you
         choose fills the page rather than a column. `shown.comments` false is
         what the reader's own collapsed column is, so the width here and the
         width there are the same width. A phone has its places in the Menu. -->
    {#if !narrow}
      <Sidebar tabs={PROJECT_TABS} panel={place} shown={{ comments: false }}
               resizable={false} label="Projects" onselectpanel={goTo}
               badges={trashed.length ? { trash: { counts: [{ tone: "warnings", of: trashed.length }] } } : {}}>
        <!-- The tags of this account are one control under the places. A rail
             three rem wide has no room for a list of names, so the names are in
             its menu, which scrolls, and the control shows when a tag is the
             place. -->
        {#snippet places()}
          <Menu onSelect={(chosen) => railTagChosen(chosen.value)}
                positioning={{ placement: "right-start", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}>
            <MenuIconButton icon="tag" label="Tags" pressed={place === "tag"} />
            <ExplorerMenu>
              <div class="tags-menu-items">
                {#each userTags as tag (tag.id)}
                  <Menu.Item value={"tag:" + tag.id} class="menuitem project-action-item" data-rail-tag={tag.id}>
                    <span class="menuitem-check" aria-hidden="true">{place === "tag" && tagId === tag.id ? "✓" : ""}</span>
                    <span class="menuitem-swatch" style:background={tagColour(tag.colour)} aria-hidden="true"></span>
                    <span class="menuitem-label">{tag.name}</span>
                  </Menu.Item>
                {/each}
              </div>
              <div class="tags-menu-divider" role="separator"></div>
              <Menu.Item value="new-tag" class="menuitem project-action-item" data-rail-action="new-tag">
                <Icon name="plus" size="1rem" /><span class="menuitem-label">New tag</span>
              </Menu.Item>
              {#if userTags.length}
                <Menu.Item value="manage-tags" class="menuitem project-action-item" data-rail-action="manage-tags">
                  <Icon name="pencil" size="1rem" /><span class="menuitem-label">Manage tags</span>
                </Menu.Item>
              {/if}
            </ExplorerMenu>
          </Menu>
        {/snippet}
        {#snippet controls()}
          <IconButton icon="settings" label="Settings" tone="plain" onclick={() => openSettings()} />
        {/snippet}
      </Sidebar>
    {/if}

    <section class="projects">
      <div class="place-line">
        <h1 class="place-heading">{placeName}</h1>

        {#if narrow && ["projects", "shared", "favorites", "templates", "tag"].includes(place)}
          <span class="compact-sort">
            <Icon name="arrow-up-down" />
            <select id="project-sort" class="select" value={compactSort} aria-label="Sort" title="Sort"
                    onchange={(event) => chooseCompactSort(event.currentTarget.value)}>
              <option value="updated:desc">Updated: newest first</option>
              <option value="updated:asc">Updated: oldest first</option>
              <option value="title:asc">Title: A to Z</option>
              <option value="title:desc">Title: Z to A</option>
              {#if sortBy === "owner"}
                <option value={`owner:${ascending ? "asc" : "desc"}`}>Owner: {ascending ? "A to Z" : "Z to A"}</option>
              {:else if sortBy === "files"}
                <option value={`files:${ascending ? "asc" : "desc"}`}>Files: {ascending ? "fewest first" : "most first"}</option>
              {/if}
            </select>
          </span>
        {/if}

        {#if narrow && place !== "trash" && !selected.size}<span class="place-count">({shown.length})</span>{/if}
      </div>

      <!-- Where a project comes from, for an account that has not made one
           yet. It is above the list rather than in it: a template is
           something to start from, and a row in this table is something you
           already have. It leaves once there is real work here. -->
      {#if place === "projects" && me.can_publish && ownWork === 0}
        <div class="starters">
          <span class="starters-say">Start something new</span>
          {#each FORMATS as choice}
            <button type="button" class="btn btn-sm lp-control-tonal-brand"
                    onclick={() => openNew("template", { template: "blank", format: choice.id })}>{choice.name}</button>
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
            <!-- One control for every selected project. Its check marks show
                 what the next press does to all of them, as the row menu does
                 for one. -->
            <!-- Templates are not projects, so there is nothing here to tag. -->
            {#if place !== "templates"}
              <Menu onSelect={(chosen) => tagChosen(chosen.value, selectedDocs)}
                    positioning={{ placement: "bottom-start", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}>
                <Menu.Trigger class="btn btn-sm lp-control-tonal-brand">
                  <Icon name="tag" size={16} /> Tag
                </Menu.Trigger>
                <ExplorerMenu>
                  {@render tagItems((tag) => selectedDocs.length > 0 && selectedDocs.every((doc) => hasTag(doc, tag.id)))}
                </ExplorerMenu>
              </Menu>
            {/if}
            <button type="button" class="btn btn-sm lp-tone-error" onclick={deleteSelected}>
              <Icon name="trash" size={16} /> Delete
            </button>
          {/if}
          <IconButton icon="x" label="Clear the selection" onclick={() => (selected = new Set())} />
        </div>
      {:else if place !== "trash"}
        {#if !narrow}
          <div class="toolbar">
            <span class="row-count">{shown.length}{place === "templates" ? (shown.length === 1 ? " template" : " templates") : shown.length === 1 ? " project" : " projects"}</span>
          </div>
        {/if}
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
        label={placeName}
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
            {@const chips = place === "trash" ? [] : userTagsOf(doc)}
            {@const chipsShown = chips.slice(0, narrow ? 2 : 4)}
            <!-- The title and its tags share a line when they fit, so a tagged
                 row is no taller than any other. When they do not, the tags
                 move under the title as one group: the title never shrinks to
                 make room, and the tags never take more than one line. -->
            <span class="title-head">
              {#if place === "trash"}
                <span class="title-line">{doc.title}</span>
              {:else}
                <a class="title-line" href="/docs/{doc.slug}">{doc.title}</a>
              {/if}
              {#if chips.length}
                <span class="tag-chips" class:compact={narrow}>
                  {#each chipsShown as tag (tag.id)}
                    <button type="button" class="tag-chip" style="--tag-colour: {tagColour(tag.colour)}" title={tag.name}
                            aria-label="Show projects tagged {tag.name}" onclick={() => goToTag(tag.id)}><span class="tag-chip-name">{tag.name}</span></button>
                  {/each}
                  {#if chips.length > chipsShown.length}
                    <span class="tag-chip tag-more">+{chips.length - chipsShown.length}</span>
                  {/if}
                </span>
              {/if}
            </span>
            <span class="title-note">
              {#if doc.local_copy}<span>Offline</span>{/if}
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
            {doc.files ?? "-"}
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
                      <Icon name="star" size="1rem" filled={hasTag(doc, "favorites")} />
                      <span class="menuitem-label">{hasTag(doc, "favorites") ? "Remove from favorites" : "Add to favorites"}</span>
                    </Menu.Item>
                    {@render tagItems((tag) => hasTag(doc, tag.id))}
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
                <Menu onSelect={(chosen) => tagChosen(chosen.value, [doc])}
                      positioning={{ placement: "bottom-end", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}>
                  <MenuIconButton icon="tag" label="Tags for {doc.title}" projectSlug={doc.slug} />
                  <ExplorerMenu>
                    {@render tagItems((tag) => hasTag(doc, tag.id))}
                  </ExplorerMenu>
                </Menu>
                <IconButton
                  icon="star"
                  tone="plain"
                  size="btn-icon-sm"
                  colour={hasTag(doc, "favorites") ? "lp-text-warning" : "lp-text-muted"}
                  filled={hasTag(doc, "favorites")}
                  pressed={hasTag(doc, "favorites")}
                  label={hasTag(doc, "favorites") ? "Remove from favorites" : "Add to favorites"}
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
  {/if}
{:else}
  <!-- The other page: what this is, and a way in. -->
  <Nav {me}>
    {#snippet tools()}
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => openSettings()}>Settings</button>
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
          {#if documents.length}
            This page could not reach the server. These projects are kept in this browser and open without a connection.
          {:else}
            This page could not reach the server, so it cannot tell whether you
            are signed in. Your projects will appear once the server answers.
          {/if}
        </aside>
        {#if documents.length}
          <section class="card p-4">
            <h2 class="h4">Projects kept in this browser</h2>
            <ul class="list-disc pl-5">
              {#each documents as doc (doc.slug)}
                <li><a href="/docs/{doc.slug}">{doc.title}</a></li>
              {/each}
            </ul>
          </section>
        {/if}
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

<SettingsDialog bind:open={settingsOpen} bind:category={settingsCategory} account={me}
                keys={preferences.state.keys} onkeys={preferences.setKeys}
                completion={preferences.state.completion} oncompletion={preferences.setCompletion}
                brackets={preferences.state.brackets} onbrackets={preferences.setBrackets}
                check={preferences.state.check} oncheck={preferences.setCheck} />

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

<Modal open={Boolean(tagFor)} title="New tag"
       description="Tags are yours. Nobody else sees the tags you put on a project."
       onclose={() => { tagFor = null; tagError = ""; }}
       confirm={{ form: "new-tag", label: "Make tag", oncancel: () => (tagFor = null) }}>
  {#snippet children()}
    <form id="new-tag" onsubmit={createTag}>
      <label class="label">
        <span class="label-text">Name</span>
        <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
        <input class="input" autofocus maxlength="40" bind:value={tagName} />
      </label>
      <div class="label">
        <span class="label-text">Colour</span>
        <TagSwatches value={tagShade} label="Colour" onchange={(key) => (tagShade = key)} />
      </div>
      {#if tagError}<p class="lp-text-error text-sm">{tagError}</p>{/if}
    </form>
  {/snippet}
</Modal>

<Modal open={managing} title="Manage tags" wider
       description="A change to a tag shows on every project that has it. Deleting a tag does not delete any project."
       onclose={() => (managing = false)}
       confirm={{ label: "Done", onclick: () => (managing = false), cancel: false }}>
  {#snippet children()}
    {#if userTags.length}
      <ul class="tag-manage">
        {#each userTags as tag (tag.id)}
          <li class="tag-manage-row">
            <input class="input" aria-label="Name of {tag.name}" maxlength="40" value={tag.name}
                   onchange={(event) => renameTag(tag, event.currentTarget)} />
            <TagSwatches value={tag.colour} label="Colour of {tag.name}" onchange={(key) => recolourTag(tag, key)} />
            <IconButton icon="trash" colour="lp-text-muted lp-hover-error" title="Delete tag"
                        label="Delete {tag.name}" onclick={() => (tagToDelete = tag)} />
          </li>
        {/each}
      </ul>
    {:else}
      <p class="lp-text-secondary text-sm">No tags yet. Use New tag in the rail to make one.</p>
    {/if}
  {/snippet}
</Modal>

<Modal open={Boolean(tagToDelete)} title="Delete this tag?"
       description={tagToDelete ? `${tagToDelete.name} is taken off every project that has it. The projects themselves stay where they are.` : ""}
       onclose={() => (tagToDelete = null)}
       confirm={{ label: "Delete tag", tone: "error", onclick: reallyDeleteTag, oncancel: () => (tagToDelete = null) }}></Modal>

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
  /* The bar decides the height of its own row: the buttons take the height
     of the search field beside them rather than a size of their own, so they
     read as one row of controls however any of them is restyled later. They
     are filled, not outlined, so neither is mistaken for another field. */
  .nav-action { align-self: stretch; }

  .projects { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; overflow-y: auto; padding: calc(var(--spacing) * 6) calc(var(--spacing) * 6) calc(var(--spacing) * 10); gap: calc(var(--spacing) * 4); }
  .place-heading { font-size: var(--text-2xl); font-weight: 600; margin: 0; }
  .place-line { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }

  .starters { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); }
  .new-page { height: calc(100dvh - var(--librepaper-bar)); max-width: none; margin: 0; padding: 0; overflow-y: auto; }
  .starters-say { color: var(--color-text-secondary); font-size: var(--text-sm); margin-right: calc(var(--spacing)); }

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

  .title-line { display: block; font-weight: 500; color: var(--color-text); text-decoration: none; }
  .title-line:hover { text-decoration: underline; }
  /* The second line of a row: who it is shared with, the file a search found,
     when the trash will take it. Empty on most rows, and collapsed when it
     is, so a row with nothing to add is one line tall. */
  .title-note { display: flex; gap: calc(var(--spacing) * 2); font-size: var(--text-xs); color: var(--color-text-secondary); }
  .title-note:empty { display: none; }

  .owner-name { font-size: var(--text-sm); }

  /* The tags sit on the line under the title. A wide row wraps them; a phone
     row keeps them on one line, and each name gives way to an ellipsis rather
     than the line being cut off. Nothing here clips the line itself, so a
     chip is never half drawn. A chip is a button that shows the projects with
     that tag. */
  .title-head { display: flex; flex-wrap: wrap; align-items: center; column-gap: calc(var(--spacing) * 2); row-gap: 2px; min-width: 0; }
  .title-head > .title-line { flex: 0 1 auto; min-width: 0; }
  .tag-chips { display: flex; flex: 0 1 auto; flex-wrap: nowrap; align-items: center; gap: var(--spacing); min-width: 0; max-width: 100%; }
  .tag-chip { display: inline-flex; align-items: center; gap: .3rem; flex: 0 1 auto; min-width: 2.5rem; max-width: 10rem; padding: 0 .45rem; border: 1px solid color-mix(in srgb, var(--tag-colour) 45%, transparent); border-radius: 999px; background: color-mix(in srgb, var(--tag-colour) 16%, transparent); color: var(--color-text); font: inherit; font-size: var(--text-xs); line-height: 1.125rem; margin: 0; appearance: none; white-space: nowrap; cursor: pointer; }
  button.tag-chip:hover { background: color-mix(in srgb, var(--tag-colour) 28%, transparent); }
  button.tag-chip:focus-visible { outline: 2px solid var(--tag-colour); outline-offset: 1px; }
  .tag-chips.compact .tag-chip:not(.tag-more) { max-width: 7rem; }
  .tag-chip-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .tag-chip::before { content: ""; width: .5rem; height: .5rem; flex: none; border-radius: 50%; background: var(--tag-colour); }
  .tag-chip.tag-more { flex: none; border-color: var(--color-border); background: none; color: var(--color-text-secondary); }
  .tag-chip.tag-more::before { display: none; }

  /* The rail's Tags menu. A hundred tags scroll inside their list, and the
     actions under it stay on screen. */
  .tags-menu-items { display: flex; flex-direction: column; max-height: min(60vh, calc(var(--available-height, 80vh) - 7rem)); overflow-y: auto; overscroll-behavior: contain; }
  .tags-menu-divider { height: 1px; margin: calc(var(--spacing) * 1) calc(var(--spacing) * 1.5); background: var(--color-divider); }

  .tags-heading { padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 2) 0; font-size: var(--text-xs); color: var(--color-text-secondary); }
  .menuitem-swatch { width: .75rem; height: .75rem; flex: none; border-radius: 50%; }
  .tag-manage { display: flex; flex-direction: column; gap: calc(var(--spacing) * 4); margin: 0; padding: 0; list-style: none; }
  .tag-manage-row { display: grid; grid-template-columns: minmax(8rem, 1fr) auto auto; align-items: center; gap: calc(var(--spacing) * 3); }
  .trash-actions { display: inline-flex; align-items: center; gap: calc(var(--spacing)); }
  .project-actions { display: inline-flex; align-items: center; gap: calc(var(--spacing)); }
  :global(.project-actions-trigger) { display: inline-flex; width: 2.75rem; height: 2.75rem; align-items: center; justify-content: center; padding: 0; font-size: 1.5rem; line-height: 1; }
  :global(.project-action-item) { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }

  /* The metadata columns a phone has no room for: the listing is read there
     to find a project, and neither how many files it has, who owns it, nor
     when it changed belongs in the compact row. */
  @media (max-width: 760px) {
    .place-heading { font-size: var(--text-lg); }
    .compact-sort { display: inline-grid; position: relative; place-items: center; width: 2rem; height: 2rem; color: var(--color-text-secondary); }
    .compact-sort :global(svg) { width: 1.125rem; height: 1.125rem; }
    .compact-sort select { position: absolute; inset: 0; width: 100%; height: 100%; opacity: 0; cursor: pointer; }
    .place-count { color: var(--color-text-secondary); font-size: var(--text-sm); }
    :global(.projects-table td.col-actions) { padding-inline: 0; padding-block: var(--spacing); overflow: visible; }
    .workspace { flex-direction: column; overflow-x: hidden; }

    .projects { width: 100%; max-width: 100%; padding-inline: calc(var(--spacing) * 3); }
    .project-search { width: min(14rem, 52vw); }
    .nav-new { width: 2.625rem; height: 2.625rem; padding-inline: 0; }
    .nav-new :global(svg) { width: 1.375rem; height: 1.375rem; }
    .nav-new-label { display: none; }

    /* One 44px menu trigger keeps the action column small and the title row
       free of duplicate metadata. */
  }
</style>
