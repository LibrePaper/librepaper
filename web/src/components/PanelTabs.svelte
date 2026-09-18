<script>
  // The tab strip a panel wears when its body is two or three views of the
  // same thing. Both panels that have one were writing out the same Skeleton
  // root, the same list, the same id scheme and the same four layout rules,
  // and the two copies had already drifted: one sat flush at the top of its
  // panel and the other a padding's width down from it, because only one of
  // them had thought to take the panel's own padding off.
  //
  // So there is one of it. The roving tabindex, the arrow-key wrap, Home and
  // End, and the tab/panel pairing are Skeleton's, which is Zag's, rather
  // than a keydown handler written out again here. Everything about how the
  // row looks and how a pane is laid out inside it lives on `.panel-tabbed`
  // and `.panel-tabs` in the stylesheet, where both panels read the same
  // rules rather than each keeping a version of them.
  //
  // The panes themselves stay with the panel that owns them: what a tab
  // contains is the panel's business, and only the strip was ever shared.
  import { Tabs } from "@skeletonlabs/skeleton-svelte";

  let {
    // Names the strip's ids: `<id>-tab-<value>` and `<id>-pane-<value>`, which
    // is how the rest of the reader reaches a particular tab.
    id,
    label,
    // `{ id, label, dot }`. A dot is the small unread mark, and its value is
    // what it says to a screen reader.
    tabs = [],
    value,
    onchange,
    listClass = "",
    children,
  } = $props();
</script>

<Tabs
  class="panel-tabs-root"
  {value}
  onValueChange={(event) => onchange?.(event.value)}
  ids={{ trigger: (one) => `${id}-tab-${one}`, content: (one) => `${id}-pane-${one}` }}
>
  <Tabs.List class="panel-tabs {listClass}" aria-label={label}>
    {#each tabs as tab (tab.id)}
      <Tabs.Trigger value={tab.id} title={tab.label}>
        {tab.label}{#if tab.dot}<span class="unread" aria-label={tab.dot}></span>{/if}
      </Tabs.Trigger>
    {/each}
  </Tabs.List>
  {@render children?.()}
</Tabs>
