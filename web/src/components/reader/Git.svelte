<script>
  // The Git panel. Every project that can be edited has one, and it shows what
  // git means for that project.
  //
  // A web project cannot run git: its repository is on GitHub and the Action
  // does the work, so the panel is where the Action is set up and seen
  // (git/GitWeb.svelte). A folder project in the desktop app has a folder, so
  // the panel runs git on it: the link to its web project that the Sync tab
  // used to hold, then the folder's own repository (git/GitLibrePaper.svelte,
  // git/GitRepository.svelte). The state of each belongs to a controller in
  // lib/reader; this lays the sections out.
  import PanelHeader from "../PanelHeader.svelte";
  import GitWeb from "./git/GitWeb.svelte";
  import GitLibrePaper from "./git/GitLibrePaper.svelte";
  import GitRepository from "./git/GitRepository.svelte";

  let {
    local = false,
    open = false,
    // A web project: the controller, the project and what the reader may do.
    web = null,
    slug = "",
    owner = false,
    linkKey = "",
    // A folder project: the two controllers, the differ the editor area shows,
    // and the title offered when the folder is published.
    sync = null,
    git = null,
    differ = null,
    defaultTitle = "",
    ondiff = undefined,
  } = $props();
</script>

<section class="panel git-panel" aria-label="Git">
  <PanelHeader title="Git" />
  {#if local}
    <div class="git-sections">
      <GitLibrePaper {sync} {open} {defaultTitle} />
      <GitRepository {git} {differ} {open} {ondiff} />
    </div>
  {:else}
    <GitWeb {web} {slug} {owner} {linkKey} {open} />
  {/if}
</section>

<style>
  .git-sections { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 5); }
  .git-sections > :global(.git-block + .git-block) { padding-top: calc(var(--spacing) * 4); border-top: 1px solid var(--color-divider); }
</style>
