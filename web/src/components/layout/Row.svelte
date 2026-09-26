<script>
  // Things side by side on one line, centred against each other. The same
  // three choices every row in the application needs: how far apart, whether
  // it wraps, and what happens to the space left over.
  let { gap = 2, wrap = false, justify = "start", children } = $props();

  const GAPS = { 1: "gap-1", 2: "gap-2", 3: "gap-3", 4: "gap-4", 6: "gap-6" };
  const JUSTIFY = {
    start: "justify-start",
    between: "justify-between",
    end: "justify-end",
    center: "justify-center",
  };
</script>

<div class="flex items-center {GAPS[gap]} {JUSTIFY[justify]}" class:flex-wrap={wrap}>
  {@render children()}
</div>
