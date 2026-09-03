package main

import (
	"encoding/base64"
	"net/http/httptest"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

func TestPolicies(t *testing.T) {
	cases := []struct {
		value   string
		login   string
		allowed bool
	}{
		{"anyone", "", true},
		{"anyone", "someone", true},
		{"any", "", false},
		{"any", "someone", true},
		{"vincent", "vincent", true},
		{"vincent", "Vincent", true}, // GitHub logins are case-insensitive
		{"vincent", "stranger", false},
		{"vincent", "", false},
		{"alice, bob", "bob", true},
		{"alice, bob", "carol", false},
		{"", "anyone at all", false}, // unconfigured allows nobody
	}
	for _, testCase := range cases {
		if got := parsePolicy(testCase.value).allows(testCase.login); got != testCase.allowed {
			t.Errorf("parsePolicy(%q).allows(%q) = %v, want %v",
				testCase.value, testCase.login, got, testCase.allowed)
		}
	}
}

func TestSessionCookies(t *testing.T) {
	key := []byte("0123456789abcdef0123456789abcdef")
	id := identity{Login: "vincent", ID: "42"}
	valid := signSession(key, id, time.Now().Add(time.Hour))
	if got := readSession(key, valid); got != id {
		t.Fatalf("round trip gave %+v", got)
	}
	if got := readSession(key, signSession(key, id, time.Now().Add(-time.Hour))); got.Login != "" {
		t.Fatalf("an expired session was accepted as %+v", got)
	}
	other := []byte("ffffffffffffffffffffffffffffffff")
	if got := readSession(other, valid); got.Login != "" {
		t.Fatalf("a cookie signed with another key was accepted as %+v", got)
	}
	// Flipping a character of the payload must invalidate the signature.
	tampered := "X" + valid[1:]
	if got := readSession(key, tampered); got.Login != "" {
		t.Fatalf("a tampered cookie was accepted as %+v", got)
	}
	// The old cookie shape carried only login|expiry. It must not be accepted
	// as though the missing id were merely empty: a caller with no id cannot
	// be told apart from one whose account was renamed, so such a session is
	// simply invalid, and its owner signs in again.
	oldPayload := base64.RawURLEncoding.EncodeToString(
		[]byte("vincent|" + strconv.FormatInt(time.Now().Add(time.Hour).Unix(), 10)))
	oldCookie := oldPayload + "." + sign(key, oldPayload)
	if got := readSession(key, oldCookie); got.Login != "" {
		t.Fatalf("an old two-field cookie was accepted as %+v", got)
	}
}

func TestCommentPolicyRefusesAndAttributes(t *testing.T) {
	dir := t.TempDir()
	instance := &server{
		store:      newStore(dir),
		rooms:      newRoomSet(filepath.Join(dir, "comments")),
		shell:      loadShell(),
		app:        githubApp{ClientID: "test-client"},
		key:        testKey,
		tokens:     newTokenCache(),
		publishers: parsePolicy(testPublisher),
		commenters: parsePolicy("any"),
	}
	server := httptest.NewServer(instance)
	t.Cleanup(server.Close)

	document := publishTestDocument(t, server.URL)
	slug := document["slug"].(string)
	path := "/api/documents/" + slug + "/comments"
	comment := map[string]string{"type": "comment", "exact": "hello", "body": "hi", "creator": "Impostor"}

	status, payload := postAs(t, "", server.URL, path, comment)
	if status != 400 || payload["message"] != "sign in with GitHub to comment" {
		t.Fatalf("anonymous comment got %d %v", status, payload)
	}

	// Signed in: the name on the comment is the verified login, not the one
	// the client asked for.
	status, payload = postAs(t, sessionAs("someone"), server.URL, path, comment)
	if status != 200 {
		t.Fatalf("signed-in comment got %d %v", status, payload)
	}
	stored := payload["comment"].(map[string]any)
	if stored["creator"] != "someone" {
		t.Fatalf("creator was %v, want the verified login", stored["creator"])
	}
}

// TestTokenCacheCachesPositiveAndNegativeAnswers is rule F's caching half: a
// verified token is not re-checked against GitHub on every request, and
// neither is one that fails, though a real deployment trusts the failure for
// a much shorter time.
func TestTokenCacheCachesPositiveAndNegativeAnswers(t *testing.T) {
	cache := newTokenCache()
	calls := 0
	good := identity{Login: "vincent", ID: "1"}
	check := func(token string) (identity, bool) {
		calls++
		if token == "good-token" {
			return good, true
		}
		return identity{}, false
	}

	if got := cache.verify(check, "good-token"); got != good {
		t.Fatalf("verify(good) = %+v, want %+v", got, good)
	}
	if got := cache.verify(check, "good-token"); got != good || calls != 1 {
		t.Fatalf("a cached positive answer made a second call: calls=%d, got=%+v", calls, got)
	}

	if got := cache.verify(check, "bad-token"); got.Login != "" {
		t.Fatalf("verify(bad) = %+v, want the zero identity", got)
	}
	if got := cache.verify(check, "bad-token"); got.Login != "" || calls != 2 {
		t.Fatalf("a cached negative answer made a third call: calls=%d, got=%+v", calls, got)
	}

	// An empty token is never even asked about: there is nothing to verify.
	if got := cache.verify(check, ""); got.Login != "" || calls != 2 {
		t.Fatalf("an empty token reached the checker: calls=%d, got=%+v", calls, got)
	}
}

func TestDeployPublishPolicyRefusesAnyone(t *testing.T) {
	// parseDeployPublishPolicy calls die() on "anyone", which exits; check the
	// parsing it builds on instead, so the refusal above it is the only
	// untested line.
	if !parsePolicy("anyone").Public {
		t.Fatal("anyone should parse as the public policy")
	}
	if parsePolicy("any").Public {
		t.Fatal("any is not public: it still needs an account")
	}
}

func TestDescribePolicy(t *testing.T) {
	for value, want := range map[string]string{
		"anyone":    "anyone",
		"any":       "any GitHub account",
		"vincent":   "@vincent",
		"alice,bob": "@alice, @bob",
		"":          "nobody (unconfigured)",
	} {
		if got := parsePolicy(value).describe(); got != want {
			t.Errorf("parsePolicy(%q).describe() = %q, want %q", value, got, want)
		}
	}
}

func TestAuthEndpoints(t *testing.T) {
	server, _ := newTestServer(t)

	// The client id is public, so the CLI can ask for it before signing in.
	status, payload := postAs(t, "", server.URL, "/api/auth/config", nil)
	_ = status
	if payload["client_id"] != "test-client" {
		t.Fatalf("/api/auth/config returned %v", payload)
	}

	status, payload = postAs(t, sessionAs(testPublisher), server.URL, "/api/me", nil)
	if status != 200 || payload["login"] != testPublisher || payload["can_publish"] != true {
		t.Fatalf("/api/me for a publisher returned %d %v", status, payload)
	}
	status, payload = postAs(t, "", server.URL, "/api/me", nil)
	if status != 200 || payload["login"] != "" || payload["can_publish"] != false {
		t.Fatalf("/api/me for a stranger returned %d %v", status, payload)
	}
	if !strings.Contains(text(payload["publishers"]), testPublisher) {
		t.Fatalf("/api/me should say who may publish, got %v", payload["publishers"])
	}
}

func TestSignedInCommentsAreNamedByTheAccount(t *testing.T) {
	// Commenting is open to anyone here, so a name may be typed. Signing in
	// should still override it: the account is the author.
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	path := "/api/documents/" + document["slug"].(string) + "/comments"
	comment := map[string]string{"type": "comment", "exact": "hello", "body": "hi", "creator": "Impostor"}

	status, payload := postAs(t, sessionAs("someone"), server.URL, path, comment)
	if status != 200 {
		t.Fatalf("signed-in comment got %d %v", status, payload)
	}
	if stored := payload["comment"].(map[string]any); stored["creator"] != "someone" {
		t.Fatalf("creator was %v, want the signed-in login", stored["creator"])
	}

	// Anonymous readers still name themselves.
	status, payload = postAs(t, "", server.URL, path, comment)
	if status != 200 {
		t.Fatalf("anonymous comment got %d %v", status, payload)
	}
	if stored := payload["comment"].(map[string]any); stored["creator"] != "Impostor" {
		t.Fatalf("creator was %v, want the typed name", stored["creator"])
	}
}

func TestAnnotationKinds(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	path := "/api/documents/" + document["slug"].(string) + "/comments"

	// A highlight is the passage itself, so it needs no words.
	status, payload := post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "motivation": "highlighting", "body": "",
	})
	if status != 200 {
		t.Fatalf("a bodyless highlight was refused: %d %v", status, payload)
	}
	if stored := payload["comment"].(map[string]any); stored["motivation"] != "highlighting" {
		t.Fatalf("stored as %v", stored["motivation"])
	}

	// Every other kind still needs something said.
	status, payload = post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "motivation": "commenting", "body": "",
	})
	if status != 400 || payload["message"] != "comment body is required" {
		t.Fatalf("a bodyless comment got %d %v", status, payload)
	}

	// A suggested edit carries the text it proposes.
	status, payload = post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "motivation": "editing",
		"body": "clearer this way", "replacement": "hello there",
	})
	if status != 200 {
		t.Fatalf("suggested edit got %d %v", status, payload)
	}
	if stored := payload["comment"].(map[string]any); stored["replacement"] != "hello there" {
		t.Fatalf("replacement came back as %v", stored["replacement"])
	}

	// Anything else sending replacement text has it dropped, not refused.
	status, payload = post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "motivation": "commenting",
		"body": "a remark", "replacement": "sneaky",
	})
	if status != 200 {
		t.Fatalf("comment got %d %v", status, payload)
	}
	if stored := payload["comment"].(map[string]any); stored["replacement"] != nil {
		t.Fatalf("replacement should be dropped for a comment, got %v", stored["replacement"])
	}
}

func TestTagsAreNormalised(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	path := "/api/documents/" + document["slug"].(string) + "/comments"

	status, payload := post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "body": "x",
		// Mixed case, padding, a duplicate, and more than the cap allows.
		"tags": []string{"Methods", " methods ", "TYPO", "a", "b", "c", "d", "e", "f"},
	})
	if status != 200 {
		t.Fatalf("tagged comment got %d %v", status, payload)
	}
	tags := payload["comment"].(map[string]any)["tags"].([]any)
	if len(tags) != config.MaxTags {
		t.Fatalf("got %d tags, want the cap of %d: %v", len(tags), config.MaxTags, tags)
	}
	if tags[0] != "methods" || tags[1] != "typo" {
		t.Fatalf("tags were not normalised or deduplicated: %v", tags)
	}
}

func TestRegionAnnotations(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	path := "/api/documents/" + document["slug"].(string) + "/comments"

	// A rectangle on a figure anchors an annotation, with no quotation at all.
	status, payload := post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "", "body": "the axis is unlabelled",
		"region": map[string]any{
			"image_digest": "abc123", "image_index": 1,
			"x": 10.5, "y": 20, "w": 30, "h": 25,
		},
	})
	if status != 200 {
		t.Fatalf("region annotation got %d %v", status, payload)
	}
	stored := payload["comment"].(map[string]any)["region"].(map[string]any)
	if stored["image_digest"] != "abc123" || stored["image_index"] != float64(1) || stored["x"] != 10.5 {
		t.Fatalf("region came back as %v", stored)
	}

	// Neither words nor a figure is nothing to point at.
	status, payload = post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "", "body": "about what?",
	})
	if status != 400 || payload["message"] != "select some text or part of a figure to comment on" {
		t.Fatalf("anchorless annotation got %d %v", status, payload)
	}

	// A rectangle outside the image, or too small to see, is not one.
	for name, spot := range map[string]map[string]any{
		"off the image": {"image_index": 0, "x": 90, "y": 10, "w": 30, "h": 10},
		"negative":      {"image_index": 0, "x": -5, "y": 10, "w": 10, "h": 10},
		"a click":       {"image_index": 0, "x": 10, "y": 10, "w": 0.1, "h": 0.1},
		"no image":      {"image_index": -1, "x": 10, "y": 10, "w": 10, "h": 10},
	} {
		status, payload = post(t, server.URL, path, map[string]any{
			"type": "comment", "exact": "", "body": "x", "region": spot,
		})
		if status != 400 {
			t.Errorf("%s: got %d %v, want a refusal", name, status, payload)
		}
	}
}

func TestSettingsMayBeQuoted(t *testing.T) {
	// A .env read by make keeps the quotes a shell would strip, and a client id
	// wearing quotation marks is one GitHub has never heard of.
	for value, want := range map[string]string{
		`"Ov23li"`:  "Ov23li",
		`'Ov23li'`:  "Ov23li",
		`  Ov23li `: "Ov23li",
		`Ov23li`:    "Ov23li",
		`"`:         `"`, // a lone quote is a value, not a wrapper
		``:          "",
	} {
		if got := firstOf(value); got != want {
			t.Errorf("firstOf(%q) = %q, want %q", value, got, want)
		}
	}
	// The first value that is not empty still wins.
	if got := firstOf("", `"second"`, "third"); got != "second" {
		t.Errorf("firstOf picked %q", got)
	}
}
