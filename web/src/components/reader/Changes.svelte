<script>
  import { Menu, Switch } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import Icon from "../Icon.svelte";

  let {
    // Ordinary proposals can still be reviewed by hunk. A linked suggestion
    // is one whole decision, including legacy proposals that contain several
    // hunks.
    proposals = [], comments = [], files = [], tracking = false,
    canTrack = true, canReview = true, selected = "", selectedProposal = "", filters = {},
    recoveryRequired = false, onrecover,
    authors = [], sessions = [], ontracking, onfilter = undefined, onselect = undefined,
    onaccept, onreject, onproposalreveal, onproposaldecide, ondiscard,
    onreveal, onproposalpreview,
    identity = "", commentingAs = "Anonymous", canModerate = false,
  } = $props();

  let expanded = $state("");
  let checked = $state(new Set());
  let deciding = $state(new Set());
  let localFilters = $state({ author: "", file: "", session: "" });
  let feedback = $state("");
  // Ticking changes is a mode you enter, not the state the queue sits in. A
  // review is one change at a time; answering several at once is the rarer
  // thing, so it waits behind a menu item and takes the row of verbs with it.
  let selecting = $state(false);
  // A bulk verb aimed at everything pending, held for one confirmation. There
  // is no undoing a decision from here (§5.1), so "accept all" asks once.
  let confirming = $state("");

  const value = (item, ...keys) => keys.map((key) => item?.[key])
    .find((answer) => answer !== undefined && answer !== null && answer !== "") ?? "";
  const idOf = (item) => String(value(item, "id", "proposalId", "proposal_id"));
  const rowId = (item) => item.__kind === "suggestion" ? `suggestion:${idOf(item)}` : idOf(item);
  const pathOf = (item) => value(item, "path", "file", "filePath", "sourcePath") || "Untitled";
  const authorOf = (item) => value(item, "author", "authorName", "creator") || "Unknown author";
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  const authorLabel = (item) => {
    const explicit = value(item, "authorDisplayName", "displayName", "author_name", "creatorName");
    const recorded = authorOf(item);
    const raw = explicit || (identity && recorded === identity && commentingAs ? commentingAs : recorded);
    if (uuid.test(raw)) return "Unknown editor";
    const local = String(raw).split("@")[0];
    const words = local.split(/[._-]+/).filter(Boolean);
    return words.map((word) => word.charAt(0).toUpperCase() + word.slice(1)).join(" ") || "Unknown editor";
  };
  const authorFilterLabel = (author) => authorLabel({ author });
  const createdOf = (item) => value(item, "created_at", "createdAt", "created");
  const timeLabel = (item) => {
    const date = new Date(createdOf(item));
    if (!Number.isFinite(date.getTime())) return "";
    const seconds = Math.round((date.getTime() - Date.now()) / 1000);
    const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
    if (Math.abs(seconds) < 60) return Math.abs(seconds) < 10 ? "just now" : formatter.format(seconds, "second");
    const minutes = Math.round(seconds / 60);
    if (Math.abs(minutes) < 60) return formatter.format(minutes, "minute");
    const hours = Math.round(minutes / 60);
    if (Math.abs(hours) < 24) return formatter.format(hours, "hour");
    return formatter.format(Math.round(hours / 24), "day");
  };
  const sessionOf = (item) => value(item, "session", "sessionId");
  const statusOf = (item) => value(item, "status", "outcome") || (item.resolved ? (item.outcome || "accepted") : "pending");
  const pending = (item) => statusOf(item) === "pending";
  // A row with no offset sorts last within its file, not first. `value`
  // answers "" when it finds nothing and `Number("")` is 0, so the fallback
  // below was unreachable and anything without a position -- a suggestion
  // anchored by its words rather than by an offset -- claimed the top of the
  // file it belonged to.
  const positionOf = (item) => {
    const raw = value(item, "position", "start_offset", "start", "from");
    const n = Number(raw);
    return raw !== "" && Number.isFinite(n) ? n : Number.MAX_SAFE_INTEGER;
  };

  // session.list() is the stable file order. File identity wins over path so
  // renames do not reshuffle an existing review queue.
  const fileOrder = $derived.by(() => {
    const order = new Map();
    files.forEach((file, index) => {
      for (const key of [value(file, "id", "file_id"), file?.path]) if (key !== "" && key !== undefined && !order.has(String(key))) order.set(String(key), index);
    });
    return order;
  });
  const allRows = $derived.by(() => {
    const proposalRows = new Map();
    for (const row of proposals) {
      const id = String(row.proposal || row.id);
      if (!proposalRows.has(id)) proposalRows.set(id, []);
      proposalRows.get(id).push(row);
    }
    const linked = new Set();
    const out = [];
    for (const comment of comments.filter((candidate) => candidate.motivation === "editing")) {
      const id = String(comment.proposal || "");
      const hunks = proposalRows.get(id) || [];
      if (!id || !hunks.length) {
        out.push({
          ...comment,
          __kind: "suggestion",
          // An absent replacement is not an empty replacement. Keep this
          // explicit even when the linked proposal has no hunk rows to use
          // for its display fallback.
          after: comment.proposed == null ? undefined : comment.proposed,
          unavailable: comment.proposed == null,
          status: comment.resolved ? comment.outcome || "accepted" : "pending",
          resolved: Boolean(comment.resolved),
        });
        continue;
      }
      linked.add(id);
      const first = hunks[0];
      out.push({
        ...first, ...comment,
        id: comment.id,
        proposal: id,
        __kind: "suggestion",
        before: comment.original_anchor?.target?.exact ?? comment.exact ?? first.before ?? "",
        // The first hunk cannot stand in for a missing whole-suggestion
        // replacement: legacy proposals may have several unrelated hunks.
        // Empty string is an available replacement that deletes the passage.
        after: comment.proposed == null ? undefined : comment.proposed,
        unavailable: comment.proposed == null,
        unavailableLabel: undefined,
        discardOnly: false,
        status: comment.resolved ? comment.outcome || "accepted" : "pending",
        resolved: Boolean(comment.resolved),
        stale: hunks.some((row) => Boolean(row.stale)),
        contested: hunks.find((row) => row.contested)?.contested || "",
        replies: comment.replies || first.replies,
        reply_total: comment.reply_total ?? first.reply_total,
      });
    }
    for (const [id, rows] of proposalRows) {
      if (linked.has(id)) continue;
      out.push(...rows.map((row) => ({ ...row, __kind: "proposal" })));
    }
    return out;
  });
  const orderOf = (item) => fileOrder.get(String(value(item, "file_id", "fileId") || pathOf(item))) ?? Number.MAX_SAFE_INTEGER;
  const orderedRows = $derived([...allRows].sort((a, b) => orderOf(a) - orderOf(b) || pathOf(a).localeCompare(pathOf(b)) || positionOf(a) - positionOf(b) || String(value(a, "created_at", "created")).localeCompare(String(value(b, "created_at", "created"))) || rowId(a).localeCompare(rowId(b))));
  const activeFilters = $derived({ ...localFilters, ...(filters || {}) });
  const filteredRows = $derived(orderedRows.filter((item) => {
    if (!pending(item)) return false;
    return (!activeFilters.author || authorOf(item) === activeFilters.author) && (!activeFilters.file || pathOf(item) === activeFilters.file) && (!activeFilters.session || sessionOf(item) === activeFilters.session);
  }));
  const pendingRows = $derived(filteredRows.filter(pending));
  const allPending = $derived(allRows.filter(pending).length);
  const selectedRows = $derived(filteredRows.filter((item) => checked.has(rowId(item))));
  const active = $derived(filteredRows.some((item) => rowId(item) === expanded) ? expanded : filteredRows.some((item) => rowId(item) === String(selectedProposal)) ? String(selectedProposal) : filteredRows.some((item) => rowId(item) === `suggestion:${String(selected)}`) ? `suggestion:${String(selected)}` : filteredRows[0] ? rowId(filteredRows[0]) : "");
  const derivedAuthors = $derived(authors.length ? authors : [...new Set(allRows.map(authorOf))]);
  const derivedFiles = $derived(files.length ? files.map((file) => file.path || file.id).filter(Boolean) : [...new Set(allRows.map(pathOf))]);
  /// The queue, with rival changes standing together.
  ///
  /// Two proposals that change the same words are two answers to one question
  /// (SPEC-loro.md §5.3), and the Reader marks them with a shared `contested`
  /// id. Here that becomes one entry holding all of them, placed where the
  /// first of them would have been -- so the reading order the queue already
  /// had is the order a reviewer keeps, and a rival change never arrives
  /// several screens away from the one it competes with.
  ///
  /// A group with one member left -- the rest filtered out, or answered -- is
  /// not a group. There is no choice to present.
  const groupedRows = $derived.by(() => {
    const out = [];
    const seen = new Set();
    for (const item of filteredRows) {
      const group = item.contested || "";
      if (!group) {
        out.push({ kind: "row", key: rowId(item), item });
        continue;
      }
      if (seen.has(group)) continue;
      seen.add(group);
      const members = filteredRows.filter((other) => other.contested === group);
      if (members.length < 2) {
        out.push({ kind: "row", key: rowId(item), item });
        continue;
      }
      out.push({ kind: "contested", key: `contested:${group}`, members });
    }
    return out;
  });
  /// Whether a reading is on offer at all. It needs somewhere to send the
  /// request and at least one proposal to put in it; a queue of nothing but
  /// comment suggestions has no branches to merge.
  const canPreview = $derived(
    Boolean(onproposalpreview) && filteredRows.some((item) => !item.discardOnly && (item.__kind === "proposal" || item.proposal)),
  );
  /// Whether ticking a row leads anywhere: to a reading, or to a decision
  /// about several changes at once. Without one of those the checkbox is a
  /// control that does nothing, so it is not drawn.
  const pickableRows = $derived(pendingRows.filter((item) => !item.discardOnly));
  const canPick = $derived(canPreview || (canReview && pickableRows.length > 1));
  const contestedCount = $derived(
    new Set(filteredRows.filter((item) => item.contested).map((item) => item.contested)).size,
  );

  /// The proposals a preview would apply.
  ///
  /// Ticking a change selects the proposal it belongs to, not the change: a
  /// proposal is a branch and a reading applies it whole. Two changes from one
  /// author's session are one proposal and count once.
  const chosenProposals = $derived([
    ...new Set(
      selectedRows
        .filter((item) => !item.discardOnly)
        .map((item) => String(value(item, "proposal", "proposalId", "proposal_id")))
        .filter(Boolean),
    ),
  ]);

  const rowFor = (id) => filteredRows.find((item) => rowId(item) === id);
  const reviewAllowed = (item) => !item?.discardOnly && (item?.__kind === "suggestion" && !item.proposal ? canModerate : canReview);
  /// Whether a decision is in flight; the bulk verbs are disabled while one
  /// is, so a second click cannot answer for rows already being answered for.
  const busy = $derived(deciding.size > 0);
  /// The changes a bulk decision would answer for: the ticked ones this
  /// caller may review and nothing is stopping. A tick means the whole
  /// proposal to a reading and this one hunk to a decision -- the two verbs
  /// say which they mean, and count what they would touch.
  const chosenChanges = $derived(
    selectedRows.filter((item) => pending(item) && reviewAllowed(item) && (!blocker(item) || item.proposal)),
  );
  const selectedAcceptable = $derived(chosenChanges.filter((item) => !blocker(item) && !item.contested && !item.unavailable));
  const selectedRejectable = $derived(chosenChanges);
  const showExtraFilters = $derived(derivedAuthors.length > 1 || derivedFiles.length > 1);
  /// Everything on show that this caller could actually answer for: what the
  /// two verbs in the menu would touch, and what the count beside them says.
  const answerable = $derived(
    pendingRows.filter((item) => reviewAllowed(item) && (!blocker(item) || item.proposal)),
  );
  const acceptAllRows = $derived(answerable.filter((item) => !blocker(item) && !item.contested && !item.unavailable));
  const rejectAllRows = $derived(answerable);
  /// Whether the filename belongs on the metadata line. One file under review
  /// names itself in every row, which is a word that tells the reviewer
  /// nothing; two or more and it is the only thing placing the change.
  const showPath = $derived(derivedFiles.length > 1);
  const filterLabel = $derived([
    "Pending changes",
    activeFilters.author ? authorFilterLabel(activeFilters.author) : "",
    activeFilters.file || "",
  ].filter(Boolean).join(" · "));
  /// The ••• appears only when it holds something: an empty menu is a button
  /// that opens nothing.
  const hasMenu = $derived(canPick || (canReview && answerable.length > 0));

  $effect(() => {
    const valid = new Set(filteredRows.filter((item) => pending(item) && !item.discardOnly).map(rowId));
    const next = new Set([...checked].filter((id) => valid.has(id)));
    if (next.size !== checked.size) checked = next;
  });
  $effect(() => { if (expanded && !filteredRows.some((item) => rowId(item) === expanded)) expanded = ""; });
  $effect(() => { if (selecting && !canPick) stopSelecting(); });

  /// Why this change cannot be answered, or "" when it can.
  ///
  /// The one case that matters is a hunk whose base has moved: a proposal's
  /// base goes stale by design (§5.1), and the words now at those offsets are
  /// not the words the author offered to change. The server refuses such a
  /// decision anyway; saying so here means the reviewer finds out before
  /// clicking rather than after, and is not invited to agree to something
  /// nobody wrote.
  function blocker(item) {
    if (!item) return "";
    if (item.discardOnly) return item.unavailableLabel || "This proposal has no readable changes.";
    if (item.unavailable) return item.unavailableLabel || "The suggested replacement is unavailable.";
    if (item.stale) return typeof item.stale === "string" ? item.stale : "The text this was written against has changed.";
    if (item.conflict) return typeof item.conflict === "string" ? item.conflict : "Concurrent edits need attention.";
    return "";
  }
  function updateFilter(key, answer) { const next = { ...localFilters, [key]: answer }; localFilters = next; onfilter?.(next); }
  function startSelecting() { selecting = true; confirming = ""; }
  function stopSelecting() { selecting = false; checked = new Set(); }
  function focusRow(id) { requestAnimationFrame(() => document.getElementById(`change-${id}`)?.focus({ preventScroll: true })); }
  function activate(item, { focus = false } = {}) {
    if (!item) return;
    expanded = rowId(item); onselect?.(item);
    if (item.__kind === "suggestion") onreveal?.(item); else onproposalreveal?.(item);
    if (focus) focusRow(rowId(item));
  }
  function reveal(item) { activate(item, { focus: true }); }
  function toggleChecked(item) { const id = rowId(item); const next = new Set(checked); next.has(id) ? next.delete(id) : next.add(id); checked = next; }
  function decisionPromise(item, action) {
    const callback = item.__kind === "suggestion" ? (action === "accept" ? onaccept : onreject) : onproposaldecide;
    if (!callback) return Promise.reject(new Error("Review action is unavailable."));
    // A proposal row is handed back whole: the decision needs the proposal it
    // belongs to and the hunk index within it, and the row id joins the two
    // only so the DOM has something to key on.
    return item.__kind === "suggestion" ? callback(item) : callback(item, action);
  }
  function addBusy(id) { deciding = new Set([...deciding, id]); }
  function removeBusy(id) { deciding = new Set([...deciding].filter((current) => current !== id)); }
  function nextAfter(ids, oldIndex) { return [...ids.slice(oldIndex + 1), ...ids.slice(0, oldIndex).reverse()].map(rowFor).find((item) => item && pending(item)); }

  async function decide(item, action) {
    const id = rowId(item); const why = blocker(item);
    if (!pending(item) || deciding.has(id)) return;
    if (item.discardOnly) { feedback = why; activate(item); return; }
    if (!reviewAllowed(item)) return;
    if (action === "accept" && item.unavailable) { feedback = blocker(item); activate(item); return; }
    if (why && (action === "accept" || !item.proposal)) { feedback = `Cannot ${action} this change: ${why}`; activate(item); return; }
    addBusy(id); feedback = "";
    const ids = pendingRows.map(rowId); const oldIndex = ids.indexOf(id);
    try {
      const operation = decisionPromise(item, action);
      if (operation === undefined || operation === false) throw new Error("Review action is unavailable.");
      await operation;
      const next = nextAfter(ids, oldIndex);
      if (next) activate(next, { focus: true }); else {
        expanded = "";
        requestAnimationFrame(() => {
          const panel = document.querySelector(".changes-panel");
          if (panel instanceof HTMLElement) panel.focus();
        });
      }
    } catch (error) { feedback = error?.message || `Could not ${action} this change.`; focusRow(id); }
    finally { removeBusy(id); }
  }

  /// Answering several changes at once: the ticked ones, or everything
  /// pending when the verb came from the menu rather than from a selection.
  async function bulk(action, rows = selectedRows) {
    if (!canReview) return;
    // Take the rows before sending anything: the ticks are cleared and the
    // queue re-sorts as answers arrive, so this is a list and not a filter
    // that keeps being asked.
    const captured = rows.filter(pending); if (!captured.length) return;
    const mayRejectWithoutMerge = (item) => action === "reject" && Boolean(item.proposal) && !item.discardOnly;
    const excluded = captured.filter((item) => (blocker(item) && !mayRejectWithoutMerge(item)) || !reviewAllowed(item) || (action === "accept" && item.contested));
    const eligible = captured.filter((item) => (!blocker(item) || mayRejectWithoutMerge(item)) && reviewAllowed(item) && (action !== "accept" || !item.contested));
    if (!eligible.length) { feedback = `${excluded.length} change${excluded.length === 1 ? "" : "s"} excluded because it needs attention.`; return; }
    eligible.forEach((item) => addBusy(rowId(item))); stopSelecting(); confirming = "";
    try {
      // Each hunk is decided on its own, because that is what a decision is:
      // it names a proposal and an index within it (§5.2), and the server
      // checks each against the branch it was written for. They go together
      // rather than in turn -- a decision does not move the branch it is
      // about, so one cannot spoil the next -- and every answer is waited for,
      // so the count below is what actually happened and not what was asked.
      const settled = await Promise.allSettled(eligible.map((item) => decisionPromise(item, action)));
      const succeeded = settled.filter((entry) => entry.status === "fulfilled").length;
      const failed = settled.length - succeeded;
      const omitted = excluded.length;
      feedback = failed || omitted ? `${succeeded} ${action}ed; ${failed} failed; ${omitted} excluded.` : `${succeeded} change${succeeded === 1 ? "" : "s"} ${action}ed.`;
    } catch (error) { feedback = error?.message || `Could not ${action} the selected changes.`; }
    finally { eligible.forEach((item) => removeBusy(rowId(item))); }
  }
  // The menu is where the infrequent things live: the two verbs that answer
  // for the whole queue, the reading of a chosen set of proposals, and the
  // mode that lets a reviewer choose that set. None of them belongs in the
  // panel itself, where they would stand above every review as controls
  // nobody presses.
  //
  // There is no undoing a decision here. A decision is recorded and broadcast
  // at once (§5.1), and what it eventually does to the document is a merge
  // the server makes when the proposal resolves -- so taking one back is an
  // ordinary edit to the paper, not a review action.
  function chooseMenuItem(value) {
    if (value === "select") startSelecting();
    else if (value === "accept-all") confirming = "accept";
    else if (value === "reject-all") confirming = "reject";
  }
  function chooseFilter(value) {
    const [key, ...rest] = String(value).split(":");
    if (key === "author" || key === "file") updateFilter(key, rest.join(":"));
  }
  function move(offset) { const index = filteredRows.findIndex((item) => rowId(item) === active); const item = filteredRows[index + offset] || filteredRows[index]; if (item) activate(item, { focus: true }); }
  // A stale hunk has one way forward: look at the passage as it stands now.
  // The author has to offer the change again against the text that is there,
  // which is theirs to do and not the reviewer's.
  function resolve(item) {
    // Reveal focuses the editor, so take focus back to the row button via focusRow in activate.
    activate(item, { focus: true });
    feedback = item.discardOnly
      ? item.unavailableLabel || "This proposal has no readable changes."
      : item.unavailable
      ? "The suggested replacement is unavailable. Reject or discard this change."
      : "This change was written against text that has since moved. Ask its author to offer it again.";
  }
  function keydown(event) {
    if (event.defaultPrevented || event.isComposing || event.target.closest("input,select,textarea,[contenteditable=true],details,[data-scope='menu']")) return;
    const item = rowFor(active); const key = event.key.toLowerCase();
    if (event.key === "Escape" && (selecting || confirming)) { event.preventDefault(); stopSelecting(); confirming = ""; return; }
    if (event.key === "ArrowDown" || key === "j") { event.preventDefault(); move(1); }
    else if (event.key === "ArrowUp" || key === "k") { event.preventDefault(); move(-1); }
    else if (item && key === "a" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "accept"); }
    else if (item && key === "r" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "reject"); }
  }
  function shortDiff(item) {
    const before = value(item, "before", "oldText", "deleted", "exact"); const after = value(item, "after", "newText", "inserted", "proposed");
    const result = item.__kind === "suggestion" ? `Proposal: ${before || "∅"} → ${after || "∅"}` : item.kind === "delete" || item.kind === "deletion" || (before && !after) ? `− ${before}` : item.kind === "insert" || item.kind === "insertion" || (!before && after) ? `+ ${after}` : before || after ? `${before || "∅"} → ${after || "∅"}` : value(item, "summary", "title") || "Text changed";
    return result.length > 180 ? `${result.slice(0, 177)}…` : result;
  }
  function diffParts(item) {
    const before = value(item, "before", "oldText", "deleted", "exact");
    const after = value(item, "after", "newText", "inserted", "proposed");
    return { before, after };
  }
</script>

<!-- A focusable review context keeps shortcuts out of source and discussion fields. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="panel changes-panel" role="application" tabindex="0" onkeydown={keydown} aria-label="Changes review" aria-keyshortcuts="ArrowDown ArrowUp J K A R">
  <!-- Two rows and nothing else above the queue: what this is, whether
       subsequent edits are tracked, and which changes are on show. Everything
       a reviewer does rarely is behind the ••• beside them, so the first
       reviewable change is a finger's width from the top of the panel rather
       than a screen down it. -->
  <header class="changes-header">
    <div class="changes-title-row">
      <h2 class="panel-section-title">Changes</h2>
      <div class="title-controls">
        <!-- A real switch rather than a checkbox with a switch painted over it:
             the state is the control's own, so the name no longer has to carry
             "On" or "Off" for a screen reader to read it out. -->
        <Switch class="tracking-toggle" checked={tracking} disabled={!canTrack}
          title={canTrack ? "Track subsequent edits in this document" : "Editing is unavailable"}
          onCheckedChange={({ checked }) => ontracking?.(checked)}>
          <Switch.Label>Track changes</Switch.Label>
          <Switch.Control class="switch"><Switch.Thumb class="switch-thumb" /></Switch.Control>
          <Switch.HiddenInput />
        </Switch>
        {#if hasMenu}<Menu onSelect={(chosen) => chooseMenuItem(chosen.value)}>
          <!-- The button is authored here rather than handed a `class`: a class
               arriving as a prop carries no scope hash, so the rules below would
               have to be global to paint at all. -->
          <Menu.Trigger>
            {#snippet element(attributes)}
              <button {...attributes} class="changes-menu" aria-label="More change options">•••</button>
            {/snippet}
          </Menu.Trigger>
          <ExplorerMenu>
            {#if canPick}<Menu.Item value="select" class="menuitem">Select multiple</Menu.Item>{/if}
            {#if canReview && answerable.length}
              {#if acceptAllRows.length}<Menu.Item value="accept-all" class="menuitem">Accept all pending changes</Menu.Item>{/if}
              {#if rejectAllRows.length}<Menu.Item value="reject-all" class="menuitem">Reject all pending changes</Menu.Item>{/if}
            {/if}
          </ExplorerMenu>
        </Menu>{/if}
      </div>
    </div>
    <div class="changes-meta" aria-live="polite">{allPending} pending{pendingRows.length !== allPending ? ` · ${pendingRows.length} shown` : ""}{contestedCount ? ` · ${contestedCount} contested` : ""}</div>
    {#if recoveryRequired}<div class="tracking-recovery" role="alert"><span>A tracked draft needs attention. Its text is preserved here for manual recovery.</span><button type="button" class="empty-link" onclick={() => onrecover?.()}>Download preserved text</button></div>{/if}
  </header>
  <!-- One control, only visible when there are multiple authors or files to filter by. -->
  {#if showExtraFilters}<div class="filter-bar">
    <Menu onSelect={(chosen) => chooseFilter(chosen.value)}>
      <Menu.Trigger>
        {#snippet element(attributes)}
          <button {...attributes} type="button" class="filter-trigger" aria-label="Which changes to show"><Icon name="list-filter" size="14px" /><span class="filter-label">{filterLabel}</span><Icon name="chevron-down" size="12px" /></button>
        {/snippet}
      </Menu.Trigger>
      <ExplorerMenu>
        {#if derivedAuthors.length > 1}
          <div class="changes-menu-label">Author</div>
          <Menu.Item value="author:" class="menuitem"><span class="menuitem-check">{activeFilters.author ? "" : "✓"}</span>Anyone</Menu.Item>
          {#each derivedAuthors as author}
            <Menu.Item value={`author:${author}`} class="menuitem"><span class="menuitem-check">{activeFilters.author === author ? "✓" : ""}</span>{authorFilterLabel(author)}</Menu.Item>
          {/each}
        {/if}
        {#if derivedFiles.length > 1}
          {#if derivedAuthors.length > 1}<hr class="hr my-1" />{/if}
          <div class="changes-menu-label">File</div>
          <Menu.Item value="file:" class="menuitem"><span class="menuitem-check">{activeFilters.file ? "" : "✓"}</span>Every file</Menu.Item>
          {#each derivedFiles as file}
            <Menu.Item value={`file:${file}`} class="menuitem"><span class="menuitem-check">{activeFilters.file === file ? "✓" : ""}</span>{file}</Menu.Item>
          {/each}
        {/if}
      </ExplorerMenu>
    </Menu>
  </div>{/if}
  <!-- Ticking rows, and the verbs that go with it, for as long as the mode
       lasts. Reading a chosen set of proposals as prose lives here too: it is
       a reading and not a version -- nothing is decided by opening it -- and
       it needs a chosen set, which is exactly what this mode makes. -->
  {#if selecting}<div class="select-bar" role="group" aria-label="Selected changes">
    <span class="select-count">{checked.size} selected</span>
    {#if canPreview}<button type="button" class="bar-action" disabled={!chosenProposals.length} title={chosenProposals.length ? "Read the paper with these proposals applied" : "Tick changes to read the paper as they would leave it"} onclick={() => onproposalpreview?.([...chosenProposals])}>Read</button>{/if}
    {#if canReview}<button type="button" class="bar-action reject" disabled={!selectedRejectable.length || busy} onclick={() => bulk("reject")}>Reject</button>
    <button type="button" class="bar-action accept" disabled={!selectedAcceptable.length || busy} onclick={() => bulk("accept")}>Accept</button>{/if}
    <button type="button" class="bar-action" onclick={stopSelecting}>Cancel</button>
  </div>{/if}
  {#if confirming}<div class="select-bar confirm-bar" role="group" aria-label="Confirm a decision for every pending change">
    <span class="select-count">{confirming === "accept" ? "Accept" : "Reject"} all {confirming === "accept" ? acceptAllRows.length : rejectAllRows.length}?</span>
    <button type="button" class="bar-action {confirming}" disabled={busy} onclick={() => bulk(confirming, confirming === "accept" ? acceptAllRows : rejectAllRows)}>{confirming === "accept" ? "Accept all" : "Reject all"}</button>
    <button type="button" class="bar-action" onclick={() => confirming = ""}>Cancel</button>
  </div>{/if}
  {#if feedback}<p class="panel-status" role="status" aria-live="polite">{feedback}</p>{/if}
  {#if !filteredRows.length}<div class="changes-empty" role="status">
    <p class="panel-muted">{activeFilters.author || activeFilters.file ? "No changes match these filters." : "No pending changes."}</p>
  </div>{/if}
  <!-- One entry per decision, except where several proposals answer the same
       question: those stand together so the choice between them is visible
       rather than spread down the queue (SPEC-loro.md §5.3). -->
  {#snippet changeBody(item)}
    {@const id = rowId(item)}{@const isOpen = active === id}{@const why = blocker(item)}{@const parts = diffParts(item)}
    <!-- The change first and whoever wrote it after it, smaller: the proposed
         words are the thing being reviewed, and a username set above them in
         bold reads as though the author were. -->
    <div class="row-head">{#if selecting && !item.discardOnly}<input type="checkbox" class="row-pick" checked={checked.has(id)} aria-label={`Tick ${authorLabel(item)}'s change`} onclick={(event) => event.stopPropagation()} onchange={() => toggleChecked(item)} />{/if}<button id={`change-${id}`} type="button" class="row-main" aria-current={isOpen ? "true" : undefined} onclick={() => reveal(item)}><span class="row-diff">{#if item.unavailable}<span class="row-summary">{item.unavailableLabel || "Suggested replacement unavailable"}</span>{:else}{#if parts.before}<span class="deletion">− {parts.before}</span>{/if}{#if parts.after}<span class="insertion">+ {parts.after}</span>{/if}{#if !parts.before && !parts.after}<span class="row-summary">{shortDiff(item)}</span>{/if}{/if}</span><span class="row-meta"><span class="row-author">{authorLabel(item)}</span>{#if showPath}<span class="row-context">{pathOf(item)}</span>{/if}{#if why}<span class="row-warning">needs attention</span>{/if}</span></button></div>
    {#if isOpen && why}<div id={`change-detail-${id}`} class="change-detail" role="region" aria-label={`Details for change in ${pathOf(item)}`}>
      <div class="conflict" role="alert"><p>{why}</p>{#if !item.discardOnly}<button type="button" class="empty-link" onclick={() => resolve(item)}>Show the passage</button>{/if}{#if item.proposal && ondiscard}<button type="button" class="empty-link" disabled={deciding.has(id)} onclick={async () => { addBusy(id); try { await ondiscard(item); feedback = "Change discarded."; } catch (error) { feedback = error?.message || "Could not discard this change."; } finally { removeBusy(id); } }}>Discard change</button>{/if}</div>
    </div>{/if}
    <!-- Only the change being looked at offers an answer. Twenty rows of
         buttons is twenty invitations to answer something nobody has read. -->
    {#if isOpen && !selecting && pending(item)}<div class="row-actions"><button type="button" class="row-action reject" aria-label={`Reject change in ${pathOf(item)}`} title={why && item.proposal ? "Reject this proposal without applying its text" : why || "Reject this change"} disabled={!reviewAllowed(item) || (Boolean(why) && !item.proposal) || deciding.has(id)} onclick={() => void decide(item, "reject")}>{deciding.has(id) ? "Rejecting…" : "Reject"}</button><button type="button" class="row-action accept" aria-label={`Accept change in ${pathOf(item)}`} title={why || "Accept this change"} disabled={!reviewAllowed(item) || Boolean(why) || deciding.has(id)} onclick={() => void decide(item, "accept")}>{deciding.has(id) ? "Accepting…" : "Accept"}</button></div>{/if}
    {#if isOpen && item.__kind === "suggestion" && item.body}<div class="discussion"><p>{item.body}</p><button type="button" class="empty-link" onclick={(event) => { event.stopPropagation(); onreveal?.(item); }}>Show suggestion note</button></div>{/if}
    {#if isOpen && item.replies?.length}<details class="discussion"><summary>Discussion ({item.replies.length})</summary><ul>{#each item.replies as reply (reply.id)}<li><strong>{reply.creator || "Author"}:</strong> {reply.body}</li>{/each}</ul></details>{/if}
  {/snippet}
  <div class="changes-list" role="list" aria-label="Revision queue">
    {#each groupedRows as entry (entry.key)}
      {#if entry.kind === "contested"}
        <article class="change-row contested" role="listitem" aria-label={`${entry.members.length} rival changes to the same text in ${pathOf(entry.members[0])}`}>
          <p class="contested-head"><span class="contested-badge">Contested</span> {entry.members.length} proposals change the same text{showPath ? ` in ${pathOf(entry.members[0])}` : ""}. Accepting one leaves the rest still to answer.</p>
          {#each entry.members as item (rowId(item))}
            <div class="contested-option" class:active={active === rowId(item)}>{@render changeBody(item)}</div>
          {/each}
        </article>
      {:else}
        {@const item = entry.item}
        <article class="change-row" class:active={active === rowId(item)} class:suggestion={item.__kind === "suggestion"} class:blocked={Boolean(blocker(item))} role="listitem">
          {@render changeBody(item)}
        </article>
      {/if}
    {/each}
  </div>
  <!-- The shortcuts, said once and quietly, rather than a panel of
       documentation standing where changes could be. -->
  {#if filteredRows.length}<footer class="changes-hint">J/K next · A accept · R reject</footer>{/if}
</div>

<style>
  .changes-panel { display: flex; flex-direction: column; min-height: 0; outline: none; height: 100%; }
  /* Two lines of header, and the filter under it: about fifty pixels before
     the first change, where the settings area this replaced took several
     hundred. Both stay put while the queue scrolls beneath them. */
  .changes-header { position: sticky; top: 0; z-index: 2; flex: 0 0 auto; padding: .4rem var(--spacing) .25rem; background: var(--color-sidebar); }
  .changes-title-row { display: flex; flex-wrap: wrap; align-items: center; gap: .25rem .5rem; }
  h2 { margin: 0; flex: 1 1 auto; min-width: 0; }
  .title-controls { display: flex; align-items: center; gap: .15rem; margin-left: auto; min-width: 0; }
  /* The quiet tier. Only -400 is grey in the surface ramp -- -500 and -600
     are the theme's ink -- so these read as body text rather than as meta.
     `--panel-muted` is what every other panel means by quiet. */
  .changes-meta, .row-meta, .changes-hint { color: var(--panel-muted); font-size: var(--panel-meta-size); }
  .changes-meta { margin-top: .1rem; }
  .tracking-recovery { display: flex; align-items: center; justify-content: space-between; gap: .6rem; margin-top: .45rem; padding: .45rem; border-left: 2px solid var(--color-warning-solid); background: var(--color-raised); color: var(--color-text); font-size: var(--panel-meta-size); }
  /* The track and thumb are `.switch` in librepaper.css, worn here and in the
     settings dialog alike; this is only where the words sit beside them. */
  .changes-panel :global(.tracking-toggle) { display: flex; align-items: center; gap: .35rem; flex: 0 0 auto; font-size: var(--panel-meta-size); white-space: nowrap; }
  .filter-bar { flex: 0 0 auto; padding: 0 var(--spacing) .3rem; background: var(--color-sidebar); border-bottom: 1px solid var(--color-divider); }
  .filter-trigger { display: flex; align-items: center; gap: .3rem; max-width: 100%; padding: .15rem .45rem; border: 1px solid var(--color-divider); border-radius: .25rem; background: none; color: inherit; font-size: var(--panel-font-size); line-height: var(--panel-line-height); text-align: left; cursor: pointer; }
  .filter-trigger:hover, .filter-trigger[data-state="open"] { background: var(--color-divider); }
  .filter-trigger :global(svg) { color: var(--panel-muted); flex: 0 0 auto; }
  .filter-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .changes-menu { flex: 0 0 auto; padding: .1rem .35rem; border: 0; border-radius: .25rem; background: none; line-height: 1; cursor: pointer; }
  .changes-menu:hover, .changes-menu[data-state="open"] { background: var(--color-divider); }
  .changes-menu-label { padding: .35rem .65rem .2rem; color: var(--panel-muted); font-size: var(--panel-meta-size); font-weight: 600; text-transform: uppercase; letter-spacing: .06em; }
  /* The contextual bar: only while a selection or a confirmation is open, and
     gone the moment it is answered or dismissed. */
  .select-bar { flex: 0 0 auto; display: flex; flex-wrap: wrap; align-items: center; gap: .3rem; padding: .35rem var(--spacing); border-bottom: 1px solid var(--color-divider); background: color-mix(in srgb, var(--color-brand) 6%, var(--color-sidebar)); font-size: var(--panel-meta-size); }
  .select-count { flex: 1 1 auto; color: var(--panel-muted); }
  .bar-action, .row-action, .empty-link { border: 0; background: none; padding: .2rem .3rem; border-radius: .25rem; font-size: var(--panel-meta-size); cursor: pointer; }
  .row-action { padding-inline: .4rem; }
  .bar-action:hover:not(:disabled), .row-action:hover:not(:disabled), .empty-link:hover { background: var(--color-divider); }
  .bar-action:disabled, .row-action:disabled { opacity: .4; cursor: default; }
  .bar-action.accept, .row-action.accept { color: var(--color-success-text); }
  .bar-action.reject, .row-action.reject { color: var(--color-error-text); }
  .empty-link { padding-inline: 0; color: var(--color-link); text-decoration: underline; }
  .changes-list { flex: 1 1 auto; min-height: 0; overflow: auto; overscroll-behavior: contain; }
  /* A row is the change and a line saying whose it is. No card, no padding
     around a card, no border but the hairline between one change and the
     next -- so a dozen of them fit where three used to. */
  .change-row { position: relative; content-visibility: auto; contain-intrinsic-size: 0 3.4rem; border-bottom: 1px solid var(--color-divider); padding: .5rem var(--spacing) .5rem calc(var(--spacing) - 3px); border-left: 3px solid transparent; }
  /* One mark for the selected change, not three: a rule down its left edge. */
  .change-row.active, .contested-option.active { border-left-color: var(--color-brand); }
  .change-row.blocked { border-left-color: var(--color-warning-solid); }
  .change-row.contested { border-left-color: var(--color-warning-solid); }
  .contested-head { margin: 0 0 .4rem; font-size: var(--panel-meta-size); color: var(--panel-muted); }
  .contested-badge { display: inline-block; padding: 0 .3rem; border-radius: .2rem; background: var(--color-warning-solid); color: var(--color-warning-on-solid); font-size: var(--panel-meta-size); text-transform: uppercase; letter-spacing: .04em; }
  .contested-option { padding-left: .4rem; border-left: 3px solid transparent; }
  .contested-option + .contested-option { border-top: 1px dashed var(--color-divider); padding-top: .4rem; margin-top: .4rem; }
  .row-head { display: flex; align-items: flex-start; gap: .4rem; }
  .row-pick { flex: none; margin-top: .25rem; }
  .row-main { min-width: 0; flex: 1; text-align: left; background: none; border: 0; padding: 0; cursor: pointer; }
  .row-diff, .row-meta { display: block; }
  /* The diff quotes the source, so it wears the source's monospace at the
     panel's own size rather than a hand-set one. */
  .row-diff { font-family: var(--font-editor); font-size: var(--panel-font-size); line-height: var(--panel-line-height); overflow-wrap: anywhere; }
  .row-diff > span { display: block; }
  /* A change nobody is looking at shows its first couple of lines; the one
     being reviewed shows all of it. */
  .change-row:not(.active) .row-diff > span, .contested-option:not(.active) .row-diff > span { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2; overflow: hidden; }
  .insertion { color: var(--color-success-text); }
  .deletion { color: var(--color-error-text); text-decoration: line-through; text-decoration-color: color-mix(in srgb, currentColor 55%, transparent); }
  .row-summary { color: var(--color-text-secondary); }
  /* One metadata line, whatever it ends up carrying: the parts are written
     without their separators and the line puts them between. */
  .row-meta { margin-top: .15rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row-meta > span + span::before { content: " · "; }
  .row-warning { color: var(--color-warning-text); }
  .row-actions { display: flex; justify-content: flex-end; gap: .5rem; margin-top: .3rem; }
  .change-detail { margin-top: .35rem; max-height: 12rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
  .conflict { padding-left: .5rem; border-left: 2px solid var(--color-warning-solid); font-size: var(--panel-meta-size); }
  .conflict p { margin: 0; }
  .discussion { margin-top: .35rem; font-size: var(--panel-meta-size); } .discussion li { margin-top: .3rem; }
  .changes-empty { padding: 1rem var(--spacing); }
  .changes-hint { flex: 0 0 auto; padding: .35rem var(--spacing); border-top: 1px solid var(--color-divider); text-align: center; }
  @media (max-width: 32rem) { .changes-header, .filter-bar, .select-bar, .change-row { padding-inline: calc(var(--spacing) * .75); } .change-row { padding-left: calc(var(--spacing) * .75 - 3px); } .row-context { max-width: 10rem; } }
</style>
