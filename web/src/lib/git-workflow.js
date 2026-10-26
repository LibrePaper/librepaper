// The text a web project's Git panel hands out: the GitHub workflow that runs
// `librepaper pull` and `librepaper push`, and the commands that seed the
// LibrePaper state once. Both are the ones docs/git.md prints, with the
// project's own address in place of the placeholder; a unit test reads the
// docs and fails if the two drift apart.

/// Where the workflow is saved in the repository.
export const WORKFLOW_FILE = ".github/workflows/librepaper.yml";

/// The workflow, with `address` (`https://<site>/docs/<slug>`) as the project.
/** @param {string} address @returns {string} */
export function workflowYaml(address) {
  return [
    "on:",
    "  push:",
    "    branches: [main]",
    "  schedule:",
    '    - cron: "*/15 * * * *"',
    "concurrency: librepaper            # one run at a time per repository",
    "jobs:",
    "  sync:",
    "    runs-on: ubuntu-latest",
    "    permissions:",
    "      contents: write",
    "      pull-requests: write",
    "    steps:",
    "      - uses: actions/checkout@v4",
    "        with:",
    "          fetch-depth: 0",
    "      - uses: LibrePaper/sync-action@v1",
    "        with:",
    `          project: ${address}`,
    "          token: ${{ secrets.LIBREPAPER_TOKEN }}",
    "",
  ].join("\n");
}

/// The two commands that seed the LibrePaper state, run once in a clone of
/// the repository. The first pull merges unrelated histories itself.
/** @param {string} address @returns {string} */
export function seedingCommands(address) {
  return [
    "# Once, on your computer, in a clone of the repository",
    `librepaper pull ${address}   # links the clone and fetches the project as commits`,
    "git push origin main 'refs/librepaper/*'                 # the Action starts from this state",
    "",
  ].join("\n");
}
