package main

import (
	"bytes"
	"crypto/tls"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"net/textproto"
	"net/url"
	"strings"
	"time"
)

// Cloudflare's edge blocks unrecognised User-Agents outright, with a 403 and
// "error code: 1010", so every request has to identify itself.
const userAgent = "komodoc/1.0"

// do makes one HTTP round trip, returning (status, body) rather than an error
// on a 4xx: both APIs here put their error detail in the response body.
func do(method, target string, headers map[string]string, body []byte, timeout time.Duration) (int, []byte) {
	var reader io.Reader
	if body != nil {
		reader = bytes.NewReader(body)
	}
	req, err := http.NewRequest(method, target, reader)
	if err != nil {
		die("%s %s: %v", method, target, err)
	}
	req.Header.Set("user-agent", userAgent)
	for name, value := range headers {
		req.Header.Set(name, value)
	}

	client := &http.Client{Timeout: timeout}
	response, err := client.Do(req)
	if err != nil {
		// A workers.dev hostname resolves as soon as it is created, but its TLS
		// certificate is issued a few minutes later. Until then the handshake is
		// refused, which is alarming and not actually a problem.
		var certErr *tls.CertificateVerificationError
		parsed, _ := url.Parse(target)
		if parsed != nil && strings.Contains(target, "workers.dev") &&
			(errors.As(err, &certErr) || strings.Contains(err.Error(), "tls:")) {
			die("TLS handshake refused by %s.\n\n"+
				"  A newly created workers.dev subdomain resolves before its\n"+
				"  certificate is issued. This usually clears within a few minutes.\n\n"+
				"  Wait for it with:\n"+
				"    until curl -sfo /dev/null %s://%s/; do sleep 30; done\n\n"+
				"  then try again.", parsed.Hostname(), parsed.Scheme, parsed.Hostname())
		}
		die("%s %s: %v", method, target, err)
	}
	defer response.Body.Close()

	raw, err := io.ReadAll(response.Body)
	if err != nil {
		die("%s %s: %v", method, target, err)
	}
	return response.StatusCode, raw
}

// postAuthed carries the GitHub token the CLI signed in with.
func postAuthed(target string, payload any, token string, timeout time.Duration) (int, map[string]any) {
	return postDecoded(target, payload, token, timeout)
}

// postDecoded is the body of both: encode, post, decode. An empty token sends
// no authorization header at all, which is what an unauthenticated call means
// -- a deployment whose publishers policy is "anyone" takes uploads with no
// bearer at all. Without a bearer, the server treats a request as
// cookie-authenticated and applies the cross-site checks in rule A, so the CLI
// carries the same marker header the browser shell does; a bearer-carrying
// call skips those checks regardless.
func postDecoded(target string, payload any, token string, timeout time.Duration) (int, map[string]any) {
	body, err := json.Marshal(payload)
	if err != nil {
		die("could not encode the request: %v", err)
	}
	headers := map[string]string{"content-type": "application/json", "x-komodoc-client": "cli"}
	if token != "" {
		headers["authorization"] = "Bearer " + token
	}
	status, raw := do("POST", target, headers, body, timeout)

	var decoded map[string]any
	if err := json.Unmarshal(raw, &decoded); err != nil {
		return status, map[string]any{"error": truncate(string(raw), 300)}
	}
	return status, decoded
}

type formPart struct {
	name        string
	filename    string
	contentType string
	body        string
}

// buildMultipart returns the Content-Type header and body for a
// multipart/form-data upload.
func buildMultipart(parts []formPart) (string, []byte) {
	var buffer bytes.Buffer
	writer := multipart.NewWriter(&buffer)
	for _, part := range parts {
		disposition := fmt.Sprintf(`form-data; name="%s"`, part.name)
		if part.filename != "" {
			disposition += fmt.Sprintf(`; filename="%s"`, part.filename)
		}
		header := textproto.MIMEHeader{}
		header.Set("Content-Disposition", disposition)
		header.Set("Content-Type", part.contentType)

		field, err := writer.CreatePart(header)
		if err != nil {
			die("could not build the upload: %v", err)
		}
		if _, err := io.WriteString(field, part.body); err != nil {
			die("could not build the upload: %v", err)
		}
	}
	if err := writer.Close(); err != nil {
		die("could not build the upload: %v", err)
	}
	return writer.FormDataContentType(), buffer.Bytes()
}

func truncate(text string, limit int) string {
	if len(text) <= limit {
		return text
	}
	return text[:limit]
}
