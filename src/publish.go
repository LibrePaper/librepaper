package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
	"unicode/utf8"
)

func endpointFrom(flagValue string) string {
	endpoint := flagValue
	if endpoint == "" {
		endpoint = os.Getenv("KOMODOC_ENDPOINT")
	}
	if endpoint == "" {
		die("set --endpoint or $KOMODOC_ENDPOINT")
	}
	return strings.TrimRight(endpoint, "/")
}

func publish(file, title, slug, endpointFlag string) {
	info, err := os.Stat(file)
	if err != nil || info.IsDir() {
		die("file not found: %s", file)
	}
	// What is stored is always HTML: the reader frames the document and
	// anchors comments into its text nodes. Markdown is rendered here, before
	// it is uploaded, so it works against any backend.
	extension := strings.ToLower(filepath.Ext(file))
	if extension != ".html" && extension != ".htm" && !isMarkdown(file) {
		die("%s is not a document Komodoc can serve.\n\n"+
			"  It takes HTML, or markdown which it renders for you. From Quarto:\n"+
			"    quarto render paper.qmd --to html -M embed-resources:true",
			filepath.Base(file))
	}
	raw, err := os.ReadFile(file)
	if err != nil {
		die("could not read %s: %v", file, err)
	}
	if !utf8.Valid(raw) {
		die("%s is not valid UTF-8 text", filepath.Base(file))
	}
	if len(raw) > config.MaxHTML {
		die("document exceeds the %d MB limit", config.MaxHTML/(1024*1024))
	}
	html := string(raw)

	if isMarkdown(file) {
		if title == "" {
			// The first heading names the document, before the filename does.
			title = titleFromMarkdown(html)
		}
		rendered, err := renderMarkdownDocument(html, titleOr(title, file))
		if err != nil {
			die("could not render %s: %v", filepath.Base(file), err)
		}
		fmt.Fprintf(os.Stderr, "rendered %s (%d KiB of markdown)\n",
			filepath.Base(file), len(raw)/1024)
		html = rendered
	} else if !strings.Contains(html, "<") {
		die("%s contains no HTML tags", filepath.Base(file))
	}

	endpoint := endpointFrom(endpointFlag)
	if title == "" && slug != "" {
		// Publishing a revision: keep the title the document already has rather
		// than silently renaming it after the file on disk.
		status, body := do("GET", endpoint+"/api/documents/"+slug, nil, nil, 30*time.Second)
		if status == 200 {
			var existing struct {
				Title string `json:"title"`
			}
			if err := json.Unmarshal(body, &existing); err == nil {
				title = existing.Title
			}
		}
	}
	if title == "" {
		stem := strings.TrimSuffix(filepath.Base(file), filepath.Ext(file))
		title = strings.TrimSpace(strings.NewReplacer("_", " ", "-", " ").Replace(stem))
	}

	status, document := postAuthed(endpoint+"/api/documents", map[string]string{
		"title": title,
		"slug":  slug,
		"html":  html,
	}, requireToken(), 300*time.Second)
	if status != 201 {
		die("upload failed (%d): %v", status, detailOf(document))
	}

	link := endpoint + text(document["url"])
	fmt.Println(link)
	if isTerminal(os.Stdout) {
		fmt.Fprintf(os.Stderr,
			"\nShare this link; anyone with it can comment, no account needed."+
				"\nTo publish a revision to the same link:"+
				"\n  %s publish %s --slug %s\n",
			os.Args[0], file, text(document["slug"]))
	}
}

func listDocuments(endpointFlag string) {
	endpoint := endpointFrom(endpointFlag)
	status, payload := postAuthed(endpoint+"/api/list", map[string]any{}, requireToken(), 60*time.Second)
	if status != 200 {
		die("listing failed (%d): %v", status, detailOf(payload))
	}

	documents, _ := payload["documents"].([]any)
	if len(documents) == 0 {
		fmt.Println("no documents yet")
		return
	}
	width := 0
	for _, entry := range documents {
		if document, ok := entry.(map[string]any); ok {
			if size := len(text(document["slug"])); size > width {
				width = size
			}
		}
	}
	for _, entry := range documents {
		document, ok := entry.(map[string]any)
		if !ok {
			continue
		}
		updated := text(document["updated_at"])
		if len(updated) > 10 {
			updated = updated[:10]
		}
		fmt.Printf("%-*s  %s  %s\n", width, text(document["slug"]), updated, text(document["title"]))
	}
}

// sortByUpdated orders documents oldest first, as the destroy listing does.
func sortByUpdated(documents []map[string]any) {
	sort.SliceStable(documents, func(i, j int) bool {
		return text(documents[i]["updated_at"]) < text(documents[j]["updated_at"])
	})
}

// titleOr falls back to the filename, the way an untitled document is named.
func titleOr(title, file string) string {
	if strings.TrimSpace(title) != "" {
		return title
	}
	stem := strings.TrimSuffix(filepath.Base(file), filepath.Ext(file))
	return strings.TrimSpace(strings.NewReplacer("_", " ", "-", " ").Replace(stem))
}
