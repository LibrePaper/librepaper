package main

import (
	"bytes"
	"fmt"
	"html"
	"regexp"
	"strings"

	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/parser"
	gmhtml "github.com/yuin/goldmark/renderer/html"
)

// Markdown is rendered here, in Go, and what gets stored is the resulting
// HTML. Rendering at publish time rather than per request keeps one thing
// true that everything else depends on: a document is HTML, addressed by the
// hash of the bytes actually served. The reader anchors comments into those
// bytes, so they must not change underneath a comment.
//
// It also means markdown works against Cloudflare, where no Go runs: the
// rendering has already happened by the time anything is uploaded.

var markdown = goldmark.New(
	goldmark.WithExtensions(
		extension.GFM, // tables, strikethrough, task lists, autolinks
		extension.Footnote,
		extension.Typographer, // quotes and dashes
	),
	goldmark.WithParserOptions(
		parser.WithAutoHeadingID(), // stable anchors for links into the document
	),
	goldmark.WithRendererOptions(
		// The document is served on its own origin and framed; raw HTML in the
		// source is the author's own, and no more dangerous than the markdown
		// around it.
		gmhtml.WithUnsafe(),
	),
)

var reFirstHeading = regexp.MustCompile(`(?m)^#\s+(.+?)\s*$`)

// isMarkdown says whether a filename is one this renders.
func isMarkdown(name string) bool {
	lower := strings.ToLower(name)
	return strings.HasSuffix(lower, ".md") || strings.HasSuffix(lower, ".markdown")
}

// titleFromMarkdown returns the first level-one heading, which is the obvious
// title when none was given.
func titleFromMarkdown(source string) string {
	if match := reFirstHeading.FindStringSubmatch(source); match != nil {
		return strings.TrimSpace(match[1])
	}
	return ""
}

// renderMarkdownDocument turns a markdown source into a standalone HTML page.
func renderMarkdownDocument(source, title string) (string, error) {
	var body bytes.Buffer
	if err := markdown.Convert([]byte(source), &body); err != nil {
		return "", err
	}
	return fmt.Sprintf(markdownTemplate, html.EscapeString(title), body.String()), nil
}

// A plain, readable page: no webfonts, no scripts, nothing to fetch. The
// styles are inline because a published document has to stand on its own.
const markdownTemplate = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>%s</title>
<style>
  :root { color-scheme: light }
  body {
    max-width: 46rem;
    margin: 3rem auto;
    padding: 0 1.25rem 4rem;
    background: #fff;
    color: #1a1d21;
    font: 16px/1.7 system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  }
  h1, h2, h3, h4, h5, h6 { line-height: 1.25; margin: 2.2rem 0 0.8rem; font-weight: 650 }
  h1 { font-size: 2rem; margin-top: 0 }
  h2 { font-size: 1.5rem }
  h3 { font-size: 1.2rem }
  p, ul, ol, blockquote, table, pre { margin: 0 0 1.1rem }
  a { color: #2f5bd0 }
  code {
    background: #f2f4f7;
    border-radius: 4px;
    padding: 0.1em 0.35em;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 0.9em;
  }
  pre { background: #f2f4f7; border-radius: 8px; padding: 0.9rem 1rem; overflow-x: auto }
  pre code { background: none; padding: 0 }
  blockquote {
    margin-left: 0;
    padding-left: 1rem;
    border-left: 3px solid #d7dbe2;
    color: #4a5058;
  }
  table { border-collapse: collapse; width: 100%% }
  th, td { border: 1px solid #d7dbe2; padding: 0.45rem 0.7rem; text-align: left }
  th { background: #f7f8fa }
  img { max-width: 100%% }
  hr { border: 0; border-top: 1px solid #d7dbe2; margin: 2rem 0 }
</style>
</head>
<body>
%s</body>
</html>
`
