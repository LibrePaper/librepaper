package main

import (
	"bytes"
	"encoding/json"
	"io"
	"mime/multipart"
	"net/http"
	"strings"
	"testing"
)

func TestRenderMarkdownDocument(t *testing.T) {
	source := `# A Paper

Some *emphasis*, a [link](https://example.test) and a footnote.[^1]

| a | b |
| - | - |
| 1 | 2 |

~~struck~~ and ` + "`code`" + `

[^1]: the note
`
	out, err := renderMarkdownDocument(source, "A Paper")
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{
		"<!doctype html>",
		"<title>A Paper</title>",
		`<h1 id="a-paper">A Paper</h1>`, // heading ids, for links into the document
		"<em>emphasis</em>",
		`<a href="https://example.test">link</a>`,
		"<table>", // GFM
		"<del>",   // GFM strikethrough
		"<code>",
		"footnote",
	} {
		if !strings.Contains(out, want) {
			t.Errorf("rendered document is missing %q", want)
		}
	}
	// Self-contained: nothing to fetch, so a published document stands alone.
	if strings.Contains(out, "<script") || strings.Contains(out, `<link rel="stylesheet"`) {
		t.Error("the rendered document should reference nothing external")
	}
}

func TestMarkdownTitleAndDetection(t *testing.T) {
	if got := titleFromMarkdown("intro\n\n# The Title\n\nbody\n\n# Later\n"); got != "The Title" {
		t.Errorf("titleFromMarkdown gave %q", got)
	}
	if got := titleFromMarkdown("no headings here"); got != "" {
		t.Errorf("titleFromMarkdown gave %q, want empty", got)
	}
	for name, want := range map[string]bool{
		"paper.md": true, "PAPER.MD": true, "notes.markdown": true,
		"paper.html": false, "paper.htm": false, "md": false, "paper.md.html": false,
	} {
		if isMarkdown(name) != want {
			t.Errorf("isMarkdown(%q) = %v", name, !want)
		}
	}
}

func TestMarkdownUploadIsRenderedByServe(t *testing.T) {
	server, _ := newTestServer(t)

	body, contentType := multipartUpload(t, "notes.md", "# Notes\n\nA *point* worth making.\n")
	request, err := newRequest("POST", server.URL+"/api/documents", body, contentType)
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("cookie", sessionAs(testPublisher))
	request.Header.Set("X-Komodoc-Client", "1")
	response, err := client().Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != 201 {
		t.Fatalf("markdown upload returned %d", response.StatusCode)
	}

	document := decode(t, response)
	// The first heading names it when no title was given.
	if document["title"] != "Notes" {
		t.Fatalf("title came out as %v, want the first heading", document["title"])
	}
	// What is stored is HTML, not the markdown source.
	stored := onDocsHost(t, server,
		"/raw/"+text(document["slug"])+"/"+text(document["sha"])+".html")
	defer stored.Body.Close()
	page := read(t, stored)
	if !strings.Contains(page, "<em>point</em>") || strings.Contains(page, "# Notes") {
		t.Fatalf("the stored document is not rendered HTML: %q", page)
	}
}

// --- small helpers, kept here so the upload test reads as one thing --------

func multipartUpload(t *testing.T, filename, content string) (*bytes.Buffer, string) {
	t.Helper()
	body := &bytes.Buffer{}
	writer := multipart.NewWriter(body)
	part, err := writer.CreateFormFile("file", filename)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := io.WriteString(part, content); err != nil {
		t.Fatal(err)
	}
	_ = writer.Close()
	return body, writer.FormDataContentType()
}

func newRequest(method, url string, body *bytes.Buffer, contentType string) (*http.Request, error) {
	request, err := http.NewRequest(method, url, body)
	if err != nil {
		return nil, err
	}
	request.Header.Set("content-type", contentType)
	return request, nil
}

func client() *http.Client { return http.DefaultClient }

func decode(t *testing.T, response *http.Response) map[string]any {
	t.Helper()
	var payload map[string]any
	if err := json.NewDecoder(response.Body).Decode(&payload); err != nil {
		t.Fatal(err)
	}
	return payload
}

func read(t *testing.T, response *http.Response) string {
	t.Helper()
	raw, err := io.ReadAll(response.Body)
	if err != nil {
		t.Fatal(err)
	}
	return string(raw)
}
