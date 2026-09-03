package main

import (
	"bytes"
	"embed"
	"encoding/base64"
	"encoding/json"
	"path"
	"regexp"
	"strings"
)

// The Cloudflare Worker: routing and uploads in worker.js, the Room Durable
// Object in room.js. They are concatenated into one module at deploy time.
//
//go:embed worker/room.js worker/worker.js
var workerFS embed.FS

// The reader shell: real .html, .css and .js files, compiled into the Worker.
//
//go:embed shell
var shellFS embed.FS

// The wordmark's typeface, complete and unmodified, under the OFL text beside
// it. It is served from its own route rather than inlined, so it can be cached
// for a year while the stylesheet stays short-lived.
//
//go:embed shell/vendor/fonts/IBMPlexSans-SemiBold.woff2
var wordmarkFont []byte

// workerModules is ordered: room.js defines and exports the Room class, so
// putting it first makes worker.js's re-export resolve without a second module
// part in the upload.
var workerModules = []string{"room.js", "worker.js"}

// shellRoutes maps a request path to the file that answers it.
var shellRoutes = map[string]string{
	// Pico CSS, vendored under vendor/ so the shell keeps its no-build,
	// no-CDN property. See vendor/pico-LICENSE.md.
	"/pico.css":               "vendor/pico.css",
	"/index.html":             "index.html",
	"/reader.html":            "reader.html",
	"/komodoc.css":            "komodoc.css",
	"/reader.js":              "reader.js",
	"/agent.js":               "agent.js",
	"/anchor.js":              "anchor.js",
	"/documentation":          "documentation.html",
	"/assets/komodo-logo.svg": "assets/komodo-logo.svg",
	"/docs/commenting.png":    "assets/commenting.png",
	"/docs/sandbox.svg":       "assets/sandbox.svg",
}

var contentTypes = map[string]string{
	".html": "text/html; charset=utf-8",
	".css":  "text/css; charset=utf-8",
	".js":   "text/javascript; charset=utf-8",
	".png":  "image/png",
	".svg":  "image/svg+xml",
}

type shellFile struct {
	Type string `json:"type"`
	Body string `json:"body"`
	// Base64 says the body is an encoded binary rather than text. The Worker
	// carries the shell as JSON, which cannot hold arbitrary bytes, so a font
	// travels encoded and is decoded when it is served.
	Base64 bool `json:"base64,omitempty"`
	// Immutable marks a file whose bytes never change, so it can be cached
	// for a year instead of five minutes.
	Immutable bool `json:"immutable,omitempty"`
}

// fontRoute is served on its own, rather than inlined into the stylesheet,
// because a font never changes and a stylesheet does. Sharing one cache
// lifetime would have meant refetching the font whenever the CSS expired.
const fontRoute = "/fonts/ibm-plex-sans-600.woff2"

// The first heading of the rendered README, which the hero replaces.
var reLeadingTitle = regexp.MustCompile(`(?s)\A\s*<h1[^>]*>.*?</h1>`)

var reExport = regexp.MustCompile(`(?m)^export \{ Room \} from "\./room\.js";\n`)

// documentation renders the project README into the HTML the documentation
// page is built around. The README is the only copy of that text: it is
// embedded from the shell directory, where the build puts it, so the page
// cannot drift from what the repository says.
func documentation() string {
	source, err := shellFS.ReadFile("shell/README.md")
	if err != nil {
		die("missing README.md in the shell: run make build, which copies it in")
	}
	var body bytes.Buffer
	if err := markdown.Convert(source, &body); err != nil {
		die("could not render README.md: %v", err)
	}
	// The page opens with the mark and the wordmark, as the landing page does,
	// so the README's own title would say the name twice.
	return reLeadingTitle.ReplaceAllString(body.String(), "")
}

func loadShell() map[string]shellFile {
	settings, err := json.Marshal(config)
	if err != nil {
		die("could not encode the configuration: %v", err)
	}
	prose := documentation()
	shell := make(map[string]shellFile, len(shellRoutes))
	for route, name := range shellRoutes {
		body, err := shellFS.ReadFile("shell/" + name)
		if err != nil {
			die("missing shell file: %s", name)
		}
		if path.Ext(name) == ".png" {
			shell[route] = shellFile{
				Type:      contentTypes[".png"],
				Body:      base64.StdEncoding.EncodeToString(body),
				Base64:    true,
				Immutable: true,
			}
			continue
		}
		// The page checks the same size and file-type limits the server does,
		// so __CONFIG__ is substituted here too rather than duplicated in HTML.
		source := strings.ReplaceAll(string(body), "__CONFIG__", string(settings))
		// After the configuration, so nothing in the README is mistaken for a
		// placeholder the shell was meant to fill in.
		source = strings.ReplaceAll(source, "__README__", prose)
		shell[route] = shellFile{Type: contentTypes[path.Ext(name)], Body: source}
	}
	shell[fontRoute] = shellFile{
		Type:      "font/woff2",
		Body:      base64.StdEncoding.EncodeToString(wordmarkFont),
		Base64:    true,
		Immutable: true,
	}
	return shell
}

func loadWorker() string {
	parts := make([]string, 0, len(workerModules))
	for _, name := range workerModules {
		source, err := workerFS.ReadFile("worker/" + name)
		if err != nil {
			die("missing worker file: %s", name)
		}
		parts = append(parts, strings.Trim(reExport.ReplaceAllString(string(source), ""), "\n"))
	}
	return strings.Join(parts, "\n\n") + "\n"
}

// workerSource is the Worker module, with the reader shell and the shared
// limits from config.go compiled into it.
func workerSource() string {
	shell, err := json.Marshal(loadShell())
	if err != nil {
		die("could not encode the shell: %v", err)
	}
	settings, err := json.Marshal(config)
	if err != nil {
		die("could not encode the configuration: %v", err)
	}
	source := strings.Replace(loadWorker(), "__SHELL__", string(shell), 1)
	return strings.Replace(source, "__CONFIG__", string(settings), 1)
}
