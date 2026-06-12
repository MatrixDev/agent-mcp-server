# mcp-server

A small [Model Context Protocol](https://modelcontextprotocol.io) server that runs
`cargo` subcommands directly (without spawning a terminal shell) and returns their
combined stdout/stderr.

## Tools

Each tool takes two parameters:

- `working_directory` — absolute path the command is run from. The agent should pass its
  own working directory (the project root), so cargo operates on the right crate.
- `arguments` — array of extra flags appended to the cargo subcommand.

| Tool    | Runs           | Example `arguments`        |
| ------- | -------------- | -------------------------- |
| `build` | `cargo build`  | `["--release"]`            |
| `check` | `cargo check`  | `["--all-targets"]`        |
| `test`  | `cargo test`   | `["--", "--nocapture"]`    |
| `clippy`| `cargo clippy` | `["--", "-D", "warnings"]` |

## Build

```sh
cargo build --release
```

The binary is produced at `target/release/mcp-server`. It speaks MCP over stdio.

## Zed configuration

Zed loads custom MCP servers from its `settings.json` (open with
`zed: open settings` from the command palette, or edit `~/.config/zed/settings.json`).
Add the server under `context_servers`:

```json
{
  "context_servers": {
    "cargo-runner": {
      "source": "custom",
      "command": "/absolute/path/to/mcp-server/target/release/mcp-server",
      "args": [],
      "env": {}
    }
  }
}
```

Notes:

- Use an **absolute path** for `command`; Zed does not resolve relative paths or `~`.
- `args` and `env` are optional — the server reads its input from stdin and needs no
  arguments. Set `env` if you want to point the spawned `cargo` at a specific
  toolchain or working directory (e.g. `"env": { "RUSTUP_TOOLCHAIN": "stable" }`).
- `cargo` must be on the `PATH` of the environment Zed launches the server in.

After saving `settings.json`, restart the server from Zed's Agent panel (or reopen the
project) and the `build`, `check`, `test`, and `clippy` tools become available to the
agent.

## Agent profiles

Zed's agent uses **profiles** to decide which tools and context servers are exposed in a
conversation. To make sure the `cargo-runner` tools are turned on, configure
`agent.profiles` in the same `settings.json`. Each profile lists built-in `tools` and,
per context server, which of its tools are enabled:

```json
{
  "agent": {
    "default_profile": "cargo-dev",
    "profiles": {
      "cargo-dev": {
        "name": "Cargo Dev",
        "tools": {
          "find_path": true,
          "read_file": true,
          "grep": true,
          "diagnostics": true
        },
        "enable_all_context_servers": false,
        "context_servers": {
          "cargo-runner": {
            "tools": {
              "build": true,
              "check": true,
              "test": true,
              "clippy": true
            }
          }
        }
      }
    }
  }
}
```

Notes:

- The `cargo-runner` key under `context_servers` must match the name you used in the
  top-level `context_servers` block above.
- `enable_all_context_servers: true` exposes every context server's tools without
  listing them individually; set it to `false` (as above) when you want to opt in
  per-tool.
- `default_profile` selects which profile new threads start with — you can also switch
  profiles from the profile selector in the Agent panel.
- Zed ships built-in `write` and `ask` profiles; defining `cargo-dev` adds to them
  rather than replacing them.

## Tool permissions

Enabling a tool in a profile only makes it *visible* to the agent — whether it's allowed
to *run* is governed separately by `agent.tool_permissions`. If the agent reports
something like **"Blocked by global default: deny"**, your global default is denying the
call and you need to allow these tools explicitly.

Context-server (MCP) tools are addressed as `mcp:<server>:<tool>`, where `<server>` is the
`context_servers` key (`cargo-runner` here). Add an `agent.tool_permissions` block to
`settings.json`:

```json
{
  "agent": {
    "tool_permissions": {
      "default": "confirm",
      "tools": {
        "mcp:cargo-runner:build": { "default": "allow" },
        "mcp:cargo-runner:check": { "default": "allow" },
        "mcp:cargo-runner:test": { "default": "allow" },
        "mcp:cargo-runner:clippy": { "default": "allow" }
      }
    }
  }
}
```

Notes:

- Per-tool `default` accepts `"allow"`, `"confirm"`, or `"deny"`. Use `"allow"` to run
  without prompting, or `"confirm"` to be asked each time.
- The per-tool setting overrides the global `tool_permissions.default`, so these four
  tools run even when the global default is `deny`.
- MCP tools only support this tool-level `default`; the regex-based `always_allow` /
  `always_deny` / `always_confirm` rules apply to built-in tools (like `terminal`), not
  to context-server tools.
- This setting requires Zed **v0.224.0** or later.
