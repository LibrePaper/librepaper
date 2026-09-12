# SPEC: Signing in

Work nobody has asked for yet, and limitations kept on purpose. Each item
says which.

## Left open

- `Grant.login` in `document/store.rs` holds a handle, not a login, since
  providers arrived; rename the field to reflect provider-neutral handles.

## References

- [Document store](../../crates/librepaper/src/document/store.rs) -- `Grant.login`.
