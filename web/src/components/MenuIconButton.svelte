<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import Icon from "./Icon.svelte";

  let { icon, label, projectSlug, size = "1.25rem", class: className = "" } = $props();
</script>

<!-- The menu primitive owns the trigger's ARIA, keyboard, and open-state
     behavior; keep its icon-button styling and project label in one wrapper. -->
<Menu.Trigger
  class={`btn-icon icon-control icon-standard icon-plain ${className}`}
  data-project-slug={projectSlug}
  aria-label={label}
>
  <Icon name={icon} {size} />
</Menu.Trigger>
