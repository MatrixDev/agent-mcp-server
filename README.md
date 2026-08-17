# mcp-server

A [Model Context Protocol](https://modelcontextprotocol.io) server (`mdev`) that exposes
file-system, build, git/GitHub, and smart-light tools to an agent. It runs commands
directly (without spawning a terminal shell) and returns their combined stdout/stderr.

Two transports are built in:

- **HTTP** (default) — streamable HTTP on `0.0.0.0:9999`, served at `http://localhost:9999/mcp`.
- **stdio** — run the binary with the `stdio` argument.

Build it with `cargo build --release`; the binary is produced at `target/release/mcp-server`.

## Tools

### Working directory

Every tool resolves paths against a single **workspace root**, which also anchors the
permission filter. Relative paths are joined to it, `~` expands to the home directory, and
absolute paths are used as-is; symlinks are then resolved.

The server picks the root per connection, in this order:

1. **`x-mcp-workspace-root` HTTP header** — if present on the request, its (canonicalized)
   value is used. This is the preferred mechanism for the HTTP transport; set it to the
   project directory the agent is working in.
2. **MCP `roots/list`** — if the header is missing or invalid, the server calls back to the
   client (`roots/list`) and uses the first `file://` root it advertises.

Over **stdio** there is no HTTP header, so the client must advertise a root via `roots/list`.
If neither a header nor a usable root is available, path resolution fails and the tools
cannot run.

### File system

Access is gated by a path-permission filter: reads/writes are confined to the workspace
(plus a few allowlisted roots), and `.git` is denied.

| Tool             | Description                                                                     |
| ---------------- | ------------------------------------------------------------------------------- |
| `read_file`      | Read a file as text with **1-based** numbered lines; optional `offset`/`limit`  |
| `read_raw`       | Read any binary file (≤ 10 MiB). Images are returned as image content, audio as audio content, everything else as a base64-encoded blob resource; the mime type comes from the extension, falling back to magic-byte sniffing |
| `write_file`     | Write a file, overwriting any existing contents                                 |
| `edit_file`      | Replace lines `[start_line, end_line)` (**1-based**, end exclusive) with new text; omit `end_line` to insert |
| `move_file`      | Move or rename a file or directory                                              |
| `list_directory` | List the entries of a directory                                                 |
| `make_directory` | Create a directory, including parents                                           |
| `glob`           | Find files by glob pattern (`*` stays within a segment, `**` crosses directories), e.g. `**/*.rs` |
| `grep`           | Search file contents with a regular expression; returns `path:line:text`        |

### Build

Each takes a `project_dir` (project root) and `arguments` — the **full** argument list,
*including the subcommand/task as the first element*. Commands run directly without a shell,
and their output is **streamed** back via MCP progress notifications as it is produced.

| Tool     | Runs                    | Example `arguments`                                       |
| -------- | ----------------------- | -------------------------------------------------------- |
| `cargo`  | `cargo …`               | `["build", "--release"]`, `["test", "--", "--nocapture"]` |
| `gradle` | `./gradlew …` (wrapper) | `["build"]`, `["test", "--info"]`                        |

### Git

| Tool       | Runs         | Example `arguments`                                  |
| ---------- | ------------ | ---------------------------------------------------- |
| `git_diff` | `git diff …` | `["--stat"]`, `["HEAD~1"]`, `["--", "src"]`, `[":(exclude)Cargo.lock"]` |

`folder` is any directory inside the git repository — it selects the repository when several are
present. `arguments` are appended verbatim after the subcommand; only `diff` is exposed for now.

### GitHub

Wraps the [`gh`](https://cli.github.com) CLI; it must be installed and authenticated
(`gh auth status`).

| Tool             | Runs               | Example `arguments`                                     |
| ---------------- | ------------------ | ------------------------------------------------------- |
| `gh_issue_view`  | `gh issue view …`  | `["123"]`, `["123", "--comments"]`, `["1", "--repo", "cli/cli"]` |
| `gh_pr_view`     | `gh pr view …`     | `["456"]`, `["--json", "title,state,body"]`, `["9000", "--repo", "cli/cli"]` |

`folder` is any directory inside the git repository — it selects the repository when several
are present, and `gh` infers the remote from it unless `--repo owner/name` is passed.
`arguments` are appended verbatim after the subcommand.

### Smart lights

Controls WLED devices discovered on the local network.

| Tool               | Description                                                                  |
| ------------------ | ---------------------------------------------------------------------------- |
| `lights_info`      | Returns the available smart lights (id, name, hostname, address)             |
| `lights_set_color` | Sets a light (by `id`) to an RGB color, each component in the `0.0`–`1.0` range. Run `lights_info` first to populate the device cache |

## Harness config

Use the **HTTP** transport (`http://localhost:9999/mcp`) when the server is already running,
or the **stdio** transport when you want the harness to spawn the binary itself. For stdio,
always use an **absolute path** to the binary and pass the `stdio` argument.

### Claude CLI

Add the server with `claude mcp add`:

```sh
# HTTP (server already running on :9999)
claude mcp add --transport http mdev http://localhost:9999/mcp

# stdio (CLI spawns the binary)
claude mcp add mdev /absolute/path/to/mcp-server/target/release/mcp-server stdio
```

List or remove it with `claude mcp list` / `claude mcp remove mdev`.

**Permissions.** MCP tools are addressed as `mcp__mdev__<tool>`, or just `mcp__mdev` for the
whole server. Allow the server in one line via the `permissions` block in
`.claude/settings.json` (or run `/permissions` in the CLI):

```json
{
  "permissions": {
    "allow": ["mcp__mdev"]
  }
}
```

Anything not matched falls back to the interactive prompt. (MCP tool patterns don't support
trailing wildcards — use the bare server name to cover every tool, or list tools individually
when you want finer control.)

### Zed

Zed loads custom MCP servers from its `settings.json` (open with `zed: open settings`, or
edit `~/.config/zed/settings.json`). Zed spawns the binary, so use the stdio transport:

```json
{
  "context_servers": {
    "mdev": {
      "source": "custom",
      "command": "/absolute/path/to/mcp-server/target/release/mcp-server",
      "args": ["stdio"],
      "env": {}
    }
  }
}
```

Use an **absolute path** for `command`; Zed does not resolve relative paths or `~`.

**Permissions.** Two layers: a profile decides which tools are *visible*, and
`agent.tool_permissions` decides whether they may *run*. Turning on
`enable_all_context_servers` exposes every `mdev` tool at once, and a global `"allow"`
default lets them run without confirmation:

```json
{
  "agent": {
    "profiles": {
      "default": {
        "enable_all_context_servers": true
      }
    },
    "tool_permissions": {
      "default": "allow"
    }
  }
}
```

`tool_permissions.default` accepts `"allow"`, `"confirm"`, or `"deny"` and applies to *all*
tools, built-in ones included — Zed has no server-wide pattern for MCP tools, so to keep the
looser default scoped you have to name them individually as `mcp:mdev:<tool>` under
`tool_permissions.tools`. (Requires Zed v0.224.0 or later.)

### opencode

opencode reads MCP servers from `opencode.json` (project root or `~/.config/opencode/`).
It supports both a `remote` (HTTP) and a `local` (stdio) server type:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "mdev": {
      "type": "remote",
      "url": "http://localhost:9999/mcp",
      "enabled": true
    }
  }
}
```

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "mdev": {
      "type": "local",
      "command": ["/absolute/path/to/mcp-server/target/release/mcp-server", "stdio"],
      "enabled": true
    }
  }
}
```

**Permissions.** opencode exposes MCP tools as `<server>_<tool>` (e.g. `mdev_read_file`) and
gates them with the top-level `permission` block. Each entry is `"allow"`, `"ask"`, or
`"deny"`, and keys accept glob patterns — so one `mdev_*` entry covers the whole server:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "permission": {
    "mdev_*": "allow"
  }
}
```

More specific keys win over wildcard patterns, so you can still carve out individual tools
(e.g. adding `"mdev_write_file": "ask"`) on top of the blanket entry.
