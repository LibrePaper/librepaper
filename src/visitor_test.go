package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

// The visitor cookie is the owner key for anonymous uploads. A client that
// could set it to anything would manufacture as many owners as it liked, so
// owner() must trust only a cookie this server signed.

// TestUnsignedVisitorCookieOwnsNothing checks that a bare token -- the shape
// the cookie used to have, and the shape a forger would try -- does not
// confer ownership at all: owner() reads it as though no cookie were sent.
func TestUnsignedVisitorCookieOwnsNothing(t *testing.T) {
	_, instance := newTestServer(t)

	request := httptest.NewRequest("GET", "/", nil)
	request.Header.Set("cookie", visitorCookie+"=deadbeef")
	if got := instance.owner(request, identity{}); got != "" {
		t.Fatalf("an unsigned visitor cookie should own nothing, got %q", got)
	}
}

// TestTamperedVisitorCookieOwnsNothing checks that flipping the token while
// keeping some signature -- what an attacker gets by editing the cookie in
// devtools -- fails verification just as cleanly as having no signature at
// all.
func TestTamperedVisitorCookieOwnsNothing(t *testing.T) {
	_, instance := newTestServer(t)

	signed := signVisitor(testKey, "alpha")
	tampered := "bravo." + signed[len("alpha")+1:]

	request := httptest.NewRequest("GET", "/", nil)
	request.Header.Set("cookie", visitorCookie+"="+tampered)
	if got := instance.owner(request, identity{}); got != "" {
		t.Fatalf("a tampered visitor cookie should own nothing, got %q", got)
	}
}

// TestSignedVisitorCookieOwnsTheVisitorPrefix checks the positive case: a
// cookie this server actually minted verifies, and names its owner under the
// visitor: prefix that keeps it from colliding with a GitHub login.
func TestSignedVisitorCookieOwnsTheVisitorPrefix(t *testing.T) {
	_, instance := newTestServer(t)

	request := httptest.NewRequest("GET", "/", nil)
	request.Header.Set("cookie", visitorAs("alpha"))
	if got, want := instance.owner(request, identity{}), visitorPrefix+"alpha"; got != want {
		t.Fatalf("owner() = %q, want %q", got, want)
	}
}

// TestShellReissuesAnUnsignedCookie checks that a browser holding an old,
// unsigned cookie -- from before this server signed them -- is simply handed
// a fresh, signed one, the same as a browser with no cookie at all.
func TestShellReissuesAnUnsignedCookie(t *testing.T) {
	server, _ := newTestServer(t)

	request, err := http.NewRequest("GET", server.URL+"/", nil)
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("cookie", visitorCookie+"=deadbeef")
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()

	for _, cookie := range response.Cookies() {
		if cookie.Name != visitorCookie {
			continue
		}
		if cookie.Value == "deadbeef" {
			t.Fatalf("the unsigned cookie was kept rather than reissued")
		}
		if readVisitor(testKey, cookie.Value) == "" {
			t.Fatalf("the reissued cookie does not verify: %q", cookie.Value)
		}
		return
	}
	t.Fatalf("no visitor cookie was reissued: %v", response.Cookies())
}
