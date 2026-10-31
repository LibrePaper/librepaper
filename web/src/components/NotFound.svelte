<script>
  import Nav from "./Nav.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import { m } from "../lib/paraglide/messages.js";
</script>

<Nav />
<Page>
  <Stack gap={4}>
    <h1 class="h2">{m.notfound_title()}</h1>
    <p class="lp-text-secondary">
      {m.notfound_body()}
    </p>
    <a class="anchor" href="/">{m.notfound_back()}</a>
  </Stack>
</Page>
