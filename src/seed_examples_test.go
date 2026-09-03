package main

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

// The curated examples are build outputs, so a checkout that has not run
// `make examples` has nothing to check. Skipping keeps `go test ./...` honest
// on a fresh clone without pretending the examples were verified.
func exampleRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs("..")
	if err != nil {
		t.Fatal(err)
	}
	return root
}

// Every Exact in seed_examples.go has to appear in the rendered document, or
// the annotation anchors nowhere and the seeded demonstration is quietly wrong.
// This is the claim the file's own comment makes; here it is enforced.
func TestSeededAnnotationsAnchor(t *testing.T) {
	root := exampleRoot(t)
	checked := 0

	for _, document := range seedDocuments {
		path := filepath.Join(root, document.File)
		source, err := os.ReadFile(path)
		if err != nil {
			t.Logf("skipping %s: not built", document.File)
			continue
		}

		rendered := string(source)
		if isMarkdown(document.File) {
			rendered, err = renderMarkdownDocument(rendered, document.Title)
			if err != nil {
				t.Fatalf("%s: %v", document.File, err)
			}
		}
		text := visibleText(rendered)

		for _, annotation := range document.Annotations {
			if annotation.Region != nil || annotation.Exact == "" {
				continue
			}
			checked++
			if !strings.Contains(text, annotation.Exact) {
				t.Errorf("%s: no anchor for %q", document.File, annotation.Exact)
			}
		}
	}

	if checked == 0 {
		t.Skip("no examples built; run `make examples`")
	}
}

var reImageTag = regexp.MustCompile(`(?i)<img\b`)

// A region annotation names an image by its position in the document. An index
// past the end draws the rectangle nowhere, which the seeder cannot detect.
func TestSeededRegionsNameAnImageThatExists(t *testing.T) {
	root := exampleRoot(t)
	checked := 0

	for _, document := range seedDocuments {
		source, err := os.ReadFile(filepath.Join(root, document.File))
		if err != nil {
			continue
		}
		images := len(reImageTag.FindAllString(string(source), -1))

		for _, annotation := range document.Annotations {
			if annotation.Region == nil {
				continue
			}
			checked++
			if annotation.Region.ImageIndex >= images {
				t.Errorf("%s: region names image %d of %d",
					document.File, annotation.Region.ImageIndex, images)
			}
		}
	}

	if checked == 0 {
		t.Skip("no examples built; run `make examples`")
	}
}
