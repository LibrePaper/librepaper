# Komodoc

Host a self-contained HTML document on the web. Anyone with the (secret) link can highlight passages and leave comments on it.

* Multiple simultaneous readers, comments appear instantly
* Serverless: a Cloudflare Worker and a bucket, nothing to maintain
* Deploy in about two minutes

| | |
| --- | --- |
| [**Host**](#host) | Get the service running, on Cloudflare or your own machine |
| [**Publish**](#publish) | Share a document for others to annotate |
| [**Comment**](#comment) | What readers do with the link you send them |
| [**Export**](#export) | Take the annotations out |
| [**Manage**](#manage) | List, destroy, export |
| [**Reference**](#reference) | Every command and setting |

---

# Host

First get the binary. It is a single static file with everything inside it, and
needs nothing beside it.

```sh
curl -fsSL https://raw.githubusercontent.com/vincentarelbundock/komodoc/main/install.sh | sh
```

That fetches the release built for your machine (Linux and macOS, Intel and
Arm) and puts it in `~/.local/bin`. Set `KOMODOC_BIN_DIR` to put it elsewhere,
or `KOMODOC_VERSION=v0.0.1` to pin a version. Windows binaries are on the
[releases page](https://github.com/vincentarelbundock/komodoc/releases).

To build it yourself instead, you need [Go](https://go.dev/dl/):

```sh
go build -o dist/komodoc ./src
```

Then pick one: Cloudflare to put it on the internet, or your own machine to try it out.

## On Cloudflare

You need a Cloudflare account and a GitHub account. Five things to collect, then one command.

**1. A workers.dev subdomain.** At [dash.cloudflare.com](https://dash.cloudflare.com), open **Workers & Pages** → **Settings**, and set a subdomain in the right-hand sidebar. It is account-wide. Your site will live at `https://<name>.<subdomain>.workers.dev`.

**2. Storage.** Search **R2 Object Storage** in the dashboard, open it, and turn it on. The free tier is usually enough; Cloudflare may still ask for a card.

**3. A Cloudflare API token.** At [dash.cloudflare.com/profile/api-tokens](https://dash.cloudflare.com/profile/api-tokens) choose **Create Custom Token**, and give it three Account permissions:

* Workers Scripts: Edit
* Workers R2 Storage: Edit
* Account Settings: Read

**4. A GitHub OAuth app.** At [github.com/settings/developers](https://github.com/settings/developers) choose **New OAuth App**. The form asks for two URLs, both built from the `<name>` you are about to deploy under, so decide it now:

```
Homepage URL                  https://<name>.<subdomain>.workers.dev
Authorization callback URL    https://<name>.<subdomain>.workers.dev/auth/callback
```

The callback is the one that has to be exact; the homepage URL is only shown to people signing in. Name the app whatever you like. Keep the client id, and press **Generate a new client secret**.

**5. Deploy.**

```sh
export CLOUDFLARE_API_TOKEN="..."
export KOMODOC_GITHUB_CLIENT_ID="..." KOMODOC_GITHUB_CLIENT_SECRET="..."

./dist/komodoc deploy --name your-example --publishers your-github-login
# -> deployed: https://your-example.your-subdomain.workers.dev
```

That creates the bucket, uploads the site, and stores the secrets for you. you never install secrets by hand, and you never need to repeat this: a later `./dist/komodoc deploy` with no options at all keeps the client secret, the settings, and the key that signs sessions, so nobody is signed out.

Add `CLOUDFLARE_ACCOUNT_ID` only if your token can see more than one account; `deploy` lists them if so.

To change one setting later, pass just that one:

```sh
./dist/komodoc deploy --publishers alice,bob
```

## On your own machine

No Cloudflare account, no internet. Everything is kept in one directory: documents as files, comments as one JSON file each.

You still need a GitHub OAuth app (step 4 above), with both URLs pointing at your own machine:

```
Homepage URL                  http://localhost:8081
Authorization callback URL    http://localhost:8081/auth/callback
```

Use a separate app from the deployed one; GitHub matches the callback exactly.

```sh
export KOMODOC_GITHUB_CLIENT_ID="..." KOMODOC_GITHUB_CLIENT_SECRET="..."

./dist/komodoc serve --port 8081 --publishers your-github-login
# -> komodoc serving http://localhost:8081
```

Open that address in a browser. Stop it with Ctrl-C.

The port has to match the callback URL you registered, so pick one and keep it. Leave `--port` off and it takes the first free port between 8080 and 8099, which is convenient but means registering a callback for each.

Documents and comments go in `komodoc-data` beside you; `--data ~/somewhere-else` puts them anywhere you like. That directory is the only copy of your comments, so back it up. Sessions survive a restart.

To see the site with documents and comments already in it:

```sh
./dist/komodoc seed
```

---

# Publish

Sign in, then publish a self-contained HTML file:

```sh
./dist/komodoc login
export KOMODOC_ENDPOINT=https://your-example.your-subdomain.workers.dev
./dist/komodoc publish paper.html --title "My Paper"
# -> https://your-example.your-subdomain.workers.dev/docs/my-paper-k7f2q9xw3m
```

That URL is the share link. Its tail, `my-paper-k7f2q9xw3m`, is the document's slug, which every command that acts on one document takes; `./dist/komodoc list` prints them. Documents are unlisted rather than private, so treat the link as the secret.

Anything up to 30 MB with its images, styles and fonts inlined will do — `quarto render paper.qmd --to html -M embed-resources:true`, or any equivalent export. Documents keep their own JavaScript, so charts and maps still work. Markdown is rendered for you, and its first `#` heading becomes the title.

You can do the same thing without the terminal: sign in on the front page and drop a file on the upload field.

---

# Comment

Open the link, sign in with GitHub if the host asks you to, and comment on the document.

![The reader: the document on the left, comments on the right](docs/commenting.png)

---

# Export

Annotations come out in the [W3C Web Annotation Data Model](https://www.w3.org/TR/annotation-model/), which Hypothesis and other annotation tools read.

```sh
./dist/komodoc export my-paper-k7f2q9xw3m > annotations.json
./dist/komodoc export my-paper-k7f2q9xw3m --format markdown --out notes.md
```

Use `--format markdown` for something to read; the default JSON-LD is for feeding another tool. Highlights, suggested edits, figure regions, tags and replies all survive the trip.

---

# Manage

```sh
./dist/komodoc list                                   # documents and their slugs
./dist/komodoc export my-paper-k7f2q9xw3m             # annotations out
./dist/komodoc destroy --document my-paper-k7f2q9xw3m # one document and its comments
./dist/komodoc destroy --service                      # the whole deployment
```

Both `destroy` forms ask you to confirm, and neither can be undone.

---

# Reference

Ten commands. Run `./dist/komodoc` with no arguments for the same list.

| Command | What it does | Options |
| --- | --- | --- |
| `login` | Sign in with GitHub, by device flow | `--client-id`, `--endpoint` |
| `logout` | Forget the stored token | |
| `deploy` | Create or update the Cloudflare deployment | `--name`, `--client-id`, `--client-secret`, `--publishers`, `--commenters` |
| `serve` | Run the service on this machine | `--port` (default: first free of 8080-8099), `--data`, `--client-id`, `--client-secret`, `--publishers`, `--commenters` |
| `publish FILE` | Publish HTML or markdown, and print the link | `--title`, `--slug`, `--endpoint` |
| `list` | List your documents and their slugs | `--endpoint` |
| `export SLUG` | Annotations out | `--format jsonld\|markdown`, `--out`, `--endpoint` |
| `destroy` | Delete a document, or the whole deployment | `--document SLUG`, `--service`, `--name`, `--endpoint`, `--yes` |
| `seed` | Wipe a local data directory and fill it with examples | `--data` |
| `version` | Print the version | |

`destroy` takes `--document SLUG` or `--service` and refuses to guess between them; either way it asks you to confirm unless you pass `--yes`. `seed` writes to a directory on disk, so it applies to `serve` and not to a Cloudflare deployment.

| Variable | Used by | For |
| --- | --- | --- |
| `CLOUDFLARE_API_TOKEN` | `deploy`, `destroy --service` | Required |
| `CLOUDFLARE_ACCOUNT_ID` | `deploy`, `destroy --service` | Only if the token sees several accounts |
| `KOMODOC_ENDPOINT` | `login`, `publish`, `list`, `export`, `destroy --document` | Your deployment URL, instead of `--endpoint` |
| `KOMODOC_TOKEN` | `publish`, `list`, `export`, `destroy --document` | A GitHub token, instead of `login` |
| `KOMODOC_NAME` | `deploy`, `destroy --service` | Deployment name, same as `--name` |
| `KOMODOC_GITHUB_CLIENT_ID` | `deploy`, `serve` | OAuth app client id |
| `KOMODOC_GITHUB_CLIENT_SECRET` | `deploy`, `serve` | OAuth app client secret |
| `KOMODOC_PUBLISHERS` | `deploy`, `serve` | Same as `--publishers` |
| `KOMODOC_COMMENTERS` | `deploy`, `serve` | Same as `--commenters` |

The sign-in token is cached in `~/.config/komodoc/token`, or under `$XDG_CONFIG_HOME` if you set it.
