package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// The CLI signs in with GitHub's device flow: it asks for a code, you type
// that code into a browser anywhere, and the token lands here. No callback URL
// and no local web server, so it works over SSH and on a machine with no
// browser of its own.

// tokenPath is where the GitHub token is cached, following XDG.
func tokenPath() string {
	base := os.Getenv("XDG_CONFIG_HOME")
	if base == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			die("no home directory to store the token in: %v", err)
		}
		base = filepath.Join(home, ".config")
	}
	return filepath.Join(base, "komodoc", "token")
}

// storedToken returns the GitHub token to send, from the environment or the
// cache written by `komodoc login`.
func storedToken() string {
	if token := strings.TrimSpace(os.Getenv("KOMODOC_TOKEN")); token != "" {
		return token
	}
	raw, err := os.ReadFile(tokenPath())
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(raw))
}

// requireToken is what every command that writes needs.
func requireToken() string {
	token := storedToken()
	if token == "" {
		die("not signed in. Run:\n    komodoc login")
	}
	return token
}

func login(clientID, endpointFlag string) {
	if clientID == "" {
		// The deployment knows its own client id, and it is not a secret.
		endpoint := endpointFrom(endpointFlag)
		status, raw := do("GET", endpoint+"/api/auth/config", nil, nil, 30*time.Second)
		if status == 200 {
			var config struct {
				ClientID string `json:"client_id"`
			}
			if json.Unmarshal(raw, &config) == nil {
				clientID = config.ClientID
			}
		}
		if clientID == "" {
			die("could not find the GitHub client id.\n" +
				"  Pass it with --client-id, or point --endpoint at your deployment.")
		}
	}

	code, err := requestDeviceCode(clientID)
	if err != nil {
		die("could not start the sign-in: %v", err)
	}

	fmt.Fprintf(os.Stderr, "\n  Open %s\n  and enter the code:  %s\n\n",
		code.VerificationURI, code.UserCode)
	fmt.Fprint(os.Stderr, "  waiting for you to approve it")

	token, err := pollForToken(clientID, code)
	fmt.Fprintln(os.Stderr)
	if err != nil {
		die("%v", err)
	}

	who, err := loginFor(token)
	if err != nil {
		die("signed in, but GitHub would not say who you are: %v", err)
	}

	path := tokenPath()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		die("could not create %s: %v", filepath.Dir(path), err)
	}
	if err := os.WriteFile(path, []byte(token+"\n"), 0o600); err != nil {
		die("could not write %s: %v", path, err)
	}
	fmt.Printf("signed in as %s\n", who)
	fmt.Fprintf(os.Stderr, "  token stored in %s\n", path)
}

func logout() {
	path := tokenPath()
	if err := os.Remove(path); err != nil {
		if os.IsNotExist(err) {
			fmt.Println("not signed in")
			return
		}
		die("could not remove %s: %v", path, err)
	}
	fmt.Println("signed out")
}

type deviceCode struct {
	DeviceCode      string `json:"device_code"`
	UserCode        string `json:"user_code"`
	VerificationURI string `json:"verification_uri"`
	ExpiresIn       int    `json:"expires_in"`
	Interval        int    `json:"interval"`
}

func requestDeviceCode(clientID string) (deviceCode, error) {
	body, err := json.Marshal(map[string]string{"client_id": clientID, "scope": ""})
	if err != nil {
		return deviceCode{}, err
	}
	status, raw := do("POST", githubDevice, map[string]string{
		"content-type": "application/json",
		"accept":       "application/json",
	}, body, 30*time.Second)

	var code deviceCode
	if err := json.Unmarshal(raw, &code); err != nil || code.DeviceCode == "" {
		return deviceCode{}, fmt.Errorf("github returned %d: %s", status, truncate(string(raw), 200))
	}
	if code.Interval == 0 {
		code.Interval = 5
	}
	return code, nil
}

// pollForToken waits for the code to be approved, at the interval GitHub asks
// for and no faster: polling too eagerly earns a slow_down.
func pollForToken(clientID string, code deviceCode) (string, error) {
	deadline := time.Now().Add(time.Duration(max(code.ExpiresIn, 300)) * time.Second)
	interval := time.Duration(code.Interval) * time.Second

	for time.Now().Before(deadline) {
		time.Sleep(interval)
		fmt.Fprint(os.Stderr, ".")

		body, err := json.Marshal(map[string]string{
			"client_id":   clientID,
			"device_code": code.DeviceCode,
			"grant_type":  "urn:ietf:params:oauth:grant-type:device_code",
		})
		if err != nil {
			return "", err
		}
		_, raw := do("POST", githubToken, map[string]string{
			"content-type": "application/json",
			"accept":       "application/json",
		}, body, 30*time.Second)

		var reply struct {
			AccessToken string `json:"access_token"`
			Error       string `json:"error"`
		}
		if json.Unmarshal(raw, &reply) != nil {
			continue
		}
		switch {
		case reply.AccessToken != "":
			return reply.AccessToken, nil
		case reply.Error == "authorization_pending":
		case reply.Error == "slow_down":
			interval += 5 * time.Second
		case reply.Error == "access_denied":
			return "", fmt.Errorf("sign-in was denied")
		case reply.Error != "":
			return "", fmt.Errorf("github said: %s", reply.Error)
		}
	}
	return "", fmt.Errorf("the code expired before it was approved")
}
