# Repository tools

For maintainers. Self-hosters use `deploy/` at the repository root (the download) and `docker/` (the image build).

```
tools/
  assets/     pinned browser inputs and the asset mirror
    pins.mjs                  fetch or update the pins in assets.lock and web/loro-codemirror.lock
    mirror                    build, check, smoke, publish the wasm and LaTeX mirror; the only publisher
    mirror.mjs                what mirror calls: validation, staging, S3 upload
  deploy/     the official instance (librepaper.org)
    production                deploy a release, rebuild the site, verify, logs
    production*.toml          config templates; @NAME@ is filled from keys.yaml
    keys                      shell, edit, names for the SOPS file keys.yaml
    runbook                   print or edit runbook.enc
    policies/                 S3 bucket policies, applied by hand at OVH
  dev/        local development
    db                        persistent Postgres in Docker; wipe, sqlx-check, sqlx-prepare, test
    dev.toml, dev-oauth.toml  configs for make serve; keys.yaml holds the dev OAuth app
    demo/                     make demo: compose (run, stop, wipe), the container configs, the Caddyfile
    *.test.mjs                checks on the Makefile and the Loro pin
  release/    publishing a release
    version                   set the version in Cargo.toml, Cargo.lock and the kit pins
    package-deploy-kit.sh     archive deploy/ for a release
    update-*.sh, resolve-stable-release.sh   the Homebrew and Scoop updaters CI runs
    kit.test.mjs              the kit pins match the version and compose resolves
  test/       the rarer suites, out of make test
    suite                     backup, backup-sidecar, deploy-install, browser, smoke, e2e, external, fuzz
    postgres.sh               disposable Postgres container, sourced by db, mirror, suite
    deploy-install.sh         boot the packaged kit and drive ./manage
    backup/                   backup drill and sidecar test; see its README.md
    fuzz/                     cargo-fuzz targets, outside the workspace
    latex/                    corpus, TeX Live oracle, mirror server and harness for the browser test
```

Each tool's `*.test.mjs` sits beside it; `make test` runs `tools/**/*.test.mjs`. Every executable prints usage with `help`.
