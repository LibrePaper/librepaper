<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import MenuIconButton from "./MenuIconButton.svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import ChatTranscript from "./ChatTranscript.svelte";
  import ChatComposer from "./ChatComposer.svelte";
  let { messages = [], connected = false, canPost = false, onsend, more = false, loadingOlder = false, loadError = "", canClear = false, onloadolder = undefined, onclear = undefined } = $props();
</script>
<section class="live-chat" aria-label="Chat">
  <PanelHeader title="Chat" meta={connected ? "Live" : "Offline"}>
    {#snippet actions()}
      {#if canClear}
        <Menu onSelect={({ value }) => { if (value === "clear") onclear?.(); }}
          positioning={{ placement: "bottom-end", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}>
          <MenuIconButton icon="more-horizontal" label="Chat options" />
          <ExplorerMenu>
            <Menu.Item value="clear" class="menuitem">Clear chat history</Menu.Item>
          </ExplorerMenu>
        </Menu>
      {/if}
    {/snippet}
  </PanelHeader>
  <ChatTranscript {messages} {more} {loadingOlder} {loadError} onloadolder={onloadolder} empty="No messages yet." roleLabel={(message) => message.creator || "Anonymous"} />
  <ChatComposer placeholder="Message everyone here…" disabled={!connected || !canPost} {onsend} />
  {#if !canPost}<p class="panel-muted">Comment access is required to chat.</p>{/if}
</section>
<style>
  /* The pane the collaboration panel puts this in carries the padding. */
  .live-chat { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 3); overflow:hidden; }
  .live-chat > :global(*) { flex-shrink:0; }
</style>
