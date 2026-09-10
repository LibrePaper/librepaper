# SPEC: Dictation

Status: design contract. This document specifies intended behavior, records
the alternatives that were rejected and why, and lists the implementation
gates. Nothing here is implemented. Written 2026-09-09.

## 1. Product objective

A person writing in LibrePaper should be able to press a microphone button or
a keyboard shortcut, speak, and see their words appear where they were typing:
in the chat, in a message to the agent, in a comment, in a reply, or in the
body of the document. It should work the same on Linux, macOS, and Windows, in
the free sandbox and on a self-hosted server, with nothing installed and
nothing configured beyond confirming a one-time model download.

Audio stays on the device. Recognition runs in the browser, in a worker, on a
model the browser has cached. No server operated by anyone sees the audio.

Operating system dictation was considered and rejected as the answer. It is
absent on Linux, uncertain about where audio goes on the other two platforms,
and its quality in a code editor is poor. It remains available to anyone who
prefers it, and this feature must not interfere with it.

### 1.1 Scope

The first release supports:

- A microphone button in the chat composer, which serves both the chat and the
  agent panel, and in the two comment forms.
- A global shortcut that dictates into whichever text input has focus,
  including the document editor.
- Pause-based segmentation: words appear a beat after the speaker pauses.
- A small catalog of models, all loaded through one recognizer library, with
  Whisper small as the default and a language override.
- A "browser built-in" backend using the Web Speech API for people who accept
  cloud recognition in exchange for no download. It is labeled as such.

Outside the first release:

- Native inference in the local app, or any use of the local app at all.
- Server-side inference, or serving model files from the LibrePaper server.
- Streaming models where words appear while the speaker is still talking.
- Translation, speaker identification, voice commands, and punctuation
  commands such as saying "comma".
- Mobile browsers. Nothing should break there, but nothing is tuned for them.

## 2. Existing architecture and required changes

| Existing component | Evidence | Proposed use or change |
| --- | --- | --- |
| Shared message input for chat and agent | [ChatComposer.svelte](web/src/components/ChatComposer.svelte), used by [Chat.svelte](web/src/components/Chat.svelte) and [Agent.svelte](web/src/components/Agent.svelte) | Add the microphone button to the footer. Transcribed text enters the draft through the existing `update` path so `ondraft` still fires. |
| Comment and suggestion dialog | [Reader.svelte](web/src/components/Reader.svelte), the `draft.body` and `draft.proposed` textareas | Add the button beside the comment textarea. The suggestion textarea is not a target in the first release. |
| Reply form on a comment | [CommentCard.svelte](web/src/components/CommentCard.svelte), `replyField` | Add the button beside the reply textarea. |
| Document editor | [Editor.svelte](web/src/components/Editor.svelte), which exposes `caret()`, `focus()`, and the CodeMirror view | Add an `insertAtCaret(text)` export that dispatches a change through the view, so the collaboration binding carries it to peers. |
| Reader window shortcuts | [Reader.svelte](web/src/components/Reader.svelte), `shortcut(event)` | Add the dictation toggle beside the layout shortcuts. |
| Worker with a message protocol | [latex/worker.js](web/src/lib/latex/worker.js) and [renderer-client.js](web/src/lib/renderer-client.js) | Follow the same request and reply shape for the recognizer worker. |
| Injected dependencies for Node checks | [latex/local.js](web/src/lib/latex/local.js), `deps` and `_testing.inject` | The dictation service, segmenter, and text assembly reach the browser only through `deps` so `web/checks` can run them under Node. |
| Settings panel | [Settings.svelte](web/src/components/Settings.svelte) | Add a dictation section: backend, model, language, and a button that clears the cached model. |
| Toasts | [Toasts.svelte](web/src/components/Toasts.svelte) | Report permission denial, missing WebAssembly, and download failure. |
| Design system check | [checks/vocabulary.js](web/checks/vocabulary.js) | The new components paint with theme tokens only. The check runs before every build and will reject anything else. |
| Vite build | [web/package.json](web/package.json) | Add the recognizer library as a dependency. Its ONNX Runtime wasm files are bundled and served from the LibrePaper origin, not from a CDN. |

No Rust change is required. No change to the local protocol is required.

## 3. Non-negotiable behavior

- Audio never leaves the device unless the user has chosen the "browser
  built-in" backend, whose description says that the browser may send audio to
  its vendor. The local backend never falls back to the built-in one silently.
- No model download starts without a confirmation that states the size. The
  confirmation appears once per model per browser.
- Text is inserted only into the input that was the target when dictation
  started. If that input is removed from the page or loses focus in a way that
  makes insertion ambiguous, dictation stops and the remaining text is
  discarded with a toast, never dropped into a different field.
- Insertion into the editor goes through the CodeMirror view so that
  collaborators, history, and track changes see an ordinary edit. Dictation
  never writes to the document through any other path.
- The microphone is released as soon as dictation stops. The browser's
  recording indicator must go out.
- Nothing is recorded. Audio buffers exist only until their segment has been
  transcribed, and the worker holds no audio after replying.
- Every failure is a recoverable state. A browser without WebAssembly, without
  a microphone, without a secure context, or with permission denied gets a
  toast that says what is missing. The button stays visible and disabled with
  the reason in its tooltip.
- The feature adds nothing to the initial page load beyond the button. The
  recognizer library, the worker, and the model load on first use.

## 4. Components

### 4.1 Dictation service

One module, `web/src/lib/dictation/service.js`, owns everything below and is
the only thing the components talk to. It holds a state machine:

| State | Meaning |
| --- | --- |
| `idle` | No microphone, no worker activity. |
| `loading` | Model download or worker start in progress. Progress is exposed. |
| `listening` | Microphone open, segmenter running, target bound. |
| `transcribing` | A segment is with the worker. Listening continues. |
| `unavailable` | A precondition failed. The reason is exposed. |

It exposes `start(target)`, `stop()`, `state`, `progress`, and a `subscribe`
for the pill and buttons. There is one instance per page. Starting while
another target is active stops the first target cleanly.

### 4.2 Capture

`getUserMedia` with `audio: { channelCount: 1, echoCancellation: true,
noiseSuppression: true, autoGainControl: true }`. An `AudioWorklet` delivers
frames of `Float32` samples. The worklet resamples to 16 kHz mono, which every
model in the catalog expects, and posts fixed-size frames of 512 samples,
which is 32 ms.

`AudioContext` construction and `getUserMedia` are reached through `deps` so
checks can feed synthetic audio.

### 4.3 Segmentation

The models in the catalog transcribe a finished clip. They do not consume a
stream. The segmenter turns frames into clips:

- A voice activity detector labels each frame speech or silence. The first
  release uses Silero VAD through the recognizer library, a model of about
  2 MB that runs in the same worker. If it cannot load, an energy threshold
  over the frame's RMS with a two-second calibration at start is the fallback.
- A segment opens at the first speech frame and includes 300 ms of pre-roll
  from a ring buffer, so a first syllable is not clipped.
- A segment closes after 600 ms of continuous silence, or when it reaches
  20 seconds, or when dictation stops. A closed segment shorter than 250 ms of
  speech is dropped.
- A closed segment is sent to the worker while the next one collects. Segments
  are transcribed and inserted in order; a slow segment delays the ones behind
  it rather than reordering them.

The thresholds are constants in one place with their units in the name.

### 4.4 Recognizer worker

`web/src/lib/dictation/worker.js` loads the recognizer library, Transformers.js,
and runs its automatic speech recognition pipeline. Requests are `{ id, kind,
... }` and replies carry the same `id`, following the LaTeX worker.

| Request | Effect |
| --- | --- |
| `load { model, device }` | Instantiate the pipeline. Reports `progress` messages with bytes loaded and total. |
| `transcribe { id, audio, language }` | Run one segment. Replies with `{ id, text, language }`. |
| `unload` | Dispose the pipeline and free memory. |

The worker tries WebGPU first and falls back to the wasm CPU backend, and the
reply to `load` says which one is in use so the settings panel can show it.

The pipeline is called with the options the model accepts. A table in the
worker maps each catalog entry to its options, because Whisper takes a
language and a task while Parakeet and the others ignore them.

Model files are fetched through the library's browser cache, which is Cache
Storage keyed by URL. Every catalog entry pins a repository revision so the URL
names exact bytes. After the first successful load the service requests
`navigator.storage.persist()` once.

### 4.5 Model catalog

`web/src/lib/dictation/models.js` lists what the settings panel offers. Sizes
are approximate and are shown to the user before download.

| Entry | Repository and revision | Download | Languages | Notes |
| --- | --- | --- | --- | --- |
| Whisper base | `onnx-community/whisper-base`, pinned | about 60 MB | 99 | The light option. |
| Whisper small | `onnx-community/whisper-small`, pinned | about 250 MB | 99 | Default. |
| Parakeet CTC 0.6B | `onnx-community` export of `nvidia/parakeet-ctc-0.6b`, pinned | about 600 MB | 25 European | Best accuracy on its languages. No language option. |
| Browser built-in | none | 0 | browser dependent | Web Speech API. Audio may go to the browser vendor. |

Entries carry id, revision, dtype, size, language list, whether the model
accepts a language option, whether it emits punctuation, and a short
description for the settings panel. Adding a model is adding a row.

The revisions are pinned when the catalog is written and moved only by a
commit that says why, in the spirit of `wasm-modules.lock`.

### 4.6 Text assembly

The worker returns raw text per segment. `web/src/lib/dictation/assemble.js`
turns segments into what is inserted, and is pure so it runs under Node.

- Leading and trailing whitespace is trimmed. Whisper's occasional bracketed
  noise tokens and empty segments are dropped.
- A space is inserted between the existing text before the caret and the new
  segment unless the caret is at the start of a line or follows whitespace or
  an opening bracket.
- If the text before the caret ends in sentence punctuation or is empty, and
  the model does not capitalize, the first letter is capitalized. Models that
  emit punctuation and casing are left alone.
- For models without punctuation, a segment that closed on a pause ends with
  no punctuation. The user adds it. Guessing periods produces worse text than
  none.
- In the editor, the segment is inserted as plain text. Nothing is escaped,
  because the user is dictating into Markdown, LaTeX, or Typst and can say the
  syntax they want.

### 4.7 Targets

A target is `{ insert(text), alive(), focus() }`.

- **Textarea target.** Inserts at the selection, replaces a selection if there
  is one, moves the caret after the inserted text, and dispatches an `input`
  event so Svelte's `bind:value` and the composer's `oninput` see the change.
- **Editor target.** Calls the editor's `insertAtCaret`, which dispatches a
  CodeMirror transaction replacing the main selection and scrolls the caret
  into view. Vim mode is not special-cased. Insertion happens in whatever mode
  the editor is in, which in normal mode is wrong. The first release stops
  dictation with a toast when the editor reports Vim normal mode, rather than
  inserting keystrokes.

`alive()` returns false once the element is detached, and the service checks
it before every insertion.

### 4.8 Surfaces

- **Composer button.** In the footer of the chat composer, before Send.
  Toggles. The composer's textarea is the target. While listening the button
  shows a pulsing state and the footer text changes to "Listening… click or
  Escape to stop".
- **Comment and reply buttons.** Same button beside the textarea, same
  behavior.
- **Shortcut.** Ctrl+Shift+D, with Cmd on macOS, registered in the reader's
  window handler. It toggles dictation into the active element if that is a
  textarea, a text input, or the editor. Otherwise a toast says "Click into a
  text field first". Escape stops dictation from anywhere.
- **Status pill.** A small fixed element near the bottom center of the window,
  present only while the service is not idle. It shows the state, download
  progress during `loading`, the model in use, and a stop button. It exists
  so the shortcut has visible feedback and so dictation started in a panel
  that later scrolls away can still be stopped.
- **Settings.** A dictation section with backend and model selection, a
  language dropdown defaulting to "Detect automatically", the backend actually
  in use after load, and "Remove downloaded model".

All new painting uses theme tokens. The pulsing state uses the primary color
token and an opacity animation.

### 4.9 Language

The pipeline receives a language when the model accepts one. The initial value
is the language the user picked in settings, or if that is "Detect
automatically", the first entry of `navigator.languages` that the model
supports, or auto-detection when there is none. Whisper's own detection needs
more audio than a dictation segment provides and mislabels short utterances,
so the browser's language is the guess and the settings override is the fix.

### 4.10 Settings keys

Stored in `localStorage`, plain, on the LibrePaper origin.

| Key | Values | Default |
| --- | --- | --- |
| `librepaper-dictation-backend` | `local`, `browser` | `local` |
| `librepaper-dictation-model` | a catalog id | `whisper-small` |
| `librepaper-dictation-language` | `auto` or a BCP 47 tag | `auto` |
| `librepaper-dictation-confirmed` | list of catalog ids whose download was confirmed | empty |

## 5. First use

1. The user presses the button. The service checks the preconditions in
   section 6 and shows a toast on the first failure.
2. If the chosen model is not in the confirmed list, a modal states the model,
   its size, that it downloads once, that it is stored by the browser, and
   that recognition then runs on this device. Confirm or cancel.
3. The pill appears in `loading` with a progress bar. The microphone is not
   opened until the model is ready, so the recording indicator is not lit
   during a download.
4. The microphone opens, the state moves to `listening`, and the button and
   pill show it.
5. The user speaks and pauses. Text lands. The user presses the button, the
   shortcut, or Escape. The microphone closes.

On the next use the model loads from cache in a second or two and the modal
does not appear.

## 6. Preconditions and failure modes

| Condition | Behavior |
| --- | --- |
| Not a secure context | Button disabled, tooltip "Dictation needs HTTPS or localhost". |
| No `WebAssembly` | Button disabled, tooltip says so. The built-in backend remains available. |
| No WebGPU | Silent. The wasm backend is used and the settings panel shows it. |
| No `getUserMedia` or no input device | Toast "No microphone found". |
| Permission denied | Toast "Microphone access was denied" with the browser's site settings as the fix. The denial is remembered for the session. |
| Download fails | Toast with the failure, state back to `idle`. The confirmation is not repeated on retry. |
| Cache evicted | The next use downloads again after the same modal, since the confirmed list survives only if `localStorage` did. |
| Worker crashes | Toast, state to `idle`, worker terminated. The next start creates a new one. |
| Target detached mid-dictation | Dictation stops, pending text dropped, toast says the field went away. |
| Editor in Vim normal mode | Dictation refuses to start and the toast says to enter insert mode. |

## 7. Privacy

The local backend's audio path is microphone, worklet, main thread buffer,
worker, and the transcript. No network request carries audio. The only
network activity is the model download, which goes to Hugging Face at a
pinned revision, and the recognizer library's runtime files, which are served
from the LibrePaper origin.

The built-in backend is a different product. Its settings description says
that the browser may send audio to its vendor for recognition and that
LibrePaper cannot see or control that.

Transcribed text is treated like typed text. It enters the same drafts,
comments, and documents, with the same persistence and sharing rules.

## 8. Testing

Node checks, run by `bun run check`, with injected dependencies:

- `checks/dictation-segmenter.mjs`: synthetic frames with known speech and
  silence spans produce segments with the specified pre-roll, close on the
  silence threshold, split at the length cap, and drop short segments.
- `checks/dictation-assemble.mjs`: spacing, capitalization, and noise-token
  rules against a table of before-caret text and segment pairs.
- `checks/dictation-service.mjs`: the state machine with a fake worker and a
  fake target, including a target that dies mid-segment, a worker that fails
  to load, and a permission denial.
- `checks/dictation-models.mjs`: every catalog entry has a pinned revision, a
  size, and a language list, and the options table in the worker covers every
  entry.

Browser check, run by `bun run check:browser`:

- `checks/dictation-browser.mjs`: with a fake `getUserMedia` returning a
  `MediaStream` from a short WAV file of a spoken sentence, and a tiny model,
  the composer button produces the sentence in the draft, and the shortcut
  produces it in the editor and the collaboration binding sees the change.

The vocabulary check covers the new components without changes.

## 9. Implementation gates

Each gate is a mergeable slice.

1. **Service, worker, catalog, and the composer button.** Whisper base only.
   Proves capture, segmentation, download, caching, and insertion end to end
   on one surface. Node checks for the segmenter, assembly, and service.
2. **Editor target and the shortcut.** The pill appears here because the
   shortcut needs it. The Vim mode refusal. The browser check.
3. **Comment and reply buttons.**
4. **Settings and the rest of the catalog.** Whisper small becomes the
   default. Parakeet CTC and the built-in backend appear. Language override.
5. **Polish.** Persist request, download resume after failure, keyboard
   focus order, and the documentation page.

## 10. Alternatives considered

- **Operating system dictation.** Rejected for the reasons in section 1.
  Documented as an option for people who like theirs.
- **The local app serving model files.** It would give one download shared by
  every browser on a machine and a pinned copy on disk. That is a small gain
  for a new job kind and a CLI command, and it excludes everyone without the
  app. Deferred, and only worth revisiting together with the next item.
- **Native inference in the local app.** Several times faster than wasm on
  machines without WebGPU, and the only path to Parakeet TDT, Canary, and
  streaming models. It puts an inference engine into the single static binary
  on three platforms, must be gated so the public server never runs it, and
  needs a streaming addition to the local protocol. Deferred. The capture and
  insertion code in this specification is what such a backend would reuse.
- **Serving model files from the LibrePaper server.** Useful for self-hosters
  who want no dependency on Hugging Face. Not for the sandbox, whose hosting
  limits large files. Deferred, and a small addition when wanted.
- **sherpa-onnx compiled to wasm.** Covers more architectures, including
  Parakeet TDT and streaming models, but is an opaque C++ runtime with its own
  model packaging and a history of crashes on exactly the Parakeet builds of
  interest. Transformers.js is readable JavaScript, maintained by Hugging Face,
  and loads models straight from their repositories.
- **A streaming model as the default.** Words while speaking is a better
  feeling than words after a pause. The models that do it in the browser today
  are English-centric or weaker than Whisper small. Revisit when the catalog
  can take one without a second code path.

## 11. Interface contract

The exact module boundaries, message shapes, and dependency injection points
are written in [docs/dictation-interfaces.md](docs/dictation-interfaces.md).
One deliberate deviation from sections 4.3 and 4.4: the voice activity
detector and the segmenter run inside the recognizer worker, which receives
raw frames, so the main thread only forwards audio and inserts text.
