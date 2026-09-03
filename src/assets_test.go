package main

import (
	"bytes"
	"encoding/base64"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestWorkerSourceEmbedsShell(t *testing.T) {
	source := workerSource()
	if strings.Contains(source, "__SHELL__") {
		t.Fatal("shell placeholder was not replaced")
	}
	for _, needle := range []string{
		"class Room", "export default", "anchorAll", "can_sign_in",
		"KOMODOC_EXAMPLES", "example_revision", "komodoc_visitor",
	} {
		if !strings.Contains(source, needle) {
			t.Fatalf("worker source is missing %q", needle)
		}
	}
	if strings.Contains(source, `export { Room } from "./room.js"`) {
		t.Fatal("re-export line should be stripped")
	}
	// The routed files, plus the font, which is added rather than read from a
	// route table.
	if len(loadShell()) != len(shellRoutes)+1 {
		t.Fatalf("shell has %d files, want %d", len(loadShell()), len(shellRoutes)+1)
	}
}

func TestReaderOffersSignIn(t *testing.T) {
	shell := loadShell()
	if !strings.Contains(shell["/reader.html"].Body, `id="signIn"`) {
		t.Fatal("reader navigation has no sign-in link")
	}
	if !strings.Contains(shell["/reader.js"].Body, "me.can_sign_in") {
		t.Fatal("reader does not use the deployment's sign-in availability")
	}
}

func TestWordmarkFontIsServedAndCached(t *testing.T) {
	shell := loadShell()

	css := shell["/komodoc.css"].Body
	if !strings.Contains(css, `url("`+fontRoute+`")`) {
		t.Fatal("the stylesheet does not point at the font route")
	}
	// Never a font host: a page reaches out to nobody.
	if strings.Contains(css, "fonts.googleapis") || strings.Contains(css, "fonts.gstatic") {
		t.Fatal("the stylesheet should reference no external font host")
	}

	font, ok := shell[fontRoute]
	if !ok {
		t.Fatalf("%s is not served", fontRoute)
	}
	if font.Type != "font/woff2" || !font.Base64 {
		t.Fatalf("the font entry is %+v", shellFile{Type: font.Type, Base64: font.Base64})
	}
	// A font never changes, so it is cached for a year rather than sharing the
	// stylesheet's five minutes.
	if !font.Immutable {
		t.Fatal("the font should be immutable")
	}

	decoded, err := base64.StdEncoding.DecodeString(font.Body)
	if err != nil {
		t.Fatalf("the font body is not valid base64: %v", err)
	}
	if !bytes.HasPrefix(decoded, []byte("wOF2")) {
		t.Fatal("what is served is not a woff2 file")
	}
	// Complete and unmodified, so the reserved family name stays honest.
	if len(decoded) < 40_000 {
		t.Fatalf("the font is %d bytes; a subset would raise an OFL naming question", len(decoded))
	}
}

func TestDocumentationScreenshotIsServed(t *testing.T) {
	shell := loadShell()
	image, ok := shell["/docs/commenting.png"]
	if !ok {
		t.Fatal("documentation screenshot has no shell route")
	}
	if image.Type != "image/png" || !image.Base64 {
		t.Fatalf("documentation screenshot is %+v", shellFile{Type: image.Type, Base64: image.Base64})
	}
	decoded, err := base64.StdEncoding.DecodeString(image.Body)
	if err != nil {
		t.Fatalf("documentation screenshot is not valid base64: %v", err)
	}
	if !bytes.HasPrefix(decoded, []byte("\x89PNG\r\n\x1a\n")) {
		t.Fatal("documentation screenshot is not a PNG")
	}

	response := httptest.NewRecorder()
	request := httptest.NewRequest("GET", "/docs/commenting.png", nil)
	(&server{shell: shell}).ServeHTTP(response, request)
	if response.Code != 200 || response.Header().Get("content-type") != "image/png" {
		t.Fatalf("screenshot route returned %d %q", response.Code, response.Header().Get("content-type"))
	}
}
