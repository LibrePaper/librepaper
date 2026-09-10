<script module>
  // Which DictationButton, if any, started the session the dictation
  // service is currently running. The service (SPEC-dictation.md 4.1,
  // docs/dictation-interfaces.md "service.js") exposes state but not whose
  // target it is bound to, and several of these buttons share the one
  // page-wide service instance, so ownership has to live somewhere all of
  // them can see: a module-level slot rather than a prop or a store, because
  // it is not data any component renders, only a fact each instance checks
  // about itself. Only one session is ever live at a time -- the service
  // stops the previous target before starting a new one -- so a single slot
  // (not a set) is enough.
  let owner = null;
</script>

<script>
  // A microphone next to a place text can land (SPEC-dictation.md 4.8: the
  // composer, comment, and reply buttons). This component owns none of the
  // dictation machinery -- that is `lib/dictation/service.js`, one instance
  // for the whole page -- it only knows how to draw the button for whichever
  // target its caller cares about, and how to tell whether *this* button is
  // the one that started the session currently running, by comparing itself
  // against the module-level `owner` slot above.
  import { onDestroy, onMount } from "svelte";
  import IconButton from "./IconButton.svelte";
  import { getDictation } from "../lib/dictation/service.js";

  // `onlistening` is optional: a caller that wants to change its own copy
  // (the composer's footer hint, SPEC 4.8) while this particular button is
  // the one running dictation subscribes to it instead of reaching into the
  // service itself, since "is it listening into *this* target" is exactly
  // the fact this component already tracks and the service does not expose.
  let { target, label = "Dictate", size = null, onlistening } = $props();

  // A unique token for this component instance, so `owner === self` can
  // tell "this button" apart from "some other DictationButton" without
  // comparing target objects (a fresh one is built on every click).
  const self = {};

  let snapshot = $state({ state: "idle", progress: null, reason: null });
  let owns = $state(false);

  const dictation = getDictation();
  let unsubscribe = null;

  onMount(() => {
    unsubscribe = dictation.subscribe((value) => {
      snapshot = value;
      if (value.state === "idle" || value.state === "unavailable") {
        if (owner === self) owner = null;
        owns = false;
      }
    });
  });

  onDestroy(() => {
    unsubscribe?.();
    if (owner === self) owner = null;
  });

  const listening = $derived(owns && (snapshot.state === "listening" || snapshot.state === "transcribing"));
  const loading = $derived(owns && snapshot.state === "loading");
  const unavailable = $derived(snapshot.state === "unavailable");

  const percent = $derived(
    snapshot.progress?.total ? Math.round((snapshot.progress.loaded / snapshot.progress.total) * 100) : null,
  );

  const title = $derived(
    unavailable
      ? snapshot.reason
      : loading
        ? percent === null
          ? "Loading the dictation model…"
          : `Loading the dictation model… ${percent}%`
        : listening
          ? "Listening… click or press Escape to stop"
          : label,
  );

  async function click() {
    if (loading) {
      await dictation.stop();
      return;
    }
    const built = target();
    owner = self;
    owns = true;
    await dictation.toggle(built);
  }

  // Escape stops dictation from the field it is typing into (SPEC 4.8). This
  // is deliberately scoped to "this button currently owns the session" --
  // the reader's own global Escape handling (a different worker's shortcut
  // work) is free to do whatever it does for every other case.
  function keydown(event) {
    if (event.key !== "Escape" || !owns) return;
    if (snapshot.state !== "listening" && snapshot.state !== "transcribing" && snapshot.state !== "loading") return;
    event.preventDefault();
    dictation.stop();
  }

  $effect(() => {
    if (!owns) return;
    document.addEventListener("keydown", keydown, true);
    return () => document.removeEventListener("keydown", keydown, true);
  });

  $effect(() => {
    onlistening?.(listening);
  });
</script>

<span class="dictation-button" class:dictation-listening={listening} class:dictation-loading={loading}>
  <IconButton icon="mic" {label} {title} {size} pressed={listening || loading} disabled={unavailable} onclick={click} />
</span>

<style>
  /* The pulsing ring is the one visible sign that a microphone the reader
     cannot see is actually recording -- SPEC 4.8 asks for "a pulsing state"
     using the primary colour token. It sits behind the button rather than
     changing the button's own colours, so the icon keeps reading as "mic",
     pressed or not. */
  .dictation-button {
    position: relative;
    display: inline-flex;
  }
  .dictation-listening::before,
  .dictation-loading::before {
    content: "";
    position: absolute;
    inset: -2px;
    border-radius: var(--radius-base);
    background: var(--color-primary-500);
    opacity: 0.35;
    animation: dictation-pulse 1.4s ease-in-out infinite;
    pointer-events: none;
  }
  .dictation-loading::before {
    animation-duration: 0.9s;
  }
  @keyframes dictation-pulse {
    0%,
    100% {
      opacity: 0.15;
    }
    50% {
      opacity: 0.45;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .dictation-listening::before,
    .dictation-loading::before {
      animation: none;
      opacity: 0.3;
    }
  }
</style>
