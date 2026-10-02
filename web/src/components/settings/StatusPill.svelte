<script>
  let { label, tone = "neutral", accessibleLabel = undefined } = $props();
</script>

<span class="setting-status-pill" class:good={tone === "good"} class:warn={tone === "warn"} class:error={tone === "error"}
      role="status" aria-label={accessibleLabel || label}>
  <span class="setting-status-pill-dot" aria-hidden="true"></span>
  <span>{label}</span>
</span>
