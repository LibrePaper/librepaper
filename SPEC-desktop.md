# The desktop app

Companion features move out of the browser and into a desktop app. The browser keeps writing, collaborating and reading. Quarto, Calepin, agents, project folders, backups and Zotero work only in the desktop app.

This is a hard cutover. Nothing is released, so the browser pairing path is deleted, not kept behind a flag.

## Parts

- `desktop/` is a new Tauri 2 crate. It is a thin shell: one window that loads a LibrePaper server, plus a URL scheme that forwards companion requests. It is excluded from the Cargo workspace, because it links WebKitGTK on Linux and the workspace must stay buildable as static musl.
- `librepaper` is the existing binary. It still runs the companion daemon, the CLI and the server. The desktop app starts it and talks to it over loopback HTTP.
- The web app detects the desktop app and sends companion requests through the shell's scheme. Outside the desktop app it shows a notice instead of companion features.

## The local credential

The companion already writes a per-instance credential to `control-token.json` (mode 0600) in its state directory. That credential becomes the only way in.

- Every companion route except `GET /librepaper/local/health` requires `Authorization: Bearer <credential>`.
- The shell sends the origin of the page it is showing in `X-LibrePaper-Origin`. Handlers use this value wherever they used the browser `Origin` header before.
- A request that carries a browser `Origin` header or a `Sec-Fetch-Site` header is refused with 403. Browsers never talk to the companion directly anymore.
- Machine management routes (`/companion/api/*`) also require `X-LibrePaper-Origin` to equal the configured server's origin, as `ControlAuth::accepts_origin` does today.
- There is no CORS on the companion. The shell answers the page.

Deleted from the companion: `pair/request`, `connect/claim`, `pair/status`, `disconnect`, `companion/api/session`, `companion/api/pairings/<id>`, `consent.rs`, the pairing tokens in `PairingStore`, pending pairs, connect rate limits, CORS headers, `librepaper local disconnect`, and pairing lines in `librepaper local status`. `PairingStore` keeps only what `service.json` needs, or that moves elsewhere. Per-document execution grants and folder grants stay exactly as they are. `librepaper local approve <code>` is removed too.

## The endpoint command

`librepaper local endpoint` (hidden) starts the local service in the background if it is not running, waits until `health` answers, and prints one line of JSON:

```json
{"address":"http://127.0.0.1:8763/","token":"<credential>","instance":"<id>","version":"0.0.42","protocol":[2]}
```

It exits non-zero with a message on stderr when the local service cannot start. The shell runs it at startup and again whenever a forwarded request fails to connect or gets 401. Tests that drive a real companion use it the same way.

## Links

The `librepaper` binary handles `librepaper://` links. `librepaper local open` is removed. The binary now accepts one form: `librepaper://open?url=<https URL>`. It launches the desktop app with `--open <URL>`. Every other form is refused. The pairing-link handling is deleted.


The desktop app is found next to the running `librepaper` executable, then on `PATH`, under the name `librepaper-desktop` (`librepaper-desktop.exe` on Windows; on macOS also `/Applications/LibrePaper.app/Contents/MacOS/librepaper-desktop`).

## The shell

- **Servers.** `desktop.json` in the app config directory holds `{"servers": ["https://app.librepaper.org/"]}`. The first entry opens at launch. `LIBREPAPER_SERVER` replaces the default for development. `--server <URL>` adds a server after a native confirmation dialog and opens it.
- **Window.** One window titled LibrePaper, 1280 by 860, loading the first server. Navigation to any https URL is allowed in the window, so OAuth sign-in works. Only the listed servers get companion access.
- **Scheme.** The shell registers the `lp-local` scheme. For each request it takes the origin of the window's current URL. If that origin is not a listed server, it answers 403. Otherwise it forwards method, path, query, body and content type to the companion address with the bearer credential and `X-LibrePaper-Origin`. It returns status, body and the `content-type`, `cache-control`, `content-disposition` and `etag` headers, and adds `Access-Control-Allow-Origin: <origin>` and `Vary: Origin`. It answers `OPTIONS` itself with 204 and allows `GET, POST, PUT, DELETE` and the `content-type` header. On a connect error or 401 it reruns the endpoint command once and retries. When the companion cannot be reached, it answers 502 with `{"error":"companion unavailable","detail":"..."}`.
- **Page global.** An initialization script defines `window.__LIBREPAPER_DESKTOP__ = Object.freeze({ base, version })`. `base` is the scheme base URL for this platform: `lp-local://localhost/` on Linux and macOS, `http://lp-local.localhost/` on Windows.
- **Arguments.** `--open <URL>` navigates the window to that URL if its origin is a listed server. `--settings` navigates to the first server with `#settings=local`. A second launch forwards its arguments to the running instance through the single-instance plugin.
- **The binary.** `librepaper` is found the same way the companion finds the desktop app, reversed: next to the shell executable, then on `PATH`. `LIBREPAPER_BIN` overrides it.

## The web app

`web/src/lib/companion/client.js` keeps its feature functions: builds, previews, Quarto, Calepin, agents, Zotero, folders, backups and `manage`. Its connection layer changes.

- `desktop()` returns `window.__LIBREPAPER_DESKTOP__` or `null`.
- Requests go to `desktop().base + "librepaper/local/<route>"` and `desktop().base + "companion/api/<route>"`, with no token.
- The address, pairing, probe backoff, connect flow, start grace, auto-reconnect, negative cache, `companion_address` fragment and every localStorage key for them are deleted.
- Status states are `browser` (not in the desktop app), `checking`, `ready`, `unavailable` (the shell answered 502) and `incompatible` (the protocol does not match). The shell checks the protocol the service prints, and an incompatible service makes the bridge answer 502 with a message to reinstall LibrePaper.
- In the browser, every place that offered companion features shows one notice: "Computing, agents, folders and backups need the LibrePaper desktop app." It links to the install page and offers "Open in the desktop app" (`librepaper://open?url=<current URL>`).

## The server

The companion embedded in `librepaper admin serve` (`local::embedded`, the `local_companion` setting, `StartLocal`, `LocalApp` and the advertised local address) is deleted. A browser cannot reach it anymore, and a person on the server's own machine uses the desktop app.

## Build and release

- `make desktop` builds the shell in release mode. On NixOS it runs inside `nix-shell -p pkg-config webkitgtk_4_1 gtk3 libsoup_3 glib-networking`.
- Signing, notarization, the updater and installers are not in this change. They need accounts and keys.

## Not in this change

- Caching the unpacked TeX engine state to speed up the first LaTeX compile in WebKitGTK.
- A server picker beyond `--server`.
- macOS testing.

## Retiring the companion

"Companion" is retired as a user-facing word. In this spec, "companion" still names the local service until the code is renamed.

- Users install one thing: the LibrePaper desktop app.
- The desktop app runs the local service in the background. The service ships inside the app as the `librepaper` binary.
- Users never start, stop or pair the local service.
- The tray icon, start at login and `librepaper install-desktop` are removed.
- The public CLI is `login`, `logout`, `list`, `export`, `agent` and `admin`.
- `librepaper` with no arguments, `start`, `stop`, `status`, `--at-login`, `--foreground`, `--port`, `local approve` and `local open` are removed.
- `librepaper local endpoint` stays as a hidden command for the desktop app.
- In a browser, features that need the desktop app say so and link to the install page.
- The docs move to `docs/desktop-app.md` and `docs/architecture/local-service.md`.
