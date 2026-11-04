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

### Keynote

Reads and edits Apple Keynote presentations (`*.key`, Keynote 6 and later) in pure Rust. It
works on any OS and doesn't need Keynote. A `.key` file is a zip of Snappy-compressed protobuf
archives (`Index/*.iwa`). Apple doesn't document the format, so the field numbers come from the
schemas [keynote-parser](https://github.com/psobot/keynote-parser) extracted from Keynote 14.5.
Fields the tools don't know about are carried over unchanged. Only single-file presentations are
supported, not package folders.

New slides are made from one of the theme's **layouts**, the same way Keynote does it (copies of
the layout's placeholders, with the sample text cleared), or **duplicated** from an existing slide
along with all its content and animations. New presentations take their theme from a
**template**: any `.key` file, `mcp-resources/keynote/template.key` by default. The template's
own slides are dropped.

| Tool                     | Description                                                                 |
| ------------------------ | --------------------------------------------------------------------------- |
| `keynote_create`         | Create a presentation from `template`'s theme with the given `slides`       |
| `keynote_info`           | Slide size, the theme's layouts with their placeholders, and every slide's layout, background, title, body, notes, skipped flag, transition, builds, build order and objects (id, kind, frame, text, fill, stroke, image). `slides: [from, to]` limits it, `summary: true` gives one line per slide |
| `keynote_add_slides`     | Add `slides` at the end, or at a 1-based `position`; every slide is checked before anything changes, only the new slides are returned |
| `keynote_edit_slide`     | Set a slide's `title`, `body`, `notes`, `skipped`, `background`, `show_title` / `show_body` / `show_slide_number`, `transition` and `builds` |
| `keynote_move_slide`     | Move a slide to another 1-based `position`                                   |
| `keynote_delete_slides`  | Delete slides by their 1-based numbers                                       |
| `keynote_add_image`      | Place an image (png, jpg, gif, heic, tiff, pdf, svg): `frame`, `fit` (contain, cover, stretch, original), `opacity`, `corner_radius`, `rotation`, `stroke`, `shadow`, `description`, or `placeholder: true` to fill the layout's photo |
| `keynote_add_shape`      | Add a shape (rect, rounded_rect, ellipse, triangle, diamond, polygon, star, arrow, chevron, plus, callout, SVG `path`) with `fill`, `stroke`, `shadow`, `opacity` and optional `text` |
| `keynote_add_text_box`   | Add a free text box with styled `text`, `text_inset`, `vertical_align`, `columns`, `autosize: "shrink"` and optional fill or line |
| `keynote_add_line`       | Draw a line `from` → `to` with `color`, `width`, `dash` and `head` / `tail` ends |
| `keynote_add_code`       | Add a syntax-highlighted code block from `code` or a source `file`: `language`, `theme`, `font`, `size`, `line_height`, `highlight_lines`, `line_numbers` |
| `keynote_edit_object`    | Change an object by `id`: `frame` (any of x/y/w/h), `rotation`, `z`, fill/stroke/shadow/opacity, `text`, text layout, `locked`, `description`, `link` |
| `keynote_delete_objects` | Delete objects by `ids`, with their builds                                   |
| `keynote_export`         | Render slides to PNG or SVG previews in `out_dir` and report text that doesn't fit its frame |
| `keynote_batch`          | Apply many operations (`op`: add_slides, edit_slide, move_slide, delete_slides, add_image, add_shape, add_text_box, add_line, add_code, edit_object, delete_objects) in one save; `ref` names created objects for later `"$name"` use; `dry_run` saves nothing |

Geometry is in slide points with the origin top-left, colours are `#RRGGBB` or `#RRGGBBAA`,
rotation is in degrees clockwise. Every object has a stable id (`"id:N"`) that object tools,
build targets and z orders (`"above:id:N"`, `"below:id:N"`, `"front"`, `"back"`) take. Add and
edit tools return the object's id and its slide, with warnings about overflowing text.

**Text.** `title`, `body` and object `text` take a plain string (one paragraph per line, each
keeping the style of the paragraph at the same position), or styled paragraphs: a paragraph
object or a list of them. A paragraph has `text`, `markdown` or `runs`, character formatting
(`font`, `size`, `bold`, `italic`, `underline`, `strike`, `color`, `highlight`, `baseline`,
`link`, `monospace`) applying to all its runs, and paragraph formatting (`align`, `bullet`,
`indent_level`, `line_spacing`, `line_height`, `space_before`, `space_after`). Markdown-lite
understands `**bold**`, `*italic*`, `~~strike~~`, `` `code` ``, `[text](url)`, and list items
starting with `- ` or `1. `, nested by two spaces. Styling is stored the way Keynote stores it:
variations of the theme's styles, shared between objects that look the same.

```json
{ "path": "deck.key", "slide": 2, "frame": { "x": 96, "y": 290, "w": 400, "h": 200 },
  "text": [
    { "runs": [ { "text": "76% → 24%", "size": 84, "bold": true, "color": "#6CC38B" } ] },
    { "markdown": "Android bugs that are *memory-safety* bugs", "size": 26, "color": "#A8A39A" }
  ],
  "fill": "#23222A", "stroke": { "color": "#2E2D35", "width": 2 }, "text_inset": 24 }
```

Changing text drops inline objects such as fields in that text box (links written by the tools
are kept). Keynote redraws slide thumbnails the next time it opens the file. Don't edit a file
while Keynote has it open, because Keynote's next save overwrites the changes.

**Animations.**

- **Transitions** play between slides. `transition` sets the one leaving a slide:
  `{ "effect", "duration", "delay", "automatic" }`. Effect `"none"` removes it.
- **Builds** animate objects within a slide. `builds` replaces all of a slide's builds; they play
  in list order, and an empty list removes them. Each build is
  `{ "target", "kind", "effect", "delivery", "duration", "delay", "automatic" }`:
  - `target` is `"title"`, `"body"`, `"id:N"` or `"object N"`, a 1-based index into the slide's
    `objects` in `keynote_info`. Only objects shown on the slide qualify.
  - `kind` is `"In"` (the default) or `"Out"`.
  - `delivery` `"By Bullet"` or `"By Highlighted Bullet"` takes one step per paragraph (a code
    block then reveals line by line).
  - `automatic` starts the first step right after the previous one instead of on click.

Effects are Keynote's internal identifiers. These are verified in real presentations:
`apple:dissolve`, `apple:fade-and-move` and `apple:magic-move-implied-motion-path` for
transitions, and `apple:dissolve`, `apple:dissolve character` and
`com.apple.iWork.Keynote.FromDarkness` for builds. Run `keynote_info` on an existing deck to find
others.

```json
{
  "path": "deck.key",
  "operations": [
    { "op": "add_slides", "slides": [ { "layout": "Title & Bullets", "title": "The 70% problem",
      "show_body": false, "background": "#17161B" } ] },
    { "op": "add_shape", "ref": "tile", "slide": 2, "shape": "rounded_rect",
      "frame": { "x": 96, "y": 290, "w": 400, "h": 200 }, "fill": "#23222A",
      "text": { "markdown": "**70%** of bugs", "size": 40, "color": "#F4EFE6" } },
    { "op": "edit_slide", "slide": 2,
      "builds": [ { "target": "$tile", "effect": "apple:dissolve" } ] }
  ]
}
```

**Previews.** `keynote_export` draws slides without Keynote: backgrounds, shapes, lines, images
(PNG, JPEG, GIF, WebP), masks, fills, strokes, shadows and styled text with wrapping, alignment,
spacing and bullets, measured with this machine's fonts. It is close to Keynote, not identical;
tables, charts, movies and effects show as labelled boxes. Text taller than its frame is reported
as a warning.

Not supported yet: tables, charts, groups and alignment of objects, connectors, slide links,
custom bullet characters, changing a slide's layout, editing layouts and the theme, and
PDF/PPTX export. Object archives without an example saved by Keynote (lines, masks, line ends,
gradients, image backgrounds) follow keynote-parser's schemas and are unverified in Keynote.

Tests run against two presentations saved by Keynote at the repository root: `Rust.key` and
`template.key` (`cargo test -p mcp-server keynote`). `explore.rs` holds an ignored test that dumps
archives of a deck to `target/explore.txt` for reverse engineering (settings in
`target/explore.cfg`).

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
