<script>
  // The Git panel. Every project that can be edited has one, and it shows what
  // git means for that project.
  //
  // A web project cannot run git, so its panel shows the last push and, in the
  // desktop app, connects a git repository on this computer (git/GitWeb.svelte).
  // A folder project has a folder, so the panel runs git on it: the link to its
  // web project that the Sync tab used to hold, then the folder's own
  // repository (git/GitLibrePaper.svelte, git/GitRepository.svelte). The state of each belongs to a controller in
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
    linkKey = "",
    // The desktop app connects a repository: the clone of an address, or a folder.
    desktop = false,
    onclone = undefined,
    onfolder = undefined,
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
    <GitWeb {web} {slug} {linkKey} {open} {desktop} {onclone} {onfolder} />
  {/if}
</section>

<style>
  .git-sections { display: grid; grid-template-columns: minmax(0, 1fr); gap: calc(var(--spacing) * 5); }
  .git-sections > :global(.git-block + .git-block) { padding-top: calc(var(--spacing) * 4); border-top: 1px solid var(--color-divider); }
</style>
