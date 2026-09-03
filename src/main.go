// Komodoc: deploy the service, and publish documents to it.
//
// A single static binary with the Cloudflare Worker and the reader shell
// compiled in, so it deploys with nothing beside it.
//
//	komodoc deploy                       # create/update the service
//	komodoc deploy --label docs          # serve at docs.<subdomain>.workers.dev
//	komodoc publish paper.html           # publish, print the share link
//	komodoc publish paper.html --slug s  # replace, keeping link+comments
//	komodoc list                         # your documents
//	komodoc destroy --service            # delete everything, after confirming
package main

import (
	"bufio"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"regexp"
	"strings"
)

// The Worker name is the first label of the URL, <label>.<subdomain>.workers.dev,
// and also names the R2 bucket. Set once at startup from --label or $KOMODOC_LABEL.
var (
	scriptName = "komodoc"
	bucket     = "komodoc"
)

var reName = regexp.MustCompile(`^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`)

const usage = `komodoc: host HTML documents that readers can annotate.

  komodoc login                        sign in with GitHub (device flow)
  komodoc deploy                       create or update the Cloudflare service
  komodoc publish FILE                 publish a document and print its link
  komodoc serve                        run the service on this machine
  komodoc list                         list your documents
  komodoc comment ID                   open a document for commenting
  komodoc export SLUG                  annotations as W3C JSON-LD or markdown
  komodoc seed [--endpoint URL]        replace local or remote data with examples
  komodoc destroy --document SLUG      delete one document and its comments
  komodoc destroy --service            delete the whole deployment
  komodoc version                      print the version

Deploying needs a Cloudflare API token with these permissions, created at
dash.cloudflare.com/profile/api-tokens:

    Account · Workers Scripts        · Edit
    Account · Workers R2 Storage     · Edit
    Account · Account Settings       · Read

    export CLOUDFLARE_API_TOKEN=...
    export CLOUDFLARE_ACCOUNT_ID=...   # only if the token sees several accounts

Both deploy and serve need a GitHub OAuth app (github.com/settings/developers),
and --publishers saying which GitHub logins may publish.

Publishing needs neither of those, only the endpoint and a sign-in:

    export KOMODOC_ENDPOINT=https://komodoc.<subdomain>.workers.dev
    komodoc login
`

func die(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "error: "+format+"\n", args...)
	os.Exit(1)
}

// configure points this run at one labelled deployment.
func configure(label string) {
	chosen := label
	if chosen == "" {
		chosen = os.Getenv("KOMODOC_LABEL")
	}
	if chosen == "" {
		chosen = "komodoc"
	}
	chosen = strings.ToLower(strings.TrimSpace(chosen))
	if !reName.MatchString(chosen) {
		die("'%s' is not a valid label. Use lowercase letters, digits and\n"+
			"  hyphens, starting and ending with a letter or digit.", chosen)
	}
	// <label>-docs is where deploy puts the document host (see deploy.go); a
	// label already ending in -docs would collide with its own document host.
	if strings.HasSuffix(chosen, "-docs") {
		die("'%s' is not a valid label: <label>-docs is reserved for the\n"+
			"  document host this deployment creates for itself.", chosen)
	}
	scriptName, bucket = chosen, chosen
}

func isTerminal(file *os.File) bool {
	info, err := file.Stat()
	return err == nil && info.Mode()&os.ModeCharDevice != 0
}

// prompt reads a secret without echoing it. stty is the portable way to do
// that without pulling in a dependency; if it is unavailable the input is
// simply visible.
func prompt(label string) string {
	fmt.Fprint(os.Stderr, label)
	if isTerminal(os.Stdin) {
		if err := stty("-echo"); err == nil {
			defer func() {
				_ = stty("echo")
				fmt.Fprintln(os.Stderr)
			}()
		}
	}
	return readLine()
}

func confirm(label string) string {
	fmt.Fprint(os.Stderr, label)
	return readLine()
}

func readLine() string {
	line, err := bufio.NewReader(os.Stdin).ReadString('\n')
	if err != nil && line == "" {
		return ""
	}
	return strings.TrimSpace(line)
}

func stty(mode string) error {
	command := exec.Command("stty", mode)
	command.Stdin = os.Stdin
	return command.Run()
}

// text reads a string out of a decoded JSON object, for the fields the API is
// known to return.
func text(value any) string {
	if str, ok := value.(string); ok {
		return str
	}
	return ""
}

// detailOf pulls the error message out of an API reply, falling back to the
// whole reply when it has no error field.
func detailOf(payload map[string]any) any {
	if message, ok := payload["error"]; ok {
		return message
	}
	return payload
}

// serviceFlags are the options that describe a running service rather than
// where it runs: who may publish and comment, how large a document may be, and
// when documents expire. `deploy` and `serve` stand up the same service in two
// places, so they take them identically and this is the one place their names,
// defaults and help text are written.
type serviceFlags struct {
	clientID     *string
	clientSecret *string
	publishers   *string
	commenters   *string
	maxSize      *int
	quota        *int
	storage      *int
	expireAfter  *string
	expireFrom   *string
}

func addServiceFlags(flags *flag.FlagSet) serviceFlags {
	return serviceFlags{
		clientID:     flags.String("client-id", "", "GitHub OAuth app client id; or $KOMODOC_GITHUB_CLIENT_ID"),
		clientSecret: flags.String("client-secret", "", "GitHub OAuth app client secret; or $KOMODOC_GITHUB_CLIENT_SECRET"),
		publishers:   flags.String("publishers", "", "who may publish: a GitHub login, a comma-separated list, or 'any'"),
		commenters:   flags.String("commenters", "", "who may comment: 'anyone' (default), 'any' GitHub account, or a list of logins"),
		maxSize:      flags.Int("max-size", 0, "largest document accepted, in megabytes (default 4)"),
		quota:        flags.Int("quota", 0, "most one publisher may store across their documents, in megabytes (default 100)"),
		storage:      flags.Int("storage", 0, "most the whole deployment will store, in megabytes (default 5120)"),
		expireAfter:  flags.String("expire-after", "", "delete documents after this duration, for example 24h or 30d (default never)"),
		expireFrom:   flags.String("expire-from", "", "start expiry at 'updated' (default; last publication) or 'created'"),
	}
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprint(os.Stderr, usage)
		os.Exit(1)
	}

	switch os.Args[1] {
	case "deploy":
		flags := flag.NewFlagSet("deploy", flag.ExitOnError)
		shared := addServiceFlags(flags)
		label := flags.String("label", "", "deployment label: the first label of the URL and the bucket name (default komodoc, or $KOMODOC_LABEL)")
		examples := flags.String("examples", "", "GitHub logins allowed to install the six reserved example notebooks; enables them")
		_ = flags.Parse(os.Args[2:])
		setMaxHTML(*shared.maxSize)
		setStorage(*shared.quota, *shared.storage)
		deploy(deployOptions{
			label: *label, clientID: *shared.clientID, clientSecret: *shared.clientSecret,
			publishers: *shared.publishers, commenters: *shared.commenters,
			expireAfter: *shared.expireAfter, expireFrom: *shared.expireFrom,
			examples: *examples,
		})

	case "publish":
		flags := flag.NewFlagSet("publish", flag.ExitOnError)
		title := flags.String("title", "", "display title; defaults to the filename")
		slug := flags.String("slug", "", "full existing slug to replace, keeping link and comments")
		endpoint := flags.String("endpoint", "", "deployment URL; defaults to $KOMODOC_ENDPOINT")
		// flag stops at the first non-flag argument, so parse again after the
		// filename to accept `publish FILE --title T` as well as the reverse.
		_ = flags.Parse(os.Args[2:])
		rest := flags.Args()
		if len(rest) == 0 {
			die("usage: komodoc publish FILE [--title T] [--slug S]")
		}
		file := rest[0]
		_ = flags.Parse(rest[1:])
		if flags.NArg() != 0 {
			die("unexpected argument %q; usage: komodoc publish FILE [--title T] [--slug S]", flags.Arg(0))
		}
		publish(file, *title, *slug, *endpoint)

	case "serve":
		flags := flag.NewFlagSet("serve", flag.ExitOnError)
		shared := addServiceFlags(flags)
		port := flags.Int("port", 0, "port to listen on; default is the first free one from 8080 to 8099")
		dir := flags.String("data", "komodoc-data", "directory for documents and comments")
		_ = flags.Parse(os.Args[2:])
		setMaxHTML(*shared.maxSize)
		setStorage(*shared.quota, *shared.storage)
		serve(serveOptions{
			port: *port, dir: *dir,
			clientID: *shared.clientID, clientSecret: *shared.clientSecret,
			publishers: *shared.publishers, commenters: *shared.commenters,
			expireAfter: *shared.expireAfter, expireFrom: *shared.expireFrom,
		})

	case "login":
		flags := flag.NewFlagSet("login", flag.ExitOnError)
		clientID := flags.String("client-id", "", "GitHub OAuth app client id; asked of the deployment when absent")
		endpoint := flags.String("endpoint", "", "deployment URL; defaults to $KOMODOC_ENDPOINT")
		_ = flags.Parse(os.Args[2:])
		login(*clientID, *endpoint)

	case "logout":
		logout()

	case "seed":
		flags := flag.NewFlagSet("seed", flag.ExitOnError)
		dir := flags.String("data", "komodoc-data", "directory to wipe and fill")
		endpoint := flags.String("endpoint", "", "deployment URL to wipe and fill instead of a local directory")
		_ = flags.Parse(os.Args[2:])
		if *endpoint != "" {
			seedRemote(*endpoint, seedDocuments)
		} else {
			fmt.Printf("seeding %s\n", *dir)
			seed(*dir, seedDocuments)
		}

	case "export":
		flags := flag.NewFlagSet("export", flag.ExitOnError)
		format := flags.String("format", "jsonld", "jsonld (W3C Web Annotation) or markdown")
		out := flags.String("out", "", "file to write; defaults to standard output")
		endpoint := flags.String("endpoint", "", "deployment URL; defaults to $KOMODOC_ENDPOINT")
		_ = flags.Parse(os.Args[2:])
		rest := flags.Args()
		if len(rest) == 0 {
			die("usage: komodoc export SLUG [--format jsonld|markdown] [--out FILE]")
		}
		slug := rest[0]
		_ = flags.Parse(rest[1:])
		exportDocument(slug, *endpoint, *format, *out)

	case "list":
		flags := flag.NewFlagSet("list", flag.ExitOnError)
		endpoint := flags.String("endpoint", "", "deployment URL; defaults to $KOMODOC_ENDPOINT")
		_ = flags.Parse(os.Args[2:])
		listDocuments(*endpoint)

	case "comment":
		flags := flag.NewFlagSet("comment", flag.ExitOnError)
		endpoint := flags.String("endpoint", "", "deployment URL; defaults to $KOMODOC_ENDPOINT")
		_ = flags.Parse(os.Args[2:])
		if flags.NArg() != 1 {
			die("usage: komodoc comment ID [--endpoint URL]")
		}
		commentDocument(flags.Arg(0), *endpoint)

	case "destroy":
		flags := flag.NewFlagSet("destroy", flag.ExitOnError)
		document := flags.String("document", "", "delete just this document and its comments")
		service := flags.Bool("service", false, "delete the whole deployment and everything in it")
		label := flags.String("label", "", "deployment to delete (default komodoc)")
		endpoint := flags.String("endpoint", "", "deployment URL, with --document")
		yes := flags.Bool("yes", false, "skip the confirmation prompt (dangerous)")
		_ = flags.Parse(os.Args[2:])

		// Both are irreversible and one is far larger than the other, so neither
		// is the default: say which.
		switch {
		case *document != "" && *service:
			die("--document and --service delete different things; pick one")
		case *document != "":
			destroyDocument(*document, *endpoint, *yes)
		case *service:
			destroyService(*label, *yes)
		default:
			die("say what to delete:\n" +
				"    --document SLUG   one document, its history and its comments\n" +
				"    --service         the whole deployment, and everything in it")
		}

	case "version", "--version":
		fmt.Println(version)

	case "-h", "--help", "help":
		fmt.Print(usage)

	default:
		fmt.Fprintf(os.Stderr, "unknown command %q\n\n", os.Args[1])
		fmt.Fprint(os.Stderr, usage)
		os.Exit(1)
	}
}
