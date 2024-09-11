# audiobookshelf-discord-rpc
Displays what you're listening to on audiobookshelf on discord!

* Now Rewritten in Rust! [You can find the NodeJS Branch here](https://github.com/0xGingi/audiobookshelf-discord-rpc/tree/Javascript)

* Note: This will display what you're listening to on any device but you must run this program on a computer with discord installed!

## Run

* Rename config.json.example to config.json and modify it! You can get your API Key when clicking on your username in settings.

* Place your config.json either in the same folder as your executable, or add the arguement -c /path/to/config.json

* Download an executable for your OS from the [releases](https://github.com/0xgingi/audiobookshelf-discord-rpc/releases) page!

## Build
```
git clone https://github.com/0xgingi/audiobookshelf-discord-rpc
cd audiobookshelf-discord-rpc
cargo build --release
```

