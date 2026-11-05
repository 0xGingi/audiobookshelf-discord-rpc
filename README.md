# audiobookshelf-discord-rpc
Displays what you're listening to on audiobookshelf on discord!

Shows Book Name, Author, Progress, Genre or Chapter, and Cover Art. Works with books and podcasts.

![image](https://github.com/user-attachments/assets/2354b157-3b54-4b4b-8ab3-fa7d7f64fa56)

## Install

You'll need your Audiobookshelf URL and an [API key](#get-api-key-must-be-admin). The installer asks for them, checks they work, and sets it up to start when you log in. Run the same command again to update (your config is kept).

### Windows
Open PowerShell (no admin needed) and run:
```powershell
irm https://raw.githubusercontent.com/0xGingi/audiobookshelf-discord-rpc/main/install.ps1 | iex
```
Files live in `%LOCALAPPDATA%\AudiobookshelfDiscordRPC`. If you used the old installer, delete its `AudiobookshelfDiscordRPC` task in Task Scheduler.

### macOS (Apple Silicon) & Linux (x64)
```sh
curl -fsSL https://raw.githubusercontent.com/0xGingi/audiobookshelf-discord-rpc/main/install.sh | sh
```
The binary goes in `~/.local/bin`, config in `~/.config/audiobookshelf-discord-rpc`. It runs as a LaunchAgent on macOS and a systemd user service on Linux.

### Uninstall
```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/0xGingi/audiobookshelf-discord-rpc/main/install.ps1))) -Uninstall
```
```sh
curl -fsSL https://raw.githubusercontent.com/0xGingi/audiobookshelf-discord-rpc/main/install.sh | sh -s -- --uninstall
```

### Manual
Download the binary for your platform from [releases](https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/latest) and run it in a terminal. If there's no config yet, it walks you through creating one. Use `-c path/to/config.json` to pick the config file and `--setup` to redo it.

### Docker (Linux only, requires Discord on the same machine)
```
git clone https://github.com/0xgingi/audiobookshelf-discord-rpc
cd audiobookshelf-discord-rpc
cp config/config.json.example config/config.json   # then edit it
docker compose up -d
```

## Config

| Key | |
|---|---|
| `audiobookshelf_url` | e.g. `http://192.168.1.10:13378` (include the port unless you're behind a reverse proxy) |
| `audiobookshelf_token` | Your ABS API key |
| `show_chapters` | Show the current chapter instead of the genre |
| `imgbb_api_key` | Optional, see covers below |
| `use_abs_cover` | Optional. `true` links covers straight from your ABS server; only works if it's reachable from the internet |
| `discord_client_id` | Optional. Leave it out unless you've made your own Discord app |
| `slack_token` | Optional. Also shows what you're listening to as your Slack status, see below |
| `discord` | Optional. Set `false` to skip Discord, e.g. Slack-only on a machine without Discord |

**Covers:** Discord can't load images from a private server, so by default covers are looked up on Audible and other metadata providers (podcasts use their feed artwork). For your exact ABS covers, add a free [ImgBB API key](https://api.imgbb.com/) and they'll be uploaded there. Cover URLs are cached in `urls.json` next to the config; delete it to refetch them.

## Slack Status

Sets your Slack status to `🎧 Book — Author` while you listen. It clears when you stop and expires on its own after 10 minutes if the app isn't running. It replaces any status you had set yourself.

1. Go to [api.slack.com/apps](https://api.slack.com/apps) > **Create New App** > **From scratch**, and pick your workspace.
2. **OAuth & Permissions** > **User Token Scopes** > add `users.profile:write`.
3. **Install to Workspace** (your workspace may need an admin to approve it), then copy the **User OAuth Token** (`xoxp-...`) into `slack_token`.

If Discord isn't running on the machine, set `"discord": false`. Otherwise the app waits for Discord and doesn't update Slack in the meantime.

## Get API Key (Must Be Admin)

<img width="1595" height="1047" alt="api1" src="https://github.com/user-attachments/assets/c1239bf0-cccf-4fe8-b94f-2f19b568a385" />
<img width="1595" height="1047" alt="api2" src="https://github.com/user-attachments/assets/6f4c6588-db75-4108-b8e7-ebb949e37969" />

## Build
```
git clone https://github.com/0xgingi/audiobookshelf-discord-rpc
cd audiobookshelf-discord-rpc
cargo build --release
```
