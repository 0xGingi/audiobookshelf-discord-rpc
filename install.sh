#!/bin/sh
# Installs (or updates) Audiobookshelf Discord RPC and starts it on login. macOS & Linux.
#   curl -fsSL https://raw.githubusercontent.com/0xGingi/audiobookshelf-discord-rpc/main/install.sh | sh
# Re-run to update; your config is kept. To uninstall:
#   curl -fsSL https://raw.githubusercontent.com/0xGingi/audiobookshelf-discord-rpc/main/install.sh | sh -s -- --uninstall
set -eu

repo=0xGingi/audiobookshelf-discord-rpc
name=audiobookshelf-discord-rpc
bin="$HOME/.local/bin/$name"
dir="${XDG_CONFIG_HOME:-$HOME/.config}/$name"
cfg="$dir/config.json"
os="$(uname -s)"

case "$os" in
Darwin)
	label=com.audiobookshelf.discord-rpc
	service="$HOME/Library/LaunchAgents/$label.plist"
	log="$HOME/Library/Logs/$name.log"
	stop() { launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true; }
	;;
Linux)
	service="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$name.service"
	log="journalctl --user -u $name -f"
	stop() { systemctl --user stop "$name" 2>/dev/null || true; }
	;;
*)
	echo "Unsupported OS: $os" >&2
	exit 1
	;;
esac

if [ "${1:-}" = --uninstall ]; then
	stop
	if [ "$os" = Linux ]; then systemctl --user disable "$name" 2>/dev/null || true; fi
	rm -f "$bin" "$service"
	echo "Uninstalled. Your config is still in $dir (delete it if you like)."
	exit 0
fi

case "$os-$(uname -m)" in
Darwin-arm64) asset=$name-macos-arm64 ;;
Linux-x86_64) asset=$name-linux-x64 ;;
*)
	echo "No prebuilt binary for $os $(uname -m). Build from source: https://github.com/$repo#build" >&2
	exit 1
	;;
esac

echo "Downloading $asset..."
mkdir -p "$(dirname "$bin")" "$dir"
curl -fL --progress-bar "https://github.com/$repo/releases/latest/download/$asset" -o "$bin.tmp"
chmod +x "$bin.tmp"
stop
mv -f "$bin.tmp" "$bin"

# Older installers kept the config next to the binary.
old="$HOME/.local/bin"
if [ ! -f "$cfg" ] && grep -qs audiobookshelf_url "$old/config.json"; then
	mv "$old/config.json" "$cfg"
	if [ -f "$old/urls.json" ]; then mv "$old/urls.json" "$dir/"; fi
	echo "Moved your existing config to $cfg"
fi

if [ ! -f "$cfg" ]; then
	"$bin" --setup -c "$cfg" </dev/tty
fi

mkdir -p "$(dirname "$service")"
if [ "$os" = Darwin ]; then
	mkdir -p "$(dirname "$log")"
	cat >"$service" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$label</string>
    <key>ProgramArguments</key>
    <array>
        <string>$bin</string>
        <string>-c</string>
        <string>$cfg</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$log</string>
    <key>StandardErrorPath</key>
    <string>$log</string>
</dict>
</plist>
EOF
	launchctl bootstrap "gui/$(id -u)" "$service"
elif command -v systemctl >/dev/null 2>&1; then
	cat >"$service" <<EOF
[Unit]
Description=Audiobookshelf Discord RPC

[Service]
ExecStart="$bin" -c "$cfg"
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
EOF
	systemctl --user daemon-reload
	systemctl --user enable --now "$name"
else
	echo "Installed to $bin, but there's no systemd to autostart it. Run it with:"
	echo "  \"$bin\" -c \"$cfg\""
	exit 0
fi

echo
echo "Done! It's running now and will start when you log in."
echo "  Config: $cfg   (change it: \"$bin\" --setup -c \"$cfg\")"
echo "  Logs:   $log"
