# SPEC: Directories

Work nobody has asked for yet, and limitations kept on purpose. Each item
says which.

## Left open

- The sandbox's `max_assets` is the default 32 MB. Nobody has measured what
  a public deployment on one disk can afford, so the number is still a
  default rather than a decision.

## References

- [Serving limits](../../crates/librepaper/src/server/serve.rs) -- where `max_assets` is set.
