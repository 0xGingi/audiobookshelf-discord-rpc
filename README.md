# audiobookshelf-discord-rpc
Displays what you're listening to on audiobookshelf on discord!

Shows Book Name, Author, Duration, Genres or Chapter, and Cover Art

* Note: This will display what you're listening to on any device but you must run this program on a computer with discord installed!

* The Installer now has an update feature, just press u and let it do its thing!

![image](https://github.com/user-attachments/assets/2354b157-3b54-4b4b-8ab3-fa7d7f64fa56)


## Run

### Windows
* [Download the installer](https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/installer-v1.4.2/audiobookshelf-discord-rpc-installer.exe)
* **Run the installer as admin to have the autostart service** 
* Generate your config file
* Create the startup task
* audiobookshelf-discord-rpc should now be started and will run on boot
* Files are located at %localappdata%/AudiobookshelfDiscordRPC & Service is created with Task Scheduler

### Linux
* [Download the installer](https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/installer-v1.4.2/audiobookshelf-discord-rpc-installer)
```
./audiobookshelf-discord-rpc-installer
```
* Generate your config file
* Create the systemd service (runs as user)
* audiobookshelf-discord-rpc should now be started and will run on boot
* Executable and config.json are located at ~/.local/bin/ & systemd service is located at ~/.config/systemd/user/audiobookshelf-discord-rpc.service

### MacOS (Arm64 Only)
* [Download the installer](https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/installer-v1.4.2/audiobookshelf-discord-rpc-installer-macos-arm64)
```
./audiobookshelf-discord-rpc-installer
```
* Generate your config file
* Create the service
* audiobookshelf-discord-rpc should now be started and will run on boot
* Executable and config.json are located at ~/.local/bin/


### Docker (Linux Only - Requires Discord Installed on system)
* Clone the repo
```
git clone https://github.com/0xgingi/audiobookshelf-discord-rpc
cd audiobookshelf-discord-rpc
```
* Create a config.json file
```
cp config/config.json.example config/config.json
```
* Edit the config.json file
* Run the docker container
```
docker compose up -d
```

## Get API Key (Must Be Admin)

<img width="1595" height="1047" alt="api1" src="https://github.com/user-attachments/assets/c1239bf0-cccf-4fe8-b94f-2f19b568a385" />
<img width="1595" height="1047" alt="api2" src="https://github.com/user-attachments/assets/6f4c6588-db75-4108-b8e7-ebb949e37969" />


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

