# ClaudeTile

English | [简体中文](README.zh-CN.md)

A desktop status tile for Claude on Windows: see what your Claude Code sessions are doing (CLI and the Claude desktop app), your 5-hour and weekly usage limits, and a GitHub-style heatmap of the tokens you use every day.

<p align="center">
  <img src="docs/images/tile-en.png" width="380" alt="ClaudeTile showing usage limits, sessions and a token heatmap">
</p>

- A single ~720 KB exe, about 13 MB of private memory, close to 0% CPU when idle.
- Follows the system light / dark theme; adjustable opacity, down to a see-through tile.
- English and Chinese interface.

> ClaudeTile is an unofficial community tool. It is not affiliated with or endorsed by Anthropic. "Claude" is a trademark of Anthropic.

## Install

Download `ClaudeTile-Setup-<version>.exe` from [Releases](https://github.com/ArthurLauCS/ClaudeTile/releases/latest) and run it. Requires 64-bit Windows 10 or 11; Windows 11 is recommended (rounded corners and shadows).

- The installer is not signed with a commercial code-signing certificate, so Windows may show "Windows protected your PC". Click **More info** → **Run anyway**.
- It installs for the current user only and does not ask for administrator permission.
- The language you pick in setup becomes the interface language. You can change it later from the menu.
- To see usage limits you need [Claude Code](https://docs.anthropic.com/en/docs/claude-code) signed in with a Claude subscription (Pro / Max). Sessions and token statistics work with any Claude Code login.

## What it shows

**Account details.** The main panel follows Sync / Wake-up → Accounts → Sessions → Token heatmap / Today's projects → Reset signals, separated by thin rules. Each account shows its email, plan, 5-hour and weekly usage percentages, bars, and reset times. Hovering a card highlights it; the current account has a stronger highlight. Unavailable fields stay explicitly unavailable rather than appearing as zero.

**Accounts.** Use several Claude subscriptions: **Add account…** in the menu opens a command window that runs `claude auth login` for the new account. With two or more accounts the usage box lists each one with its 5-hour bar and weekly percentage; click an account to switch Claude Code to it (new sessions use it; restart running ones). Turn on **Auto-start idle 5h windows** and the tile sends each account a tiny Haiku message (about 3k tokens) whenever its 5-hour window is not running, so the window starts counting while you are away.

**Reset signals.** Two compact footer rows show Codex / Claude status from [codexreset.vercel.app](https://codexreset.vercel.app/en), refreshed every five minutes. The tile uses the source status directly; failures show Unknown and source data older than three hours shows Stale. Click a row to open its embed page. The public request sends no account or token. These social-media signals do not confirm a personal quota reset.

**Sessions.** Every running Claude Code session, from terminals and from the Claude desktop app: title, project, the tool it is running right now, and how long it has been in its current state. Sessions that need you (a permission prompt, a question) are listed first. Click a session to jump to its window.

**Token heatmap and today's projects.** Switch between daily cells and weekly bars; hover or select a period to see its total. A separate list shows today's project usage. Each section has a close button and can be restored independently from the right-click menu; preferences persist. Cache reads are excluded unless enabled with the Cache switch. Daily statistics retain the Windows local date; quota reset, sync and wake-up times use the time zone chosen in the menu (Beijing by default, or local / UTC).

**Tray icon.** Shows the 5-hour usage percentage. Its color shows the state:

| Color | Meaning |
|---|---|
| Yellow | A session is waiting for you |
| Red | Something is wrong: login expired, usage API changed, or 5-hour usage at 90% or more |
| Green | A session is running |
| Terracotta | Idle |

**Notifications.** Cards in the lower-right corner when a session waits for you, when a limit reaches your alert level (70 / 80 / 90%, or off), or when the 5-hour limit will run out within 30 minutes. Click a card to jump to that session. They hold off while you are in a full-screen game or presenting.

<p align="center">
  <img src="docs/images/toast-en.png" width="372" alt="Notification card: a session needs approval">
</p>

## Using the tile

- **Move:** drag the tile anywhere. It always stays fully on screen.
- **Width:** drag the left or right edge. The heatmap shows more weeks when wider.
- **Section heights:** drag the gap below the limits, sessions or token section. Each section scrolls with the mouse wheel when its content is taller. The sessions section shows 5 rows by default.
- **Always on top:** the pin in the upper-right corner. When unpinned, the tile behaves like a normal window: clicking or dragging it brings it above other windows, and clicking another window covers it again.
- **Menu:** right-click the tile or the tray icon. Opacity slider, always on top, auto-start idle 5h windows, start with Windows, usage alert level, language, time zone, add account, refresh, reset layout, hide the tile, exit. Switches and options keep the menu open, so you can change several at once; press Esc or click elsewhere to close it.
- **Show / hide:** left-click the tray icon.

<p align="center">
  <img src="docs/images/menu-en.png" width="272" alt="Right-click menu with opacity slider and switches">
</p>

## Where the data comes from

ClaudeTile only reads what Claude Code already keeps on your computer, and never changes it.

| What | Source |
|---|---|
| Session state | `%USERPROFILE%\.claude\sessions\*.json`, written by Claude Code itself |
| Session titles, current tool, tokens | Conversation transcripts in `%USERPROFILE%\.claude\projects` |
| Desktop app session titles | `%APPDATA%\Claude\claude-code-sessions` |
| Usage limits | `https://api.anthropic.com/api/oauth/usage`, using the sign-in token Claude Code saved in `%USERPROFILE%\.claude\.credentials.json` |
| Other accounts | `%APPDATA%\ClaudeTile\accounts\<account>`, each the `CLAUDE_CONFIG_DIR` the account signed in with. Switching copies the current account's `.credentials.json` and the `oauthAccount` entry of `%USERPROFILE%\.claude.json` there and the chosen account's back |

- This is the only network request ClaudeTile makes, about every 2 minutes. There is no telemetry. It uses the proxy from your environment variables or from `env` in `~/.claude/settings.json`, the same one Claude Code uses.
- The usage endpoint is not a public API and may change. If it does, the tile and a notification tell you that ClaudeTile needs an update; sessions and token statistics keep working.
- ClaudeTile never refreshes the sign-in token itself. When it expires, open Claude Code once and it refreshes.
- Claude Code deletes transcripts older than 30 days by default, so ClaudeTile keeps its own daily totals. The heatmap keeps your history beyond 30 days.

ClaudeTile's own files are in `%APPDATA%\ClaudeTile`: `config.txt` (position, sizes, options) and `history.txt` (daily token totals). When uninstalling, you are asked whether to delete them; the default is to keep them.

## Build from source

Requires Rust (stable, edition 2024) on Windows.

```powershell
cargo build --release          # target\release\claudetile.exe
cargo test --release           # unit tests
```

- App icon: edit `assets/icon.svg` (40 px and up) or `assets/icon-small.svg` (16–32 px), then run `python scripts/icon.py`. Needs Chrome, Pillow and the Windows SDK (`rc.exe`). The generated `assets/app.res` is committed, so a normal build does not need the SDK.
- Installer: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/installer.ps1`. Needs [Inno Setup 6](https://jrsoftware.org/isinfo.php). Output: `target\installer\ClaudeTile-Setup-<version>.exe`.

Command-line options for testing:

| Option | Effect |
|---|---|
| `--claude-dir <dir>` | Read this directory instead of `~/.claude` |
| `--data <dir>` | Keep settings and history here instead of `%APPDATA%\ClaudeTile` |
| `--usage-file <file>` | Use this JSON instead of calling the usage endpoint |
| `--dump <file>` | Do not open a window; write what was read (sessions, daily tokens, limits) to a text file |
| `--top`, `--light`, `--dark`, `--lang zh\|en` | Force always on top, a theme or a language |

The scripts in `scripts/` capture the tile, menu and notification windows and send them clicks and scrolls without moving the real mouse.

## License

[MIT](LICENSE)
