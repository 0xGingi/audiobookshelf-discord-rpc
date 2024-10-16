# audiobookshelf-discord-rpc
Displays what you're listening to on audiobookshelf on discord!

Shows Book Name, Author, Duration, Genres, and Cover Art

* Now Rewritten in Rust! [You can find the NodeJS Branch here](https://github.com/0xGingi/audiobookshelf-discord-rpc/tree/Javascript)

* Note: This will display what you're listening to on any device but you must run this program on a computer with discord installed!

* The Installer now has an update feature, just press u and let it do its thing!

![image](https://github.com/user-attachments/assets/2354b157-3b54-4b4b-8ab3-fa7d7f64fa56)


## Run

### Windows
* [Download the installer](https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/installer-v1.3.0/audiobookshelf-discord-rpc-installer.exe)
* **Run the installer as admin to have the autostart service** 
* Generate your config file
* Create the startup task
* audiobookshelf-discord-rpc should now be started and will run on boot
* Files are located at %localappdata%/AudiobookshelfDiscordRPC & Service is created with Task Scheduler
### Linux
* [Download the installer](https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/installer-v1.3.0/audiobookshelf-discord-rpc-installer)
```
./audiobookshelf-discord-rpc-installer-linux
```
* Generate your config file
* Create the systemd service (runs as user)
* audiobookshelf-discord-rpc should now be started and will run on boot
* Executable and config.json are located at ~/.local/bin/ & systemd service is located at ~/.config/systemd/user/audiobookshelf-discord-rpc.service

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

