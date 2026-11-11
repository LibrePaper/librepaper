<script>
  import Icon from "./Icon.svelte";
  import ShortcutTip from "./ShortcutTip.svelte";
  import ToolBand from "./ToolBand.svelte";
  import { m } from "../lib/paraglide/messages.js";
  import { FORMAT_TOOLS, isToolActive } from "../lib/visual-markup/format-tools.js";

  // The formatting row at the top of the source pane, shown in visual mode. It
  // is a ToolBand, the same band as the preview's, so the two sides line up and
  // fall back to a "..." menu the same way: tools move into it from the right
  // as the pane narrows. What it offers is FORMAT_TOOLS; `active` is the
  // formatting at the caret, and `onformat(id)` is asked to apply a tool, and
  // `oninsert(actionId)` to open the Insert menu flow of an insert tool.
  // `modalEditor` is true in Vim or Emacs mode, where a tool's key belongs to
  // the mode and is not printed, the same rule the menus follow.
  // `oncommand(id)` runs a tool that names a command from lib/commands.js, and
  // `available` is the Edit menu's availability by command id: a tool whose
  // command is unavailable is drawn disabled. `onmenuselect` runs after a tool
  // chosen from the "..." menu, so the editor can take the focus back.
  let { active = null, modalEditor = false, available = null, onformat = () => {}, oninsert = () => {}, oncommand = () => {}, onmenuselect = undefined } = $props();

  // A tool whose command is unavailable is drawn disabled, and a disabled button
  // cannot take the focus, so it is neither an arrow-key stop nor the Tab stop.
  function isDisabled(tool) {
    return Boolean(tool.command && available && !available[tool.command]);
  }
  const run = (tool) => (tool.command ? oncommand(tool.command) : tool.insert ? oninsert(tool.insert) : onformat(tool.id));

  const items = $derived(
    FORMAT_TOOLS.map((tool) => ({
      id: tool.id,
      group: tool.group,
      label: tool.label,
      pressed: isToolActive(tool, active),
      disabled: isDisabled(tool),
      onselect: () => run(tool),
      tool,
    })),
  );

  // A roving tabindex: one button is in the Tab order, and the arrow keys move
  // the focus between them.
  let focused = $state(FORMAT_TOOLS[0].id);
  // The tools that have moved into the menu; they cannot be the Tab stop.
  let moved = $state([]);

  // The Tab stop is the focused tool while it is enabled and showing; otherwise
  // it falls back to the first such tool, so the toolbar stays reachable with Tab.
  const tabStop = $derived.by(() => {
    const ready = items.filter((item) => !item.disabled && !moved.includes(item.id));
    if (ready.some((item) => item.id === focused)) return focused;
    return ready[0]?.id ?? null;
  });
</script>

<ToolBand {items} toolbar label={m.formattoolbar_label()} bind:hidden={moved} {onmenuselect}>
  {#snippet item(entry)}
    {@const tool = entry.tool}
    <!-- The mousedown is cancelled so the click does not take the focus from
         the source, which keeps the selection the tool acts on. -->
    <ShortcutTip text={tool.label} shortcut={modalEditor ? null : tool.shortcut}>
      {#snippet children(attributes)}
        <button
          {...attributes}
          type="button"
          class="format-tool"
          data-tool={tool.id}
          aria-label={tool.label}
          aria-pressed={entry.pressed}
          tabindex={tabStop === tool.id ? 0 : -1}
          disabled={entry.disabled}
          onfocus={() => (focused = tool.id)}
          onmousedown={(event) => event.preventDefault()}
          onclick={() => run(tool)}
        >
          <Icon name={tool.icon} size={16} />
        </button>
      {/snippet}
    </ShortcutTip>
  {/snippet}
</ToolBand>

<style>
  .format-tool {
    display: inline-grid;
    flex: none;
    place-items: center;
    min-width: 2rem;
    height: 1.75rem;
    padding-inline: calc(var(--spacing) * 1.5);
    border: 1px solid transparent;
    border-radius: var(--radius-base);
    color: inherit;
    font-size: var(--text-sm);
    line-height: 1;
    cursor: pointer;
  }
  .format-tool:not([aria-pressed="true"]) { background: none; }
  .format-tool:hover:not(:disabled) {
    background: var(--color-tonal-hover);
  }
  .format-tool:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .format-tool:focus-visible {
    outline: 2px solid var(--color-brand);
    outline-offset: 1px;
  }
</style>
