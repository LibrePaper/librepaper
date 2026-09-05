<script>
  import Nav from "./Nav.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import { me as whoami } from "../lib/api.js";

  // The page the server shows when there is more than one way in. It is served
  // for /auth/login, so the choice is made here and every other page can link
  // to the one address without knowing what this deployment configured.
  let me = $state({});
  $effect(() => {
    whoami().then((who) => (me = who ?? {}));
  });

  // Carried through from the query rather than read off location.pathname:
  // this page is not where the person was going.
  const next = new URLSearchParams(location.search).get("next") || "/";
  const href = (provider) => `/auth/login/${provider}?next=${encodeURIComponent(next)}`;

  const NAMES = { github: "Sign in with GitHub", google: "Sign in with Google" };
</script>

<Nav {me} />
<Page>
  <Stack gap={4}>
    <h1 class="h2">Sign in</h1>
    <p class="text-surface-600-400">
      Signing in gives your comments a name that outlives this browser, and lets this deployment
      know what you may publish.
    </p>
    <Stack gap={2}>
      {#each me.providers ?? [] as provider (provider)}
        <a role="button" class="btn preset-filled-primary-500 w-fit" href={href(provider)}>
          {NAMES[provider] ?? provider}
        </a>
      {/each}
    </Stack>
  </Stack>
</Page>
