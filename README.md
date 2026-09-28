# Spectra — a native Rust browser MCP server

[![License: PolyForm Shield 1.0.0](https://img.shields.io/badge/license-PolyForm%20Shield%201.0.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021-orange?logo=rust)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/protocol-MCP-green)](https://modelcontextprotocol.io)

Spectra is a source-available, free-to-use MCP server that gives AI agents fast, token-efficient control of Chrome over the Chrome DevTools Protocol.

Spectra drives Chrome directly through the Chrome DevTools Protocol (CDP), without Playwright's abstraction layer, to give an AI agent (Claude Code or any MCP client) fast, information-dense browser control that can be extended per project.

See [`docs/COMPARISON.md`](docs/COMPARISON.md) for the measured comparison against Playwright MCP on a real application (the document is written in French; it opens with an English summary). A French version of this README is available in [`README.fr.md`](README.fr.md).

## Why

- **Native Rust binary**: ~27 MB of RAM for the server process (versus ~130 MB for an equivalent Node.js process), no runtime to install.
- **Direct CDP**: no client → relay server → browser double hop.
- **Condensed snapshot**: pruned accessibility tree (presentational roles removed) with short `[eN]` refs; a screenshot is not the only source of truth.
- **Extensible per project**: each repository can define its own scenarios (`.spectra/recipes/*.mjs`) without ever recompiling the server.
- **Multi-session**: drives several Chrome instances in parallel (configurable limit), useful to simulate multiple users.
- **Native Core Web Vitals**: LCP/CLS/FCP/TTFB measured through the browser's native APIs (`PerformanceObserver`), with no external dependency (no Lighthouse, no web-vitals.js).
- **Secure by construction**: stdio transport only (`rmcp` feature `transport-io`, never HTTP/SSE). No local network socket is ever opened, which structurally eliminates the whole class of DNS rebinding vulnerabilities that affected other browser MCP servers exposing a local HTTP transport (e.g. CVE-2025-9611 on Playwright MCP). Nothing to configure, nothing to audit on this point: the attack vector simply does not exist in this architecture.

## Installation

### Prerequisites

- Rust (2021 edition, stable toolchain) — `rustup` with the `x86_64-pc-windows-msvc` target
- On Windows: Visual Studio Build Tools with the **Desktop development with C++** workload (the MSVC compiler) **and** the **Windows SDK — Desktop C++ x64/x86** component (it contains `kernel32.lib` and the other Win32 libraries; often missing from a minimal Build Tools installation, so add it explicitly)
- Node.js **22+** on the PATH (only required for `browser_run_recipe`, degrades gracefully if absent). The recipe runtime (`recipe-runner.mjs`) uses Node's native global `WebSocket`, which is only stable from version 22; on an earlier version, `browser_run_recipe` would fail with `WebSocket is not defined`
- Google Chrome installed (detected automatically in the standard locations)

### Build

```powershell
cargo build --release
```

The `release` profile uses `lto = "thin"` rather than `lto = true` (fat LTO). Full LTO crashed LLVM with an out-of-memory error on a standard development machine while this project was being tuned; `thin` captures most of the performance gain without that risk.

The resulting binary is at `target/release/spectra-server.exe`.

### Permanent installation

```powershell
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\.spectra\bin"
Copy-Item "target\release\spectra-server.exe" "$env:USERPROFILE\.spectra\bin\spectra-server.exe"
```

## Enabling it in Claude Code

Manually add the following entry to your MCP configuration (`~/.claude.json` or the equivalent for your platform, `mcpServers` section):

```json
{
  "mcpServers": {
    "spectra": {
      "command": "%USERPROFILE%\\.spectra\\bin\\spectra-server.exe"
    }
  }
}
```

If your client does not expand environment variables in this field, use the absolute path of the installed binary instead.

Once added, `/mcp` in Claude Code should list `spectra` with its 21 tools available.

## Emergency cleanup

If test Chrome instances pile up (this happens when an MCP client crashes without shutting the server down cleanly):

```powershell
powershell -File scripts\kill-all-spectra.ps1
```

This script targets only the Chrome processes launched by Spectra (identified by their `--user-data-dir` under `%LOCALAPPDATA%\Spectra\profiles`); it never touches your personal Chrome.

Alternative from within Claude Code, without leaving the conversation:

```
browser_sessions(action="close_all")
```

## Environment variables

| Variable | Purpose |
|---|---|
| `SPECTRA_MAX_SESSIONS` | Maximum number of simultaneous Chrome sessions (default: 6) |
| `SPECTRA_CHROME_PATH` | Explicit path to a Chrome/Chromium binary, if Spectra does not find it automatically in the standard locations |
| `SPECTRA_MINIMAL_CHROME` | Set to `0` to disable the 7 additional launch flags (GPU, notifications, etc. — see docs/COMPARISON.md, section V10) that reduce Chrome's RAM footprint by about 4%. Enabled by default |
| `SPECTRA_WINDOW_SIZE` | Window + CDP viewport resolution, format `"1920x1080"` (default: `1920x1080`, dense 16:9) |
| `SPECTRA_PREWARM` | Set to `1` to pre-warm a Chrome as soon as the server starts (20 s TTL if not consumed). Disabled by default: no Chrome opens until an explicit `browser_launch`/`browser_navigate` is called |
| `SPECTRA_STEALTH` | Set to `1` to disable the most visible automation-detection signals on the page side (`navigator.webdriver`, the "Chrome is being controlled" banner, disabled GPU), to drive a third-party site that blocks detected automated browsers. Disabled by default (`SPECTRA_MINIMAL_CHROME` remains the normal mode, optimized for performance rather than discretion). Combine with `ctx.addInitScript()` on the recipe side for a more thorough JS evasion script (hiding `navigator.plugins`, WebGL fingerprint, etc.). See [Responsible use](#responsible-use) |
| `SPECTRA_USER_AGENT` | Custom user agent (useful in stealth mode — a default user agent containing `HeadlessChrome` is itself a detection signal); has no effect unless `SPECTRA_STEALTH` is enabled |

## Tool catalog

| Tool | Description |
|---|---|
| `browser_launch` | Starts or attaches Chrome for the current project |
| `browser_snapshot` | Captures the condensed accessibility tree (text + `[eN]` refs, with `[disabled]`/`[checked]`/`[expanded]`/... state when present). With `diff_only=true`, returns only a compact summary of nodes added/removed/changed since the previous snapshot of this page (structural diff by `backendDOMNodeId`, not by line text). With `check_a11y=true`, adds an `a11y_issues` list of basic accessibility problems detected. With `detail="compact"` or `"refs_only"`, reduces verbosity to save tokens (see `docs/COMPARISON.md`, section V8) |
| `browser_navigate` | Navigates to a URL, waits for the application to mount |
| `browser_click` | Clicks a referenced element. Supports `diff_only=true` |
| `browser_type` | Types text, with an optional submit (Enter). Supports `diff_only=true` |
| `browser_fill_form` | Fills several fields in a single call. Supports `diff_only=true` |
| `browser_select_option` | Selects a `<select>` option. Supports `diff_only=true` |
| `browser_hover` | Hovers an element. Supports `diff_only=true` |
| `browser_screenshot` | Image capture (reserved for visual checks) |
| `browser_wait_for` | Waits for a text to appear and/or for a ref to designate an element that is actually rendered (valid `DOM.getBoxModel`), with a configurable timeout |
| `browser_console_messages` | Deduplicated console messages, with a counter |
| `browser_network_requests` | Network requests grouped by domain/status |
| `browser_network_request_detail` | Full detail of a request (headers, status) |
| `browser_evaluate` | Runs a JavaScript expression in the page |
| `browser_performance_metrics` | Measures the Core Web Vitals of the active page (LCP, CLS, FCP, TTFB) |
| `browser_tabs` | Lists/opens/closes/selects a tab |
| `browser_file_upload` | Uploads files to an `<input type=file>` |
| `browser_run_recipe` | Runs a `.mjs` recipe from the current project |
| `browser_sessions` | Lists, closes one, or closes all Chrome sessions |
| `browser_report` | Compiles the session log (navigations, actions, errors) into a structured report |
| `browser_act_sequence` | Runs a sequence of actions (`click`/`type`/`select_option`) in a single call, without going back through a reasoning turn between steps. With `diff_only=true`, adds a compact summary of the change (captured once, after the last step) |

## Per-project extensibility

Create `<your-project>/.spectra/recipes/<name>.mjs`:

```js
export default async function loginAdmin(ctx, args) {
  await ctx.navigate(`${process.env.SPECTRA_BASE_URL}/login`);
  const snap = await ctx.snapshot();

  const emailRef = ctx.findRef(snap, { role: "textbox", name: /email/i });
  await ctx.type(emailRef, args.email);

  const pwRef = ctx.findRef(snap, { role: "textbox", name: /password/i });
  await ctx.type(pwRef, args.password, { submit: true });

  await ctx.waitFor({ text: "Dashboard", timeoutMs: 8000 });
  return { loggedIn: true };
}
```

Call from Claude Code:

```
browser_run_recipe(project="my-project", recipe="login_admin", args={email: "test@example.com", password: "..."})
```

The Node runtime (`recipe-runner.mjs`, a minimal CDP client with no npm dependency) is embedded in the binary and auto-installed into `%LOCALAPPDATA%\Spectra\runtime\` on first call — nothing to install manually.

Secrets: pass them explicitly in the `args` of `browser_run_recipe` (there is no automatic `.env`/`.gitignore` mechanism for now).

## Multi-session management

By default, up to **6 simultaneous Chrome sessions** (configurable via `SPECTRA_MAX_SESSIONS`), one per project key. A second `browser_launch` on the same project always reuses the existing session rather than spawning a new one.

Useful for driving several browsers in parallel — for example, simulating several users testing collaborative editing on the same application.

**How to switch between sessions**: apart from `browser_launch` and `browser_run_recipe`, no tool accepts an explicit `project` parameter — they all operate on the server's "default project", the one from the last `browser_launch(project=...)` call. To act alternately on two sessions (`user_a`, `user_b`), call `browser_launch(project=...)` again before each block of actions targeting one or the other:

```
browser_launch(project="user_a") → browser_navigate(...) → browser_click(...)
browser_launch(project="user_b") → browser_navigate(...) → browser_click(...)
browser_launch(project="user_a") → browser_snapshot(...)   # resumes user_a's state, unchanged in the meantime
```

Each session keeps its own state (page, refs, history) independently of which one is "active" at the server level — verified under real conditions: navigating `user_b` to another page never affects `user_a`'s state, even after several round trips.

## Startup pre-warming

When `SPECTRA_PREWARM=1` is set, Spectra silently pre-warms a Chrome for the current directory (the most likely project) as soon as the server starts, without blocking the MCP handshake. If the first `browser_launch`/`browser_navigate` targets that same project, it instantly picks up this already-started Chrome. If it is never consumed, it closes automatically after 20 seconds — no risk of silent accumulation.

## Architecture

```
crates/
├── spectra-cdp/       # CDP client (launcher, session, snapshot, actions, network/console observers)
├── spectra-tools/     # Exposed MCP tools (schemas, rmcp routing)
└── spectra-server/    # Binary, MCP stdio transport
runtime/
└── recipe-runner.mjs  # Minimal JS CDP client (embedded in the binary via include_str!, auto-installed)
scripts/
└── kill-all-spectra.ps1  # Emergency cleanup of test Chrome instances
```

Main libraries: `chromiumoxide` (CDP client), `rmcp` (the official Rust MCP SDK), `tokio` (async runtime).

## Development history and known limitations

See [`docs/COMPARISON.md`](docs/COMPARISON.md) — a measured comparison against Playwright MCP and an honest account of each development wave (V1 to today), including abandoned attempts and bugs found by actively testing against a real application.

## Responsible use

`SPECTRA_STEALTH` and the other anti-detection features exist only to test and automate sites that you have the right to automate access to. Respect the terms of service of the sites you interact with, and do not use Spectra to bypass access controls, scrape content you are not allowed to collect, or otherwise act against the rules that apply to a site.

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). To report a vulnerability, see [SECURITY.md](SECURITY.md).

## License

Free to use, including commercially. You may not use it to build a competing product or service. Contributions welcome. See [LICENSE](LICENSE) (PolyForm Shield 1.0.0). Source-available, not open source.

## More from AstroQuest

- [Cortex](https://github.com/AstroQuestStudio/cortex)
- [Thonny AI](https://github.com/AstroQuestStudio/thonny-ai)
- [CATIA V5 MCP](https://github.com/AstroQuestStudio/catia-v5-mcp)
