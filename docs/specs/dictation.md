# SPEC: Dictation

Work nobody has asked for yet, and limitations kept on purpose. Each item
says which.

## Left open

- The dictation section of the settings panel is inside the reader's
  settings tab, which only editors can open. Readers who only comment cannot
  change the model.
- The local app could serve model files: one download shared by every
  browser on a machine and a pinned copy on disk. A new job kind and a CLI
  command for a small gain, and it excludes everyone without the app. Only
  worth revisiting together with the next item.
- Native inference in the local app. Several times faster than wasm on
  machines without WebGPU, and the only path to Parakeet TDT, Canary, and
  streaming models. It puts an inference engine into the single static
  binary on three platforms, must be gated so the public server never runs
  it, and needs a streaming addition to the local protocol. The capture and
  insertion code in `web/src/lib/dictation/` is what such a backend would
  reuse.
- Serving model files from the LibrePaper server, for self-hosters who want
  no dependency on Hugging Face. Not for the sandbox, whose hosting limits
  large files. A small addition when wanted.
- A streaming model as the default. Words while speaking is a better feeling
  than words after a pause, but the models that do it in the browser today
  are English-centric or weaker than Whisper small. Revisit when the catalog
  can take one without a second code path.

## References

- [Dictation capture](../../web/src/lib/dictation/) -- capture and insertion a native backend would reuse.
- [Local bridge protocol](../../crates/librepaper/src/local/protocol.rs) -- where a streaming addition would land.
