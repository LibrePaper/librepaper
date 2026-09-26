<script>
  // Things one above another, evenly spaced. The gap is a step of the theme's
  // spacing scale rather than a number, so two stacks on two pages are the
  // same distance apart.
  let { gap = 4, children } = $props();

  const GAPS = { 2: "gap-2", 3: "gap-3", 4: "gap-4", 6: "gap-6", 8: "gap-8" };
</script>

<div class="flex flex-col {GAPS[gap]}">
  {@render children()}
</div>
