# SPEC: Directories

Work nobody has asked for yet, and limitations kept on purpose. Each item
says which.

## Left open

- The sandbox's `max_assets` is the default 32 MB. Nobody has measured what
  a public deployment on one disk can afford, so the number is still a
  default rather than a decision.
- A zip in. The editor hands back the directory as a zip; a zip dropped on
  the landing page -- the Overleaf habit -- would make `publish <directory>`
  reachable from the browser. Small, once the routes exist; not scheduled.

## References

- [Serving limits](../../crates/librepaper/src/server/serve.rs) -- where `max_assets` is set.
