<script>
  // The Git panel. Every project that can be edited has one.
  //
  // A web project cannot run git, so its panel shows the last push and, in the
  // desktop app, connects the project to a git repository kept on this computer
  // (git/GitWeb.svelte). The state belongs to a controller in lib/reader; this
  // lays the panel out.
  import PanelTabs from "../PanelTabs.svelte";
  import GitWeb from "./git/GitWeb.svelte";
  import { m } from "../../lib/paraglide/messages.js";

  let {
    open = false,
    // The controller, the project and what the reader may do.
    web = null,
    slug = "",
    linkKey = "",
    // The desktop app connects a repository: its connection.
    desktop = false,
    connection = null,
    // Opens the history panel.
    onhistory = null,
  } = $props();
</script>

<section class="panel panel-tabbed git-panel" aria-label={m.panels_git()}>
  <PanelTabs id="git" title={m.panels_git()}>
    <div class="git-body">
      <GitWeb {web} {slug} {linkKey} {open} {desktop} {connection} {onhistory} />
    </div>
  </PanelTabs>
</section>

<style>
  /* The pane does not scroll; this body does. */
  .git-body { flex: 1 1 0; min-height: 0; overflow-y: auto; }
</style>
