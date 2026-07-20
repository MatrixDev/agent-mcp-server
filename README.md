# mcp-server

A [Model Context Protocol](https://modelcontextprotocol.io) server (`mdev`) that exposes
file-system, build, benchmark, and smart-light tools to an agent. It runs build commands
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
| `read_image`     | Read an image file, returned as base64-encoded image content (≤ 10 MiB)         |
| `write_file`     | Write a file, overwriting any existing contents                                 |
| `edit_file`      | Replace `lines_count` lines from **1-based** `start_line` with new text (`lines_count: 0` inserts) |
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

### Benchmarks

| Tool             | Description                                                                        |
| ---------------- | ---------------------------------------------------------------------------------- |
| `ieee1905_bench` | Runs the `ieee1905` release binary under a timeout and returns its resource-usage report |

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

**Permissions.** MCP tools are addressed as `mcp__mdev__<tool>` (or `mcp__mdev` for the whole
server). Allow or deny them via the `permissions` block in `.claude/settings.json` (or run
`/permissions` in the CLI):

```json
{
  "permissions": {
    "allow": ["mcp__mdev__read_file", "mcp__mdev__grep", "mcp__mdev__cargo_build"],
    "deny": ["mcp__mdev__write_file"]
  }
}
```

Anything not matched falls back to the interactive prompt. Use `mcp__mdev` to cover every
tool on the server at once. (MCP tool patterns don't support trailing wildcards — list the
tools, or the server name for all of them.)

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
`agent.tool_permissions` decides whether they may *run*. MCP tools are addressed as
`mcp:mdev:<tool>`:

```json
{
  "agent": {
    "profiles": {
      "default": {
        "enable_all_context_servers": false,
        "context_servers": {
          "mdev": {
            "tools": { "read_file": true, "grep": true, "cargo_build": true }
          }
        }
      }
    },
    "tool_permissions": {
      "default": "confirm",
      "tools": {
        "mcp:mdev:read_file": { "default": "allow" },
        "mcp:mdev:grep": { "default": "allow" },
        "mcp:mdev:cargo_build": { "default": "allow" }
      }
    }
  }
}
```

Per-tool `default` accepts `"allow"`, `"confirm"`, or `"deny"`, and overrides the global
`tool_permissions.default`. (Requires Zed v0.224.0 or later.)

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
`"deny"`, and keys accept glob patterns:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "permission": {
    "mdev_*": "ask",
    "mdev_read_file": "allow",
    "mdev_grep": "allow",
    "mdev_write_file": "deny"
  }
}
```

More specific keys win over wildcard patterns, so the example asks for any `mdev` tool by
default while allowing the read-only ones and denying writes.
