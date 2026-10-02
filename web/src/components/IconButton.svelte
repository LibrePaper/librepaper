<script>
  import { Tooltip } from "@skeletonlabs/skeleton-svelte";
  import Icon from "./Icon.svelte";

  // One button, one shape. Every icon control in the application is this, so
  // they are the same square with the icon at the same size, rather than each
  // place that wants one deciding again.
  //
  // Four weights, which is the whole vocabulary: filled for the one action
  // that changes the document, tonal for a state that is on, plain for ordinary
  // actions, and outlined when an action needs extra emphasis.
  //
  // An icon without a word beside it has to say what it is. A title attribute
  // says it only to a mouse, after a wait the browser chooses, in a box the
  // page has no say over; this says it to the keyboard too, promptly, in the
  // application's own colours.
  let {
    icon,
    label,
    title = label,
    href = null,
    pressed = null,
    // For a button that reveals something: what it reveals, and whether it is
    // showing. A pressed icon says the control is on; these say what the page
    // did about it, which is the part a reader who cannot see the panel open
    // has no other way to learn.
    controls = null,
    expanded = null,
    disabled = false,
    tone = null,
    size = null,
    colour = null,
    filled = false,
    // Optional visible text inside the button. `label` remains the full
    // accessible name.
    visibleLabel = null,
    onclick,
  } = $props();

  const TONES = {
    filled: "lp-control-brand",
    tonal: "lp-control-tonal-brand",
    outlined: "lp-control-outline",
    plain: "",
  };
  const preset = $derived(TONES[tone] ?? (pressed === true ? TONES.tonal : TONES.plain));
  const classes = $derived(`btn-icon icon-control ${visibleLabel ? "has-compact-label" : ""} ${size ? "" : "icon-standard"} ${tone === "plain" || (tone === null && pressed !== true) ? "icon-plain" : ""} ${size ?? ""} ${preset} ${colour ?? ""}`);
</script>

<Tooltip openDelay={400} closeDelay={100} positioning={{ placement: "bottom" }}>
  <!-- The trigger is the control itself rather than a wrapper around it: an
       element in between would break the row the controls sit in. -->
  <Tooltip.Trigger>
    {#snippet element(attributes)}
      {#if href}
        <a {...attributes} {href} class={classes} aria-label={label}>
          <Icon name={icon} {filled} />
          {#if visibleLabel}<span class="compact-label" aria-hidden="true">{visibleLabel}</span>{/if}
        </a>
      {:else}
        <button
          {...attributes}
          type="button"
          class={classes}
          aria-label={label}
          aria-controls={controls ?? undefined}
          aria-expanded={expanded === null ? undefined : expanded}
          aria-pressed={pressed === null ? undefined : pressed}
          {disabled}
          {onclick}
        >
          <Icon name={icon} {filled} />
          {#if visibleLabel}<span class="compact-label" aria-hidden="true">{visibleLabel}</span>{/if}
        </button>
      {/if}
    {/snippet}
  </Tooltip.Trigger>
  <Tooltip.Positioner class="z-50">
    <Tooltip.Content class="card lp-control-neutral px-2 py-1 text-xs shadow-lg">
      {title}
    </Tooltip.Content>
  </Tooltip.Positioner>
</Tooltip>

<style>
  .compact-label { display: block; max-width: 100%; overflow: hidden; color: inherit; font-size: 10px; font-weight: 500; line-height: 1.1; text-overflow: ellipsis; white-space: nowrap; }
</style>
