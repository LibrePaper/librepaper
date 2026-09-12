<script>
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import Files from "./Files.svelte";
  import Outline from "./Outline.svelte";
  import Agent from "./Agent.svelte";
  import Collaboration from "./Collaboration.svelte";
  import Changes from "./Changes.svelte";
  import Share from "./Share.svelte";
  import Diagnostics from "./Diagnostics.svelte";
  import History from "./History.svelte";
  import PendingAnnotations from "./PendingAnnotations.svelte";
  import Grip from "../Grip.svelte";

  let {
    shown,
    tabs = [],
    panel = "",
    diagnostics = [],
    diagnosticBadge = "",
    errorCount = 0,
    warningCount = 0,
    editing = false,
    layout = "split",
    arrangements = {},
    settled = false,
    visitedPanels = [],
    unconfirmed = [],
    mayEdit = false,
    trackingState = { revisions: [], enabled: false, showMarkup: true },
    selectedRevision = "",
    ontracking,
    onmarkup,
    onrevisionreveal,
    onrevisiondecide,
    onrevisionundo,
    files = [],
    folders = [],
    openFile = "",
    peersByFile = {},
    rules = {},
    outlineHeadings = [],
    outlineActiveFrom = null,
    slug = "",
    link = "",
    canShare = false,
    canSeeSharing = false,
    path = "",
    selection = null,
    selected = "",
    revision = "",
    request = null,
    comments = [],
    figureAt = [],
    identity = "",
    commentingAs = "Anonymous",
    canModerate = false,
    tool = "commenting",
    went = {},
    replacements = {},
    liveChat = [],
    connected = false,
    mayChat = false,
    unreadChat = false,
    collaborationTab = $bindable("comments"),
    checkpoints = [],
    viewing = null,
    historyDurability = null,
    historyController,
    historyProblem = "",
    historyBaseline = null,
    historyChanges = null,
    historyChangedPaths = [],
    historyRedlines = true,
    fileDiff = null,
    historyComparePoint = null,
    localAppDiagnostics = [],
    previewMain = "",
    lastLatexResult = null,
    compact = false,
    outbox,
    collaboration,
    panes,
    sidebarPane,
    onsize,
    onguide,
    ongrab,
    onselectpanel,
    oncyclelayout,
    onfilelist = $bindable(null),
    onopen,
    onadd,
    onmkdir,
    onrelocate,
    ondelete,
    onduplicate,
    onmain,
    onfigure,
    ontext,
    ondownload,
    ondownloaditem,
    onopenoutline,
    oncommenttask,
    ondiagnostictask,
    onreview,
    onpreview,
    onsendchat,
    ontool,
    onreveal,
    onresolve,
    onaskdelete,
    ondeletemany,
    onreply,
    ondecide,
    onrejectconfirmed,
    onsethistoryredlines,
    onshowhistory,
    onshareclose,
    onretrylocal,
    canopendiagnostic = () => false,
    onopendiagnostic,
    ondrop,
    onviewpoint,
    oncompare,
    oncomparecurrent,
    onrefreshcurrent,
    onbackhistory,
    onnamecheckpoint,
    onrestore,
    oncopycheckpoint,
    onstephistory,
    oncheckpointfile,
    onfilediff,
    onclosefilediff,
    onretryannotation,
    ondiscardannotation,
  } = $props();

  const iconFor = (id) => id === "files" ? "folder"
    : id === "outline" ? "list"
      : id === "collaboration" ? "comment"
        : id === "changes" ? "pencil"
          : id === "agent" ? "bot"
            : id === "history" ? "history"
              : id === "share" ? "users" : "sliders";
</script>

<!-- The sidebar owns panel selection and panel-specific wiring. Reader keeps
     document state and actions; this component only presents those features. -->
<aside class="sidebar" class:collapsed={!shown.comments}
       ondragover={(event) => event.preventDefault()} ondrop={ondrop}>
  <div class="sidebar-activity">
    <div class="activity-sections" role="group" aria-label="Sidebar sections">
      {#each tabs as tab (tab.id)}
        {#if tab.id === "diagnostics"}
          <div class="activity-diagnostics">
            <IconButton icon="triangle-alert"
              label={diagnosticBadge ? `${tab.says}: ${diagnosticBadge}` : tab.says}
              pressed={panel === tab.id}
              onclick={() => onselectpanel?.(tab.id)} />
            {#if diagnostics.length}
              <span class="activity-counts" aria-hidden="true">
                {#if errorCount}<span class="activity-count errors">{errorCount}</span>{/if}
                {#if warningCount}<span class="activity-count warnings">{warningCount}</span>{/if}
              </span>
            {/if}
          </div>
        {:else if tab.id === "changes"}
          <div class="activity-tracking">
            <IconButton icon={iconFor(tab.id)} label={trackingState.enabled ? "Changes — tracking on" : tab.says}
              pressed={panel === tab.id} onclick={() => onselectpanel?.(tab.id)} />
            {#if trackingState.enabled}<span class="tracking-indicator" aria-hidden="true"></span>{/if}
          </div>
        {:else}
          <IconButton icon={iconFor(tab.id)} label={tab.says} pressed={panel === tab.id}
            onclick={() => onselectpanel?.(tab.id)} />
        {/if}
      {/each}
    </div>
    <div class="activity-bottom" role="group" aria-label="Workspace controls">
      {#if editing}
        <IconButton icon={arrangements[layout].icon}
          label={`Layout: ${arrangements[layout].says}. Switch to ${arrangements[arrangements[layout].next].says}`}
          onclick={oncyclelayout} />
      {/if}
      <IconButton icon="help" label="Documentation" href="/documentation" />
    </div>
  </div>
  {#if settled}
    <div class="sidebar-content">
      {#each tabs.filter((tab) => visitedPanels.includes(tab.id) || (tab.id === "collaboration" && unconfirmed.length)) as tab (tab.id)}
        <div class="panel-slot" hidden={panel !== tab.id || !shown.comments}>
          {#if tab.id === "files" && mayEdit}
            <Files bind:this={onfilelist} {files} {folders} open={openFile} peers={peersByFile}
              {mayEdit} {rules} onopen={onopen} onadd={onadd} onmkdir={onmkdir} onrelocate={onrelocate}
              ondelete={ondelete} onduplicate={onduplicate} onmain={onmain} onfigure={onfigure} ontext={ontext}
              ondownload={ondownload} ondownloaditem={ondownloaditem} />
          {:else if tab.id === "outline" && mayEdit}
            <Outline headings={outlineHeadings} activeFrom={outlineActiveFrom} onselect={onopenoutline} />
          {:else if tab.id === "agent"}
            <Agent {slug} {link} {canShare} {path} {selection} {revision} request={request}
              {comments} {diagnostics} oncommenttask={oncommenttask} ondiagnostictask={ondiagnostictask}
              onreview={onreview} onpreview={onpreview} />
          {:else if tab.id === "collaboration"}
            <Collaboration messages={liveChat} {connected} canPost={mayChat} onsend={onsendchat}
              {unreadChat} bind:tab={collaborationTab} {comments} {figureAt} {identity}
              commentingAs={commentingAs} {canModerate} {tool} {went} {replacements}
              canComment={mayChat} hasFigures={figureAt.length > 0} ontool={ontool} onreveal={onreveal}
              {selected} onresolve={onresolve} ondelete={onaskdelete}
              ondeletemany={ondeletemany} onreply={onreply} />
          {:else if tab.id === "changes"}
            <Changes {comments} {figureAt} {identity} commentingAs={commentingAs} {canModerate} {tool}
              revisions={trackingState.revisions} tracking={trackingState.enabled} showMarkup={trackingState.showMarkup}
              canTrack={mayEdit && !viewing} {selectedRevision} {ontracking} {onmarkup}
              onrevisionreveal={onrevisionreveal} onrevisiondecide={onrevisiondecide} onrevisionundo={onrevisionundo}
              {went} {replacements} canComment={mayChat} ontool={ontool} onreveal={onreveal}
              {selected} onresolve={onresolve} ondelete={onaskdelete}
              ondeletemany={ondeletemany} onreply={onreply}
              onaccept={(comment) => ondecide?.(comment, "accept")}
              onreject={(comment) => ondecide?.(comment, "reject")}
              onrejectconfirmed={onrejectconfirmed}
              onhistory={onshowhistory} />
          {:else if tab.id === "share" && canSeeSharing}
            <Share open={panel === "share" && shown.comments} inline {slug} onclose={onshareclose} />
          {:else if tab.id === "diagnostics"}
            <Diagnostics {diagnostics} localAppProblem={localAppDiagnostics.length > 0}
              onretrylocal={onretrylocal} main={previewMain} canOpen={canopendiagnostic}
              onopen={onopendiagnostic} provenance={lastLatexResult?.provenance || null}
              attempts={lastLatexResult?.attempts || []} />
          {:else if tab.id === "history"}
            <History {checkpoints} viewing={viewing?.sha || null} canEdit={mayEdit} durability={historyDurability}
              comparingCurrent={historyController?.comparingCurrent} newerEdits={historyController?.newerEdits}
              oncomparecurrent={oncomparecurrent} onrefreshcurrent={onrefreshcurrent} problem={historyProblem}
              baseline={historyBaseline} changes={historyChanges} changedPaths={historyChangedPaths}
              redlines={historyRedlines} onredlines={onsethistoryredlines} {fileDiff}
              target={historyComparePoint} onview={onviewpoint} oncompare={oncompare}
              onback={onbackhistory} onname={onnamecheckpoint} onrestore={onrestore}
              oncopy={oncopycheckpoint} onstep={onstephistory} oncheckpointfile={oncheckpointfile}
              onfilediff={onfilediff} onclosefilediff={onclosefilediff} />
          {/if}
          {#if panel === tab.id && ["collaboration", "changes"].includes(tab.id) && unconfirmed.length}
            <div class="pending-recovery">
              <PendingAnnotations items={unconfirmed}
                onretry={(id) => onretryannotation?.(id, (message) => collaboration?.send(message))}
                ondiscard={ondiscardannotation} />
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</aside>

{#if shown.comments && !compact}
  <Grip pane={sidebarPane} label="Resize the left-hand column" {panes}
    onsize={onsize} onguide={onguide} ongrab={ongrab} />
{/if}

<style>
  .sidebar { flex-direction: row; }
  .sidebar.collapsed { flex: 0 0 var(--librepaper-activity); }
  .sidebar-activity { display: flex; flex: none; flex-direction: column; align-items: center; gap: var(--spacing); width: var(--librepaper-activity); padding-block: calc(var(--spacing) * 3); border-right: 1px solid var(--color-divider); }
  .activity-bottom { display: flex; flex-direction: column; align-items: center; gap: var(--spacing); margin-top: auto; }
  .activity-sections { display: flex; flex-direction: column; align-items: center; gap: var(--spacing); }
  .activity-sections :global(.icon-control) { position: relative; width: 2rem; height: 2rem; border-radius: var(--radius-base); }
  .activity-sections :global(.icon-control[aria-pressed="true"]) { background: var(--color-primary-100-900); color: var(--color-primary-700-300); }
  .activity-sections :global(.icon-control[aria-pressed="true"]::before) { content: ""; position: absolute; left: calc((2rem - var(--librepaper-activity)) / 2 + 1px); top: .375rem; bottom: .375rem; width: 3px; border-radius: 0 2px 2px 0; background: var(--color-primary-500); }
  .activity-diagnostics { display: flex; flex-direction: column; align-items: center; gap: 2px; }
  .activity-tracking { position: relative; }
  .tracking-indicator { position: absolute; right: 1px; top: 1px; width: 7px; height: 7px; border-radius: 50%; background: var(--color-primary-500); pointer-events: none; }
  .activity-counts { display: flex; gap: 4px; font-size: .625rem; line-height: 1; font-weight: 600; font-variant-numeric: tabular-nums; }
  .activity-count.errors { color: var(--color-error-500); }
  .activity-count.warnings { color: var(--color-warning-500); }
  .sidebar-content { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; overflow: hidden; }
  .panel-slot { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; overflow: hidden; }
  .panel-slot[hidden] { display: none; }
  .pending-recovery { flex-shrink: 0; max-height: 35%; overflow-y: auto; border-top: 1px solid var(--color-surface-300-700); background: var(--color-surface-100-900); }
  @media (max-width: 760px) {
    .sidebar, .sidebar.collapsed { flex-direction: column; }
    .sidebar-activity { display: flex; order: 1; width: 100%; padding-block: var(--spacing); border-right: 0; border-top: 1px solid var(--color-divider); }
    .sidebar-activity .activity-sections { display: none; }
    .activity-bottom { flex-direction: row; }
    .activity-sections { flex-direction: row; }
    .sidebar-content { overflow: hidden; }
  }
</style>
