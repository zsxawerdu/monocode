# Native jj and Pi codemode

Build the release app with `npm run native:build`. On macOS, open
`crates/monocode-gpui/target/release/Monocode Native.app`. This bundle uses the
installed Monocode database, not the development database.

## Jujutsu

The native app detects `.jj` workspaces before using Git. This integration was
validated with jj 0.46.0, including a non-colocated repository. Install `jj` on
your login shell's PATH so the app can find it when launched from Finder.

The Changes and History panes use jj. Working-copy changes have no staging
step: Commit runs `jj commit -m`, which describes the current change and starts
an empty child. Describe Working Copy updates the current change description
without creating a child. The Changes panel commits the entire working-copy
change; the conversation's commit action asks its agent to isolate that
session's changes.

Fetch uses `jj git fetch` without automatically rebasing. Push selects one
bookmark on the working change, or its parent if the working change has none.
It refuses ambiguous or missing bookmarks and does not create tracking
bookmarks automatically. Rejections are reported by jj.

The existing branch picker lists bookmarks. Selecting one starts a new change
on that bookmark; creating one creates a bookmark at the working change.
Creating a worktree creates a jj workspace in a sibling `-workspaces` folder.
These operations never invoke Git mutations on a jj workspace.

Current limits: the picker retains some Git terminology, Git-style upstream
counts and PR creation are not implemented for jj, and per-file comparisons
require a single parent. Use `jj diff` for merge comparisons. Session commit
summary cards still recognize Git command output; jj results remain available
in the agent's tool output and reply. Remote-host projects retain the host's
existing Git implementation.

## Pi codemode

In Settings → Providers → Advanced, enable **Pi codemode (experimental)**,
then start a new Pi agent process/session. It is off by default, requires a Pi
version with codemode support, and adds `--tools +codemode` to normal Pi
sessions. It does not affect omp or isolated text jobs, and Monocode does not
enable it for plan/read-only turns.

Pi executes the scripts; Monocode displays their tool events, including nested
calls. Raw JavaScript arguments are preserved as well as object arguments.
See https://pi.dev/docs/latest/codemode for the runtime API.

Validation was limited to two focused release-profile tests (jj workflow and
Pi argument/access handling), one successful read-only Pi RPC codemode call,
and the native release build. Visual layout and interaction need checking in
the release app.

## Shared codemode for other agents

**Settings → Providers → Advanced → Shared codemode** is enabled by default.
It supplies a session-scoped `monocode_codemode` MCP server to Codex, Claude
Code, and ACP agents that accept stdio MCP servers. Start a new turn after
changing the setting; access mode and this setting participate in process
reuse checks. Pi and omp keep their own integrations. OpenCode's separate
native managed-service transport is not automatically connected in this
iteration. Remote-host sessions are also excluded.

The release app bundles `@earendil-works/pi-codemode` 1.4.0 and MCP SDK 1.32.2
as a local sidecar. Node 22.19+ must be available on the login-shell PATH;
`search` also needs ripgrep. The build installs pinned dependencies with
`npm ci --ignore-scripts`; no packages are downloaded at app runtime. No
global provider MCP settings are modified.

The agent discovers tools with `monocode_codemode.search`, then submits a
JavaScript function body to `monocode_codemode.execute`:

```js
const [readme, files] = await Promise.all([
  tools.read({path: "README.md"}),
  tools.list({path: "."}),
]);
text({readme: readme.slice(0, 500), entries: files.entries.length});
```

The sidecar has its own workspace-confined `read`, `list`, and `search` tools.
It does not reach into the host agent's private tools. **Full Access** also
exposes `write` and `shell`, and connects explicitly configured downstream MCP
servers. Shell has the user's normal OS permissions and can access outside
the workspace. Plan, read-only, and approval-based modes omit these broader
tools instead of bypassing the agent's per-operation approval machinery.
Use the host agent's normal tools for writes that need approval.

Scripts have a 60-second deadline, 64 MiB VM limit, and 100-call limit.
Commands have a 30-second deadline and 1 MiB output limit. Explicit script
text is capped at 40,000 characters; images share a 2 MiB base64 budget.
`store`/`load` are per MCP process, survive successful executions, and are lost
on process restart. Script failure does not undo tool side effects.
Execution results include tool names, completion states and durations.

Optional downstream servers belong in your user-owned
`~/.config/monocode/codemode.json`, not project-controlled configuration:

```json
{
  "mcpServers": {
    "docs": {
      "url": "https://example.com/mcp",
      "headers": {"Authorization": "env:DOCS_AUTHORIZATION"}
    },
    "local": {
      "command": "my-mcp-server",
      "args": [],
      "env": {"TOKEN": "env:LOCAL_MCP_TOKEN"}
    }
  }
}
```

Use environment references for credentials. `DOCS_AUTHORIZATION` contains
the complete header value, including `Bearer ` when required. Downstream
stdio and Streamable HTTP are supported; OAuth login, MCP resources and
sampling/elicitation forwarding are not implemented. Tool names use
`server__tool`, such as `tools.docs__search(...)`; use discovery for their
schemas. Restart the agent process after changing downstream configuration.

The standalone sidecar can also be registered manually with another MCP
client using `node <path>/server.mjs --cwd <workspace> --access read-only`.
In the macOS app, the script lives at
`Monocode Native.app/Contents/Resources/codemode/server.mjs`.
Use `--access full` only when broader tool execution is intended.

Shared-server verification: two Node integration tests cover batched reads,
path/symlink confinement, process-local state, Full Access writes/commands,
a downstream MCP fixture, and cancellation. A real read-only Codex execution
successfully invoked this server and returned the expected file contents.
Other agent transports are wired but have not been live-tested.
