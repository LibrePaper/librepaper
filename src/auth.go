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
	"net/http"
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
	// visitorCookie names the browser itself, so an upload made without
	// signing in still belongs to whoever made it.
	visitorCookie = "komodoc_visitor"
	sessionMaxAge = 30 * 24 * time.Hour

	// hostCookiePrefix is added to every cookie name on an HTTPS request. A
	// browser refuses to set a __Host- cookie unless it also carries Secure,
	// Path=/, and no Domain -- exactly how every cookie here is already set --
	// which is what keeps a same-site subdomain from planting one.
	hostCookiePrefix = "__Host-"
)

// identity is who a caller is, once verified: the GitHub login, and its
// numeric account id as a decimal string. Both are empty for an anonymous
// caller. The id is what ownership and comment authorship actually key on --
// a login can be renamed, the numeric id cannot -- the login is kept mainly
// for display and for the publishers/commenters policies, which are written
// in terms of it.
type identity struct {
	Login string
	ID    string
}

// cookieName gains the __Host- prefix on an HTTPS request; on plain HTTP
// (local `serve`) the old name is used, since __Host- is refused by browsers
// without Secure. An HTTPS request must read only the prefixed name: the
// plain one is exactly what a same-site document could plant in the reader's
// browser, so falling back to it would defeat the point of the prefix.
func cookieName(r *http.Request, base string) string {
	if requestScheme(r) == "https" {
		return hostCookiePrefix + base
	}
	return base
}

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

// signSession returns "<payload>.<signature>", where the payload is the login,
// the numeric account id, and an expiry. Nothing is stored server-side: the
// signature is what makes it trustworthy. A cookie from before the id was
// added has only two fields and fails to parse below, which is deliberate:
// such a session carries no id to check comment or document ownership
// against, so it is treated as invalid rather than half-trusted.
func signSession(key []byte, id identity, expiry time.Time) string {
	payload := base64.RawURLEncoding.EncodeToString(
		[]byte(id.Login + "|" + id.ID + "|" + strconv.FormatInt(expiry.Unix(), 10)))
	return payload + "." + sign(key, payload)
}

// readSession returns the identity a cookie carries, or the zero identity if
// it is forged, damaged, expired, or in the old two-field shape.
func readSession(key []byte, cookie string) identity {
	payload, signature, found := strings.Cut(cookie, ".")
	if !found || subtle.ConstantTimeCompare([]byte(sign(key, payload)), []byte(signature)) != 1 {
		return identity{}
	}
	raw, err := base64.RawURLEncoding.DecodeString(payload)
	if err != nil {
		return identity{}
	}
	parts := strings.SplitN(string(raw), "|", 3)
	if len(parts) != 3 {
		return identity{}
	}
	login, id, stamp := parts[0], parts[1], parts[2]
	expiry, err := strconv.ParseInt(stamp, 10, 64)
	if err != nil || time.Now().Unix() > expiry {
		return identity{}
	}
	return identity{Login: login, ID: id}
}

func sign(key []byte, payload string) string {
	mac := hmac.New(sha256.New, key)
	mac.Write([]byte(payload))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}

// signVisitor returns "<token>.<signature>" for a freshly minted visitor
// token, so a browser cannot simply pick its own owner key.
func signVisitor(key []byte, token string) string {
	return token + "." + sign(key, token)
}

// readVisitor returns the token a visitor cookie carries, or "" when the
// cookie is forged, damaged, or in the old unsigned form a browser issued
// this server before might still hold. Treating that old form as absent
// means such a browser is simply reissued a signed cookie, rather than kept
// on a value nothing here can verify.
func readVisitor(key []byte, cookie string) string {
	token, signature, found := strings.Cut(cookie, ".")
	if !found || token == "" {
		return ""
	}
	if subtle.ConstantTimeCompare([]byte(sign(key, token)), []byte(signature)) != 1 {
		return ""
	}
	return token
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

// loginFor asks GitHub who a token belongs to, via the browser OAuth flow's
// own token: the code exchange already proves it was issued to this app, so
// the plain /user endpoint is enough here. It reads the numeric id as well as
// the login, since both go into the session cookie.
func loginFor(token string) (identity, error) {
	status, raw := do("GET", githubUser, map[string]string{
		"authorization": "Bearer " + token,
		"accept":        "application/vnd.github+json",
	}, nil, 30*time.Second)
	if status != 200 {
		return identity{}, fmt.Errorf("github returned %d", status)
	}
	var user struct {
		Login string `json:"login"`
		ID    int64  `json:"id"`
	}
	if err := json.Unmarshal(raw, &user); err != nil || user.Login == "" {
		return identity{}, fmt.Errorf("github returned no login")
	}
	return identity{Login: strings.ToLower(user.Login), ID: strconv.FormatInt(user.ID, 10)}, nil
}

// checkToken verifies a bearer token the way the CLI's device-flow token
// arrives: not through this app's own OAuth code exchange, so GET /user alone
// only proves the token belongs to *some* GitHub account, not that it was
// issued to this deployment. GitHub's check-token endpoint proves that: it
// answers only for tokens issued to the client id being asked about, and 404s
// for anything else, including a token that is simply invalid.
func (app githubApp) checkToken(token string) (identity, bool) {
	if !app.configured() {
		return identity{}, false
	}
	body, err := json.Marshal(map[string]string{"access_token": token})
	if err != nil {
		return identity{}, false
	}
	basic := base64.StdEncoding.EncodeToString([]byte(app.ClientID + ":" + app.ClientSecret))
	status, raw := do("POST", fmt.Sprintf("https://api.github.com/applications/%s/token", app.ClientID),
		map[string]string{
			"authorization": "Basic " + basic,
			"accept":        "application/vnd.github+json",
			"content-type":  "application/json",
		}, body, 30*time.Second)
	if status != 200 {
		// 404 means the token is not this app's (or is not valid at all); any
		// other status is treated the same way -- unverified, not an error the
		// caller has to handle differently.
		return identity{}, false
	}
	var reply struct {
		User struct {
			Login string `json:"login"`
			ID    int64  `json:"id"`
		} `json:"user"`
	}
	if err := json.Unmarshal(raw, &reply); err != nil || reply.User.Login == "" {
		return identity{}, false
	}
	return identity{Login: strings.ToLower(reply.User.Login), ID: strconv.FormatInt(reply.User.ID, 10)}, true
}

// tokenCache keeps bearer tokens from costing a GitHub call per request.
// Positive answers are cached longer than negative ones, so a token that is
// revoked or was never valid does not sit trusted for as long as one that is.
type tokenCache struct {
	mu      sync.Mutex
	entries map[string]cachedToken
}

type cachedToken struct {
	identity identity
	ok       bool
	expires  time.Time
}

func newTokenCache() *tokenCache {
	return &tokenCache{entries: map[string]cachedToken{}}
}

const (
	tokenPositiveTTL = 10 * time.Minute
	tokenNegativeTTL = time.Minute
)

// verify resolves a bearer token to an identity, caching the answer keyed by
// a digest of the token -- never the token itself. check is what actually
// asks GitHub; a test substitutes a stand-in with the same shape so the
// caching behaviour can be exercised without a network call.
func (c *tokenCache) verify(check func(token string) (identity, bool), token string) identity {
	if token == "" {
		return identity{}
	}
	sum := sha256.Sum256([]byte(token))
	key := hex.EncodeToString(sum[:])

	c.mu.Lock()
	entry, found := c.entries[key]
	c.mu.Unlock()
	if found && time.Now().Before(entry.expires) {
		if entry.ok {
			return entry.identity
		}
		return identity{}
	}

	id, ok := check(token)
	ttl := tokenNegativeTTL
	if ok {
		ttl = tokenPositiveTTL
	}
	c.mu.Lock()
	c.entries[key] = cachedToken{identity: id, ok: ok, expires: time.Now().Add(ttl)}
	c.mu.Unlock()
	if !ok {
		return identity{}
	}
	return id
}

// parseDeployPublishPolicy is parsePolicy with the public option refused. The
// rule is a deployment's, not a universal one: a deployed Worker is on the open
// internet, where publishing with no account at all would let anyone fill the
// bucket. `serve` runs on a machine its operator already controls and keeps the
// public option, which is what --publishers anyone means there.
func parseDeployPublishPolicy(value string) policy {
	chosen := parsePolicy(value)
	if chosen.Public {
		die("--publishers cannot be %q when deploying: publishing to a deployment\n"+
			"  always needs a GitHub account. Use 'any' for any account, or a\n"+
			"  comma-separated list of logins.", value)
	}
	return chosen
}
