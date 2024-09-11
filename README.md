# audiobookshelf-discord-rpc
Displays what you're listening to on audiobookshelf on discord!

* Now Rewritten in Rust! [You can find the NodeJS Branch here](https://github.com/0xGingi/audiobookshelf-discord-rpc/tree/Javascript)

* Note: This will display what you're listening to on any device but you must run this program on a computer with discord installed!

## Run

### Windows
* [Download the installer](https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/v1.0.0/audiobookshelf-discord-rpc-installer.exe)
* **Run the installer as admin to have the autostart service** 
* Generate your config file
* Create the startup task
* audiobookshelf-discord-rpc should now be started and will run on boot
### Linux
* [Download the installer](https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/v1.0.0/audiobookshelf-discord-rpc-installer-linux)
```
./audiobookshelf-discord-rpc-installer-linux
```
* Generate your config file
* Create the systemd service (runs as user)
* audiobookshelf-discord-rpc should now be started and will run on boot

## Get API Key (Must Be Admin)
![abs-api-1](https://github.com/user-attachments/assets/57a0c95d-acfc-447e-aa6a-fc8651ddca24)
![abs-api-2](https://github.com/user-attachments/assets/b712957b-3402-469c-a85c-8f283ccc8c08)
![abs-api-3](https://github.com/user-attachments/assets/edf71490-a695-443e-b25f-98923107f70b)



## Build
```
git clone https://github.com/0xgingi/audiobookshelf-discord-rpc
cd audiobookshelf-discord-rpc
cargo build --release
```
### Build Installer
```
git clone https://github.com/0xgingi/audiobookshelf-discord-rpc
cd audiobookshelf-discord-rpc/installer
cargo build --release
```

