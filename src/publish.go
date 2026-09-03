package main

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
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

	// storedToken, not requireToken: a deployment whose publishers are
	// "anyone" takes documents with no sign-in, and one that does need an
	// account answers with its own message.
	status, document := postAuthed(endpoint+"/api/documents", map[string]string{
		"title": title,
		"slug":  slug,
		"html":  html,
	}, storedToken(), 300*time.Second)
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
	ids := shortIDs(documents)
	width := 0
	for _, id := range ids {
		if len(id) > width {
			width = len(id)
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
		slug := text(document["slug"])
		fmt.Printf("%-*s  %s  %s\n", width, ids[slug], updated, text(document["title"]))
	}
}

// shortIDs gives each listed document the shortest prefix of its generated
// random suffix (or explicit slug) that is unique within this listing.
func shortIDs(documents []any) map[string]string {
	type item struct{ slug, key string }
	items := make([]item, 0, len(documents))
	for _, value := range documents {
		document, ok := value.(map[string]any)
		if !ok {
			continue
		}
		slug := text(document["slug"])
		parts := strings.Split(slug, "-")
		key := slug
		if last := parts[len(parts)-1]; len(last) == config.SuffixLength && strings.IndexFunc(last, func(r rune) bool { return !strings.ContainsRune(config.SuffixAlphabet, r) }) < 0 {
			key = last
		}
		items = append(items, item{slug: slug, key: key})
	}
	ids := make(map[string]string, len(items))
	for _, current := range items {
		chosen := current.key
		for length := 1; length <= len(current.key); length++ {
			prefix := current.key[:length]
			matches := 0
			for _, other := range items {
				if strings.HasPrefix(other.key, prefix) {
					matches++
				}
			}
			if matches == 1 {
				chosen = prefix
				break
			}
		}
		ids[current.slug] = chosen
	}
	return ids
}

func commentDocument(identifier, endpointFlag string) {
	endpoint := endpointFrom(endpointFlag)
	status, payload := postAuthed(endpoint+"/api/list", map[string]any{}, requireToken(), 60*time.Second)
	if status != 200 {
		die("listing failed (%d): %v", status, detailOf(payload))
	}
	documents, _ := payload["documents"].([]any)
	ids := shortIDs(documents)
	var match map[string]any
	for _, value := range documents {
		document, ok := value.(map[string]any)
		if !ok {
			continue
		}
		slug := text(document["slug"])
		if identifier == slug || identifier == ids[slug] {
			if match != nil {
				die("%q matches more than one document", identifier)
			}
			match = document
		}
	}
	if match == nil {
		die("no visible document matches %q", identifier)
	}
	slug := text(match["slug"])
	openURL(endpoint + "/docs/" + slug)
}

func openURL(target string) {
	var command string
	switch runtime.GOOS {
	case "darwin":
		command = "open"
	case "windows":
		command = "rundll32"
	default:
		command = "xdg-open"
	}
	args := []string{target}
	if runtime.GOOS == "windows" {
		args = []string{"url.dll,FileProtocolHandler", target}
	}
	if err := exec.Command(command, args...).Start(); err != nil {
		die("could not open %s: %v", target, err)
	}
}

// titleOr falls back to the filename, the way an untitled document is named.
func titleOr(title, file string) string {
	if strings.TrimSpace(title) != "" {
		return title
	}
	stem := strings.TrimSuffix(filepath.Base(file), filepath.Ext(file))
	return strings.TrimSpace(strings.NewReplacer("_", " ", "-", " ").Replace(stem))
}
