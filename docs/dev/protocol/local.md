# Local helper protocol

The local helper (`librepaper local start`) and the desktop app agree on a
wire version. It is separate from the release version people install.

The wire version uses [semantic versioning](https://semver.org/),
`MAJOR.MINOR.PATCH`.

- A major goes up on a breaking change, such as a removed or renamed field
  or route. The helper keeps older majors working through migrations.
- A minor goes up on an addition, such as a new optional field or route. It
  goes back to 0 when the major goes up.
- A patch goes up on a fix that a page must rely on and that changes no
  shape. It goes back to 0 when the major or minor goes up.

The helper answers every major whose successor came out less than six months
ago. An older major stops being answered after that.

For the full rules, see [SPEC-wire.md](../../../SPEC-wire.md).

| Version | Release | Date | Changes |
| --- | --- | --- | --- |
| `3.0.0` | `0.0.52 or later` | 2026-10-25 | Render pipeline replaces typed builders; shapes changed after this without a bump |
| `3.1.0` | `0.0.66` | 2026-11-04 | Version published in the page global; lenient decoding of non-confirmed request bodies |
