<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import MenubarMenu from "../MenubarMenu.svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import Icon from "../Icon.svelte";
  import { m } from "../../lib/paraglide/messages.js";
  import { labelOf, noteOf } from "../../lib/commands.js";
  import { RATIOS } from "../../lib/panes.js";

  let {
    role,
    demo,
    demoHint,
    tabs,
    hasDesktop,
    source,
    exportable,
    view,
    outputs,
    can,
    keys,
    editGroups,
    downloadOpen = $bindable(false),
    onfile,
    onedit,
    oneditopen,
    onview,
    onhelp,
    insert,
  } = $props();

  const tipFor = (id) => (can[id] ? undefined : noteOf(id) || undefined);
</script>

{#snippet exportItem(value, label, target)}
  {@const state = exportable[target]}
  <Menu.Item value={value} class="menuitem" disabled={!state.ok}>
    <span class="menuitem-label">{label}</span>
    {#if !state.ok && state.reason}<span class="export-reason">{state.reason}</span>{/if}
  </Menu.Item>
{/snippet}

<!-- A child menu reports its own selections, so it gets the same handler as
     the File menu; Zag does not pass a sub-menu's select up to its parent. -->
{#snippet downloadMenu()}
  <Menu open={downloadOpen}
        onOpenChange={(event) => (downloadOpen = event.open)}
        onSelect={(chosen) => void onfile(chosen.value)}>
    <Menu.TriggerItem value="submenu:download" class="menuitem">
      <span class="menuitem-label">{m.common_download()}</span>
      <span class="menuitem-chevron" aria-hidden="true"><Icon name="chevron-right" size="1rem" /></span>
    </Menu.TriggerItem>
    <ExplorerMenu>
      {#if role.mayEdit}<Menu.Item value="download" class="menuitem"><span class="menuitem-label">{labelOf("download")}</span></Menu.Item>{/if}
      {@render exportItem("download-html", labelOf("download-html"), "html")}
      {@render exportItem("download-pdf", labelOf("download-pdf"), "pdf")}
      {@render exportItem("download-docx", labelOf("download-docx"), "docx")}
      {#if role.canExportComments && !demo}<Menu.Item value="download-comments" class="menuitem"><span class="menuitem-label">{labelOf("download-comments")}</span></Menu.Item>{/if}
      {#if role.mayEdit}
        <Menu.Item value="download-source" class="menuitem" disabled={!source.toolbarPath}>
          <span class="menuitem-label">{labelOf("download-source")}</span>
          {#if !source.toolbarPath}<span class="export-reason">{m.readerd_download_source_reason()}</span>{/if}
        </Menu.Item>
      {/if}
    </ExplorerMenu>
  </Menu>
{/snippet}

{#snippet fileItems()}
  {#if role.mayEdit}
    <Menu.Item value="new-file" class="menuitem">{labelOf("new-file")}</Menu.Item>
    <Menu.Item value="new-folder" class="menuitem">{labelOf("new-folder")}</Menu.Item>
    <Menu.Item value="upload" class="menuitem" disabled={demo}>{labelOf("upload")}{demo ? ` ${demoHint}` : ""}</Menu.Item>
    <hr class="hr my-1" />
  {/if}
  {@render downloadMenu()}
  {#if role.me?.id || demo}<Menu.Item value="save-template" class="menuitem" disabled={demo}>{m.readerd_save_as_template_item()}{demo ? ` ${demoHint}` : ""}</Menu.Item>{/if}
  <hr class="hr my-1" />
  {#if tabs.some((tab) => tab.id === "git")}<Menu.Item value="git" class="menuitem" disabled={demo}>{m.readerd_git_item()}{demo ? ` ${demoHint}` : ""}</Menu.Item>{/if}
  {#if demo || role.canSeeSharing || role.canPublish}<Menu.Item value="share" class="menuitem" disabled={demo}>{labelOf("share")}{demo ? ` ${demoHint}` : ""}</Menu.Item>{/if}
  <!-- Only where the panel behind it is one this reader is offered: a menu
       that opens a column the rail does not have is the second list of who
       may see what that lib/panels.js exists to prevent. -->
  {#if tabs.some((tab) => tab.id === "history")}<Menu.Item value="history" class="menuitem" disabled={demo}>{labelOf("history")}{demo ? ` ${demoHint}` : ""}</Menu.Item>{/if}
  <hr class="hr my-1" />
  {#if role.mayEdit && source.format === "latex" && source.compilesHere}
    <Menu.Item value="compile" class="menuitem"><span class="menuitem-label">{labelOf("compile")}</span>{#if keys.compile}<span class="menuitem-keys">{keys.compile}</span>{/if}</Menu.Item>
  {/if}
  <Menu.Item value="settings" class="menuitem"><span class="menuitem-label">{labelOf("settings")}</span>{#if keys.settings}<span class="menuitem-keys">{keys.settings}</span>{/if}</Menu.Item>
  {#if hasDesktop}
    <hr class="hr my-1" />
    <Menu.Item value="quit" class="menuitem">{m.readerd_quit()}</Menu.Item>
  {/if}
{/snippet}

{#snippet layoutItems()}
  {#each [["source", labelOf("layout-source")], ["document", labelOf("layout-document")], ["split", labelOf("layout-split")]] as [value, label]}
    <Menu.Item value="layout-{value}" class="menuitem" disabled={view.compact && value === "split"} title={view.compact && value === "split" ? noteOf("layout-split") : undefined}>
      <span class="menuitem-check">{(view.compact ? view.activeMobileView === value : view.layout === value) ? "✓" : ""}</span>{label}
    </Menu.Item>
  {/each}
  <hr class="hr my-1" />
  {#each [["left", labelOf("side-left")], ["right", labelOf("side-right")]] as [side, label]}
    <Menu.Item value="side-{side}" class="menuitem" disabled={view.compact} title={view.compact ? noteOf("side-left") : undefined}>
      <span class="menuitem-check">{view.sourceSide === side ? "✓" : ""}</span>{label}
    </Menu.Item>
  {/each}
  <hr class="hr my-1" />
  {#each RATIOS as ratio}
    <Menu.Item value="ratio-{ratio.share}" class="menuitem" disabled={view.compact}>
      <span class="menuitem-check">{view.editorShare === ratio.share ? "✓" : ""}</span>{ratio.says}
    </Menu.Item>
  {/each}
  <hr class="hr my-1" />
  <Menu.Item value="linked" class="menuitem">
    <span class="menuitem-check">{view.linked ? "✓" : ""}</span><span class="menuitem-label">{labelOf("linked")}</span>{#if keys.linked}<span class="menuitem-keys">{keys.linked}</span>{/if}
  </Menu.Item>
  <Menu.Item value="visual-markup" class="menuitem">
    <span class="menuitem-check">{view.visual ? "✓" : ""}</span><span class="menuitem-label">{labelOf("visual-markup")}</span>{#if keys["visual-markup"]}<span class="menuitem-keys">{keys["visual-markup"]}</span>{/if}
  </Menu.Item>
  <hr class="hr my-1" />
  <Menu.Item value="text-bigger" class="menuitem">
    <span class="menuitem-check"></span><span class="menuitem-label">{labelOf("text-bigger")}</span>{#if keys["text-bigger"]}<span class="menuitem-keys">{keys["text-bigger"]}</span>{/if}
  </Menu.Item>
  <Menu.Item value="text-smaller" class="menuitem">
    <span class="menuitem-check"></span><span class="menuitem-label">{labelOf("text-smaller")}</span>{#if keys["text-smaller"]}<span class="menuitem-keys">{keys["text-smaller"]}</span>{/if}
  </Menu.Item>
  <Menu.Item value="text-reset" class="menuitem">
    <span class="menuitem-check"></span><span class="menuitem-label">{labelOf("text-reset")}</span>{#if keys["text-reset"]}<span class="menuitem-keys">{keys["text-reset"]}</span>{/if}
  </Menu.Item>
{/snippet}

{#snippet previewItems()}
  {@const selectedFormat = outputs.displayedFormat === "latex" ? outputs.latexOutput : outputs.displayedFormat === "typst" ? outputs.typstOutput : ["markdown", "quarto"].includes(outputs.displayedFormat) ? outputs.markdownOutput : "html"}
  {@const selectableFormat = ["latex", "typst", "markdown", "quarto"].includes(outputs.displayedFormat)}
  <Menu.Item value="format-html" class="menuitem" disabled={!selectableFormat}>
    <span class="menuitem-check">{selectedFormat === "html" ? "✓" : ""}</span>HTML
  </Menu.Item>
  <Menu.Item value="format-pdf" class="menuitem" disabled={!selectableFormat}>
    <span class="menuitem-check">{selectedFormat === "pdf" ? "✓" : ""}</span>PDF
  </Menu.Item>
{/snippet}

{#snippet viewItems()}
  <Menu.Item value="lock-preview" class="menuitem" disabled={!can["lock-preview"]} title={tipFor("lock-preview")}>{labelOf("lock-preview")}</Menu.Item>
  {#if can["unlock-preview"]}<Menu.Item value="unlock-preview" class="menuitem">{labelOf("unlock-preview")}</Menu.Item>{/if}
  <hr class="hr my-1" />
  {@render previewItems()}
  <hr class="hr my-1" />
  {@render layoutItems()}
{/snippet}

<!-- The File menu is the project's, not the preview's: new files, uploads,
     downloads, sharing, history and settings all belong to the project
     whatever pane is on screen, so it stays put when the document pane is
     hidden. -->
<MenubarMenu id="file" label={m.common_file()} onselect={onfile}>
  {@render fileItems()}
</MenubarMenu>
{#if source.editing && role.mayEdit}
  <MenubarMenu id="edit" label={m.readerd_edit_menu()} disabled={!source.editorReady || source.figureShown}
               onopen={oneditopen}
               onselect={onedit}>
    {#each editGroups as group, index}
      {#if index}<hr class="hr my-1" />{/if}
      {#each group as [command, label]}
        <Menu.Item value={command} class="menuitem" disabled={!can[command]} title={tipFor(command)}>
          <span class="menuitem-label">{label}</span>{#if keys[command]}<span class="menuitem-keys">{keys[command]}</span>{/if}
        </Menu.Item>
      {/each}
    {/each}
  </MenubarMenu>
  {@render insert?.()}
{/if}
{#if source.editing}
  <MenubarMenu id="view" label={m.reader_menu_view()} onselect={onview}>
    {@render viewItems()}
  </MenubarMenu>
{/if}
<!-- Help is the registry's two entries that are about the workspace itself;
     runCommand runs them as it runs the palette and the keyboard. -->
<MenubarMenu id="help" label={m.mobilenav_help()} onselect={onhelp}>
  <Menu.Item value="palette" class="menuitem"><span class="menuitem-label">{m.common_command_palette()}</span>{#if keys.palette}<span class="menuitem-keys">{keys.palette}</span>{/if}</Menu.Item>
  <Menu.Item value="shortcuts" class="menuitem"><span class="menuitem-label">{m.common_keyboard_shortcuts()}</span>{#if keys.shortcuts}<span class="menuitem-keys">{keys.shortcuts}</span>{/if}</Menu.Item>
</MenubarMenu>

<style>
  .menuitem-chevron { display: inline-flex; margin-left: auto; padding-left: calc(var(--spacing) * 2); }
  .export-reason { flex: 0 0 auto; margin-left: auto; max-width: 12rem; overflow: hidden; color: var(--color-text-secondary); font-size: .7rem; text-overflow: ellipsis; white-space: nowrap; }
</style>
