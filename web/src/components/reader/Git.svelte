<script>
  // The Git panel. Every project that can be edited has one.
  //
  // A web project cannot run git, so its panel shows the last push and, in the
  // desktop app, connects the project to a git repository kept on this computer
  // (git/GitWeb.svelte). The state belongs to a controller in lib/reader; this
  // lays the panel out.
  import PanelHeader from "../PanelHeader.svelte";
  import GitWeb from "./git/GitWeb.svelte";

  let {
    open = false,
    // The controller, the project and what the reader may do.
    web = null,
    slug = "",
    linkKey = "",
    // The desktop app connects a repository: its connection.
    desktop = false,
    connection = null,
  } = $props();
</script>

<section class="panel git-panel" aria-label="Git">
  <PanelHeader title="Git" />
  <GitWeb {web} {slug} {linkKey} {open} {desktop} {connection} />
</section>
