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
| `3.2.0` | `0.0.72` | 2026-11-11 | Run directory routes; `notices` on the job status |

## Run directory (3.2.0)

Pressing "Run code" on a Quarto document runs its code with a folder of the person's own as
its working directory, so `read.csv("data/x.csv")` and `source("R/helpers.R")`
work as they do in RStudio. The companion keeps one such folder, the Run
directory, per origin and project. It is stored on the computer only and never
uploaded. Before each Quarto render the companion links the folder's entries
into the project's hidden workspace (the project's own files win on a clash)
and runs Quarto with `--execute-dir <run directory>`. A Quarto render with no
Run directory is refused, and the page asks the person to choose one.

The routes authenticate like the `connections` routes: the bearer credential
and the `X-LibrePaper-Origin` header. A page calls them with `since: "3.2.0"`.
Each takes a project slug and answers a status, or a refusal as
`{"error": "..."}`.

| Route | Body | Does |
| --- | --- | --- |
| `GET run-directory?project=<slug>` | none | Answers the status. |
| `POST run-directory/choose` | `{"project"}` | Opens the native folder chooser titled "Choose the folder your code runs in", stores the folder and answers the status. Cancelling answers the current status. A chooser already open answers 409. |
| `POST run-directory/default` | `{"project"}` | Creates `<home>/LibrePaper/<project>`, stores it and answers the status. |
| `POST run-directory/open` | `{"project"}` | Opens the stored folder in the system file manager and answers the status. Answers 404 when none is set or it no longer exists. |

A missing project is a 400. The status is:

```json
{"project": "paper-abc", "path": "/home/me/Dropbox/research", "exists": true, "suggested": "/home/me/LibrePaper/paper-abc"}
```

`path` is `null` when none is set. `exists` is `false` when a path is stored
but is no longer a folder. `suggested` is always the default location, and is
not created by asking for it.

### Notices on the job status

A job status may carry `notices`, a list of `{"kind", "path"}`. The field is
left out when empty. `path` is relative to the workspace, with forward slashes.

- `clash`: the path is both a project file and an entry of the Run directory.
  The project's copy is the one in the workspace.
- `not-linked`: an entry of the Run directory could not be made available, for
  example on a Windows network drive.
