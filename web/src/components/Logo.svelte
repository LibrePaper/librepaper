<script>
  let { size = "bar" } = $props();
</script>

<img class="logo {size}" src="/librepaper-logo.svg" alt="LibrePaper" />

<style>
  .logo {
    display: block;
    width: auto;
    aspect-ratio: 1254 / 1254;
  }
  .bar {
    height: 1.4rem;
  }
  @media (max-width: 760px) {
    .bar {
      height: 1.05rem;
    }
  }
  .hero {
    height: 3rem;
  }
</style>
