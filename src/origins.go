package main

import (
	"net/http"
	"strings"
)

// Documents are served from a different hostname than the reader, so an
// uploaded file is a stranger to the page framing it. The browser then refuses
// it any access to the reader's DOM or its session, which is what lets the
// document run its own scripts safely: charts, maps, anything.
//
// A different port would not do. Cookies ignore ports, so a document on
// another port could still make requests carrying the reader's session. It has
// to be a different host.
//
// The names are derived rather than configured: whatever host the reader is
// on, documents live on "docs." in front of it. On a real domain that is one
// DNS record and one certificate; in development, browsers resolve anything
// ending in .localhost by themselves, so it needs no setup at all.

const docsPrefix = "docs."

// isDocsHost says whether this request arrived on the document hostname.
func isDocsHost(r *http.Request) bool {
	return strings.HasPrefix(strings.ToLower(r.Host), docsPrefix)
}

// docsHost is where documents for this deployment live.
func docsHost(host string) string {
	if strings.HasPrefix(strings.ToLower(host), docsPrefix) {
		return host
	}
	return docsPrefix + host
}

// readerHost is the inverse: the reader that owns a document hostname.
func readerHost(host string) string {
	return strings.TrimPrefix(host, docsPrefix)
}

func requestScheme(r *http.Request) string {
	if r.TLS != nil || r.Header.Get("X-Forwarded-Proto") == "https" {
		return "https"
	}
	return "http"
}

// docsOrigin is the origin the reader frames documents from, and the only
// origin it accepts postMessage traffic from.
func docsOrigin(r *http.Request) string {
	return requestScheme(r) + "://" + docsHost(r.Host)
}

func readerOrigin(r *http.Request) string {
	return requestScheme(r) + "://" + readerHost(r.Host)
}
