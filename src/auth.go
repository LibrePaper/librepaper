package main

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Identity comes from GitHub. Two paths reach the same place: a browser signs
// in through the OAuth web flow and carries a signed cookie afterwards, while
// the CLI holds a GitHub token from the device flow and sends it as a bearer.
// Both end up as a login name, which the policies below either allow or not.

const (
	githubAuthorize = "https://github.com/login/oauth/authorize"
	githubToken     = "https://github.com/login/oauth/access_token"
	githubDevice    = "https://github.com/login/device/code"
	githubUser      = "https://api.github.com/user"

	sessionCookie = "komodoc_session"
	stateCookie   = "komodoc_state"
	sessionMaxAge = 30 * 24 * time.Hour
)

// A policy says who may do something. The zero value allows nobody, which is
// the right default for publishing on a deployment that was never configured.
type policy struct {
	// Public means no sign-in at all, and is only meaningful for commenting.
	Public bool
	// Any means any GitHub account, once signed in.
	Any bool
	// Logins is the allowlist, lowercased, when neither of the above is set.
	Logins []string
}

// parsePolicy reads the value of --publishers or --commenters:
//
//	anyone            no sign-in required at all
//	any               any signed-in GitHub account
//	alice,bob         only these GitHub logins
func parsePolicy(value string) policy {
	trimmed := strings.ToLower(strings.TrimSpace(value))
	switch trimmed {
	case "":
		return policy{}
	case "anyone", "public":
		return policy{Public: true}
	case "any", "*", "anygithub":
		return policy{Any: true}
	}
	var logins []string
	for _, entry := range strings.Split(trimmed, ",") {
		if login := strings.TrimSpace(entry); login != "" {
			logins = append(logins, login)
		}
	}
	return policy{Logins: logins}
}

func (p policy) allows(login string) bool {
	if p.Public {
		return true
	}
	if login == "" {
		return false
	}
	if p.Any {
		return true
	}
	for _, allowed := range p.Logins {
		if strings.EqualFold(allowed, login) {
			return true
		}
	}
	return false
}

// describe is what the page shows when someone is refused.
func (p policy) describe() string {
	switch {
	case p.Public:
		return "anyone"
	case p.Any:
		return "any GitHub account"
	case len(p.Logins) == 0:
		return "nobody (unconfigured)"
	case len(p.Logins) == 1:
		return "@" + p.Logins[0]
	default:
		return "@" + strings.Join(p.Logins, ", @")
	}
}

func (p policy) String() string {
	switch {
	case p.Public:
		return "anyone"
	case p.Any:
		return "any"
	default:
		return strings.Join(p.Logins, ",")
	}
}

// --------------------------------------------------------------------------
// sessions
// --------------------------------------------------------------------------

// signSession returns "<payload>.<signature>", where the payload is the login
// and an expiry. Nothing is stored server-side: the signature is what makes it
// trustworthy.
func signSession(key []byte, login string, expiry time.Time) string {
	payload := base64.RawURLEncoding.EncodeToString(
		[]byte(login + "|" + strconv.FormatInt(expiry.Unix(), 10)))
	return payload + "." + sign(key, payload)
}

// readSession returns the login a cookie carries, or "" if it is forged,
// damaged or expired.
func readSession(key []byte, cookie string) string {
	payload, signature, found := strings.Cut(cookie, ".")
	if !found || subtle.ConstantTimeCompare([]byte(sign(key, payload)), []byte(signature)) != 1 {
		return ""
	}
	raw, err := base64.RawURLEncoding.DecodeString(payload)
	if err != nil {
		return ""
	}
	login, stamp, found := strings.Cut(string(raw), "|")
	if !found {
		return ""
	}
	expiry, err := strconv.ParseInt(stamp, 10, 64)
	if err != nil || time.Now().Unix() > expiry {
		return ""
	}
	return login
}

func sign(key []byte, payload string) string {
	mac := hmac.New(sha256.New, key)
	mac.Write([]byte(payload))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}

// sessionKey loads the key that signs cookies, creating it on first run. It
// lives beside the documents so restarts do not sign everyone out.
func sessionKey(dir string) []byte {
	path := filepath.Join(dir, "session.key")
	if raw, err := os.ReadFile(path); err == nil {
		if key, err := hex.DecodeString(strings.TrimSpace(string(raw))); err == nil && len(key) == 32 {
			return key
		}
	}
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		die("no randomness available: %v", err)
	}
	if err := os.WriteFile(path, []byte(hex.EncodeToString(key)), 0o600); err != nil {
		die("could not write %s: %v", path, err)
	}
	return key
}

func randomToken() string {
	raw := make([]byte, 16)
	if _, err := rand.Read(raw); err != nil {
		die("no randomness available: %v", err)
	}
	return hex.EncodeToString(raw)
}

// --------------------------------------------------------------------------
// GitHub
// --------------------------------------------------------------------------

type githubApp struct {
	ClientID     string
	ClientSecret string
}

func (app githubApp) configured() bool { return app.ClientID != "" }

// authorizeURL is where a browser is sent to sign in. No scopes are asked for:
// the default gives the account's public profile, which is the login name, and
// nothing else.
func (app githubApp) authorizeURL(redirect, state string) string {
	query := url.Values{
		"client_id":    {app.ClientID},
		"redirect_uri": {redirect},
		"state":        {state},
		"scope":        {""},
	}
	return githubAuthorize + "?" + query.Encode()
}

// exchange turns the code GitHub redirected back with into an access token.
func (app githubApp) exchange(code, redirect string) (string, error) {
	body, err := json.Marshal(map[string]string{
		"client_id":     app.ClientID,
		"client_secret": app.ClientSecret,
		"code":          code,
		"redirect_uri":  redirect,
	})
	if err != nil {
		return "", err
	}
	status, raw := do("POST", githubToken, map[string]string{
		"content-type": "application/json",
		"accept":       "application/json",
	}, body, 30*time.Second)

	var reply struct {
		AccessToken string `json:"access_token"`
		Error       string `json:"error_description"`
	}
	if err := json.Unmarshal(raw, &reply); err != nil {
		return "", fmt.Errorf("github returned %d", status)
	}
	if reply.AccessToken == "" {
		if reply.Error == "" {
			reply.Error = fmt.Sprintf("github returned %d", status)
		}
		return "", fmt.Errorf("%s", reply.Error)
	}
	return reply.AccessToken, nil
}

// loginFor asks GitHub who a token belongs to.
func loginFor(token string) (string, error) {
	status, raw := do("GET", githubUser, map[string]string{
		"authorization": "Bearer " + token,
		"accept":        "application/vnd.github+json",
	}, nil, 30*time.Second)
	if status != 200 {
		return "", fmt.Errorf("github returned %d", status)
	}
	var user struct {
		Login string `json:"login"`
	}
	if err := json.Unmarshal(raw, &user); err != nil || user.Login == "" {
		return "", fmt.Errorf("github returned no login")
	}
	return user.Login, nil
}

// tokenCache keeps bearer tokens from costing a GitHub call per request.
type tokenCache struct {
	mu      sync.Mutex
	entries map[string]cachedLogin
}

type cachedLogin struct {
	login   string
	expires time.Time
}

func newTokenCache() *tokenCache {
	return &tokenCache{entries: map[string]cachedLogin{}}
}

func (c *tokenCache) login(token string) string {
	if token == "" {
		return ""
	}
	// The token itself is never stored, only a digest of it.
	sum := sha256.Sum256([]byte(token))
	key := hex.EncodeToString(sum[:])

	c.mu.Lock()
	entry, ok := c.entries[key]
	c.mu.Unlock()
	if ok && time.Now().Before(entry.expires) {
		return entry.login
	}

	login, err := loginFor(token)
	if err != nil {
		return ""
	}
	c.mu.Lock()
	c.entries[key] = cachedLogin{login: login, expires: time.Now().Add(10 * time.Minute)}
	c.mu.Unlock()
	return login
}

// parsePublishPolicy is parsePolicy with the public option refused: publishing
// always requires a signed-in GitHub account, so the weakest setting is "any".
func parsePublishPolicy(value string) policy {
	chosen := parsePolicy(value)
	if chosen.Public {
		die("--publishers cannot be %q: publishing always needs a GitHub account.\n"+
			"  Use 'any' for any account, or a comma-separated list of logins.", value)
	}
	return chosen
}
