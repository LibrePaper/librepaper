---
title: "Running a server"
---

## Deploy

LibrePaper is one static binary with everything compiled into it: the reader, the
renderers, and the server. Run it on your laptop for a quick trial, or on a
small host for something durable.

### Local

Run a public local instance with no GitHub setup at all:

```sh
librepaper admin serve --port 8081 --publishers YOUR-GITHUB-LOGIN
```

Open <http://localhost:8081>. Everything is stored in `librepaper-data` (see [Storage](./storage.md)).

### Self-managed server

Run the bundled server on your own host, with `--data-directory` set to a persistent
directory:

```sh
librepaper admin serve --port 8080 --data-directory /var/lib/librepaper --publishers YOUR-GITHUB-LOGIN
```

To let people sign in, set up a [GitHub app](./access.md) for this server's address.

Run the server behind a reverse proxy that terminates HTTPS, and have the proxy send the `X-Forwarded-Proto: https` header. That header is how the server knows its own address is an HTTPS one: without it the session cookie is not marked `Secure`, and uploads and comments are refused because the browser's idea of where the page came from does not match the server's. Plain HTTP is fine on `localhost` and nowhere else.

## Retention

Retention is off by default: a server started without `--document-expire-after`
keeps every document until somebody deletes it. That is the right default for a
server whose publishers you know, and the wrong one for a server anybody may
publish to. A public deployment that accepts uploads from strangers is durable
hosting for whatever they upload, so set a lifetime on it.

Delete documents automatically after their most recent publication:

```sh
librepaper admin serve --document-expire-after 24h
```

For a fixed lifetime from the first upload, use `--document-expire-from created`. Use
`--document-expire-after never` to disable expiry. Expired documents are removed by an
hourly pass, and once at startup.

## Privacy and the LaTeX mirror

Browsers download the LaTeX compiler distribution directly from the default
project mirror, `https://latex.librepaper.workers.dev/`. The mirror receives
the browser's IP address and the digest-named files it requests. Those
requests can reveal package choices and suggest a document's field or
template. Compiler downloads do not send document source or private input
assets to the mirror.

An operator can host a copy and keep compiler requests on their own
infrastructure by passing `--latex-mirror URL` to `librepaper admin serve`. The URL
must be an HTTPS static mirror with the documented mirror layout and headers.
