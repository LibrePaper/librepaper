<script>
  import Icon from "./Icon.svelte";
  import { FORMAT_TOOLS, isToolActive } from "../lib/visual-markup/formatting.js";

  // The formatting row under the top bar, shown in visual mode. What it
  // offers is FORMAT_TOOLS; `active` is the formatting at the caret, and
  // `onformat(id)` is asked to apply a tool.
  let { active = null, hidden = false, onformat = () => {} } = $props();

  // Consecutive tools that share a group are drawn together.
  const GROUPS = FORMAT_TOOLS.reduce((groups, tool) => {
    const last = groups.at(-1);
    if (last && last.name === tool.group) last.tools.push(tool);
    else groups.push({ name: tool.group, tools: [tool] });
    return groups;
  }, []);

  // A roving tabindex: one button is in the Tab order, and the arrow keys move
  // the focus between them.
  let focused = $state(FORMAT_TOOLS[0].id);
  let bar = $state(null);

  function titleOf(tool) {
    return tool.shortcut ? `${tool.label} (${tool.shortcut})` : tool.label;
  }

  function onkeydown(event) {
    const buttons = [...bar.querySelectorAll("button")];
    const here = buttons.indexOf(document.activeElement);
    if (here === -1) return;
    const step = { ArrowRight: here + 1, ArrowLeft: here - 1, Home: 0, End: buttons.length - 1 }[event.key];
    if (step === undefined) return;
    event.preventDefault();
    buttons[(step + buttons.length) % buttons.length].focus();
  }
</script>

<div class="format-bar" class:bar-hidden={hidden} role="toolbar" tabindex="-1" aria-label="Formatting" bind:this={bar} {onkeydown}>
  {#each GROUPS as group, index (group.name)}
    {#if index}<span class="format-divider" aria-hidden="true"></span>{/if}
    <div class="format-group">
      {#each group.tools as tool (tool.id)}
        {@const on = isToolActive(tool, active)}
        <!-- The mousedown is cancelled so the click does not take the focus from
             the source, which keeps the selection the tool acts on. -->
        <button
          type="button"
          class="format-tool"
          data-tool={tool.id}
          aria-label={tool.label}
          aria-pressed={on}
          title={titleOf(tool)}
          tabindex={focused === tool.id ? 0 : -1}
          onfocus={() => (focused = tool.id)}
          onmousedown={(event) => event.preventDefault()}
          onclick={() => onformat(tool.id)}
        >
          {#if tool.icon}<Icon name={tool.icon} size={16} />{:else}{tool.glyph}{/if}
        </button>
      {/each}
    </div>
  {/each}
</div>

<style>
  /* One row as tall as the top bar's own rhythm, with the same subtle rule under
     it. Overflow scrolls sideways, which is how a phone keeps every tool. */
  .format-bar {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    height: var(--librepaper-format-bar);
    padding-inline: calc(var(--spacing) * 4);
    border-bottom: 1px solid var(--color-pane-edge);
    background: var(--color-shell);
    overflow-x: auto;
    overflow-y: hidden;
    white-space: nowrap;
    scrollbar-width: thin;
  }
  /* On a phone the bar steps aside with the top bar, rising into the space it leaves. */
  @media (max-width: 760px) {
    .format-bar {
      transition: margin-top 200ms ease;
    }
    .format-bar.bar-hidden {
      margin-top: calc(-1 * var(--librepaper-bar));
    }
  }
  .format-group {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 0.5);
    flex: none;
  }
  .format-divider {
    flex: none;
    width: 1px;
    height: 1.25rem;
    background: var(--color-pane-edge);
  }
  .format-tool {
    display: inline-grid;
    flex: none;
    place-items: center;
    min-width: 2rem;
    height: 1.75rem;
    padding-inline: calc(var(--spacing) * 1.5);
    border: 1px solid transparent;
    border-radius: var(--radius-base);
    background: none;
    color: inherit;
    font-size: var(--text-sm);
    line-height: 1;
    cursor: pointer;
  }
  .format-tool:hover {
    background: var(--color-tonal-hover);
  }
  .format-tool[aria-pressed="true"] {
    border-color: var(--color-brand);
    background: var(--color-row-selected);
    color: var(--color-brand);
  }
  .format-tool:focus-visible {
    outline: 2px solid var(--color-brand);
    outline-offset: 1px;
  }
</style>
