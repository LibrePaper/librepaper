<script>
  import IconButton from "../IconButton.svelte";
  import PanelRail from "./PanelRail.svelte";
  import Grip from "../Grip.svelte";
  import { slotId } from "../../lib/panels.js";

  // The column at the left: a rail of icons, and one panel at a time beside
  // it. What is *in* a panel is none of this component's business -- each
  // arrives as a snippet, already wired by whoever owns the state behind it --
  // so adding a panel means adding a descriptor and a snippet, and never
  // touching this file.
  //
  // Which is why it is here rather than under reader/: a document's panels and
  // an account's projects are the same shape -- a rail of places, one of them
  // showing -- and the landing page draws this too. That is the whole of what
  // makes going into a project feel like going *in* rather than going
  // somewhere else: the rail is at the same three rem on both pages, the bar
  // above it is the same bar, and the width of the column beside it is read
  // from the same key. Only the contents cross the navigation.
  let {
    shown,
    tabs = [],
    panel = "",
    // The panels to keep rendered: a panel is mounted on its first visit and
    // retained after it, so scroll positions, expanded folders and unsent
    // drafts survive looking at something else.
    mounted = [],
    panels = {},
    // What a rail button says beyond its name, by panel id: an overriding
    // label, counts to print beneath it, a dot to mark it. The rail draws
    // whatever it is given without knowing what a diagnostic or a tracked
    // change is.
    badges = {},
    settled = false,
    // What sits at the foot of the rail, under the places: the controls that
    // belong to the page rather than to any one panel. The reader puts its
    // layout switch and a way home there; the landing page has no home to go
    // to and puts only help. A snippet rather than a list of flags, because
    // this component has no business knowing what a layout is.
    controls,
    // Whether the column can be dragged wider, and whether a panel is even
    // expected. The landing page's rail is navigation with nothing beside it,
    // so it takes neither.
    resizable = true,
    label = "Sidebar",
    compact = false,
    panes,
    sidebarPane,
    onsize,
    onguide,
    ongrab,
    onselectpanel,
    oncyclelayout,
    ondrop,
  } = $props();
</script>

<!-- The sidebar owns panel selection and the shape of the column. Reader keeps
     document state and actions; this component only presents them. -->
<aside class="sidebar" class:collapsed={!shown.comments} aria-label={label}
       ondragover={(event) => event.preventDefault()} ondrop={ondrop}>
  <div class="sidebar-activity">
    <div class="activity-sections" role="group" aria-label="{label} sections">
      <PanelRail {tabs} {panel} {badges} open={shown.comments} marksWhenClosed
        onselect={(id) => onselectpanel?.(id)} />
    </div>
    <div class="activity-bottom" role="group" aria-label="Workspace controls">
      {@render controls?.()}
      <IconButton icon="help" label="Documentation" href="/documentation" />
    </div>
  </div>
  {#if settled}
    <div class="sidebar-content">
      {#each tabs.filter((tab) => mounted.includes(tab.id)) as tab (tab.id)}
        <div id={slotId(tab.id)} class="panel-slot" role="region" aria-label={tab.says}
             hidden={panel !== tab.id || !shown.comments}>
          {@render panels[tab.id]?.()}
        </div>
      {/each}
    </div>
  {/if}
</aside>

{#if shown.comments && resizable && !compact}
  <Grip pane={sidebarPane} label="Resize the left-hand column" {panes}
    controls={panel ? slotId(panel) : undefined}
    onsize={onsize} onguide={onguide} ongrab={ongrab} />
{/if}

<style>
  .sidebar { flex-direction: row; }
  .sidebar.collapsed { flex: 0 0 var(--librepaper-activity); }
  .sidebar-activity { display: flex; flex: none; flex-direction: column; align-items: center; gap: var(--spacing); width: var(--librepaper-activity); min-height: 0; padding-block: calc(var(--spacing) * 3); border-right: 1px solid var(--color-pane-edge); }
  .activity-bottom { display: flex; flex: none; flex-direction: column; align-items: center; gap: var(--spacing); margin-top: auto; }
  /* Eight panels and three workspace controls are taller than a short window,
     and the column clips what it cannot fit -- so the panels scroll and the
     controls below them stay put, rather than the home and help buttons
     disappearing off the bottom of a laptop in landscape with no sign that
     they were ever there. The bar is hidden because it would be most of the
     rail's width; the wheel, a drag and the keyboard all still reach it. */
  .activity-sections { display: flex; flex-direction: column; align-self: stretch; align-items: center; gap: var(--spacing); min-height: 0; overflow-y: auto; scrollbar-width: none; }
  .activity-sections::-webkit-scrollbar { display: none; }
  .activity-sections :global(.icon-control) { position: relative; width: 2rem; height: 2rem; border-radius: var(--radius-base); }
  .activity-sections :global(.icon-control[aria-pressed="true"]) { background: var(--color-primary-100-900); color: var(--color-primary-700-300); }
  .activity-sections :global(.icon-control[aria-pressed="true"]::before) { content: ""; position: absolute; left: calc((2rem - var(--librepaper-activity)) / 2 + 1px); top: .375rem; bottom: .375rem; width: 3px; border-radius: 0 2px 2px 0; background: var(--color-primary-500); }
  .sidebar-content { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; overflow: hidden; }
  .panel-slot { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; overflow: hidden; }
  .panel-slot[hidden] { display: none; }
  @media (max-width: 760px) {
    .sidebar, .sidebar.collapsed { flex-direction: column; }
    .sidebar-activity { display: flex; order: 1; width: 100%; padding-block: var(--spacing); border-right: 0; border-top: 1px solid var(--color-pane-edge); }
    .sidebar-activity .activity-sections { display: none; }
    .activity-bottom { flex-direction: row; }
    .activity-sections { flex-direction: row; }
    .sidebar-content { overflow: hidden; }
  }
</style>
