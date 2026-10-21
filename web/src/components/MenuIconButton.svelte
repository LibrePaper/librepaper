<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import Icon from "./Icon.svelte";

  // `pressed` marks a trigger whose menu lists the current place, the way a
  // rail place is marked; the rail's styling keys on aria-pressed.
  let { icon, label, projectSlug = undefined, size = "1.25rem", pressed = null, class: className = "" } = $props();
</script>

<!-- The menu primitive owns the trigger's ARIA, keyboard, and open-state
     behavior; keep its icon-button styling and project label in one wrapper. -->
<Menu.Trigger
  class={`btn-icon icon-control icon-standard icon-plain ${className}`}
  data-project-slug={projectSlug}
  aria-label={label}
  aria-pressed={pressed === null ? undefined : pressed}
>
  <Icon name={icon} {size} />
</Menu.Trigger>
