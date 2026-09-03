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

// crossSiteRefused applies rule A to a state-changing route a browser can
// reach with cookies attached: docs.<host> is same-site with the reader, so
// SameSite cookies alone do not stop a hostile document from posting here.
// A bearer token (the CLI) skips all of this -- it is never attached to a
// request automatically, so a hostile page cannot forge one. Otherwise all
// three must hold: any Origin header sent must be this reader's own origin,
// any Sec-Fetch-Site header must say the request was not cross-site, and a
// custom header must be present, which a browser cannot attach to a
// cross-origin request without a CORS preflight that is never granted.
func crossSiteRefused(r *http.Request) bool {
	if strings.HasPrefix(r.Header.Get("Authorization"), "Bearer ") {
		return false
	}
	if origin := r.Header.Get("Origin"); origin != "" && origin != readerOrigin(r) {
		return true
	}
	if site := r.Header.Get("Sec-Fetch-Site"); site != "" && site != "same-origin" && site != "none" {
		return true
	}
	return r.Header.Get("X-Komodoc-Client") == ""
}

// wsOriginRefused is rule A's WebSocket variant: browsers always send Origin
// on a WebSocket handshake and cannot be made to skip it or to attach a
// custom header, so the custom-header check does not apply here -- an absent
// Origin is not itself suspicious, but a foreign one is refused.
func wsOriginRefused(r *http.Request) bool {
	origin := r.Header.Get("Origin")
	return origin != "" && origin != readerOrigin(r)
}

// crossSiteRefusal is the JSON body every refusal under rule A answers with.
func crossSiteRefusal() map[string]any {
	return map[string]any{"error": "cross-site request refused"}
}
