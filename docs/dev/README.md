# Developer runbooks

- [Releasing](../releasing.md): version tags, artifacts and package channels.
  The librepaper.org runbook is private: `tools/deploy/runbook`.
- [Browser asset mirrors](asset-mirrors.md): pinning and publishing renderer
  and LaTeX assets before a release uses them.
- [CodeMirror fork](loro-codemirror.md): update the verified binding pin.
- [Cost policy](cost-policy.md) and [privacy duties](privacy-operators.md):
  deployment limits, storage, and account data.
- Protocols: [room](protocol/room-v2.md) (currently v3),
  [comments](protocol/comments-v1.md), and [assistant](protocol/chat.md).
- [Brand artwork](brand/README.md): editable and served assets.

These are repository references. The public site builds only pages listed in
`docs/nav.js`; `make site` does not publish `docs/dev/`.
