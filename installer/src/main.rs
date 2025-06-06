use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use reqwest::Client;
use serde_json::json;
use std::process::Command;
use serde_json::Value;

const CURRENT_INSTALLER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Audiobookshelf Discord RPC Installer/Updater");

    let current_installer_version = format!("installer-v{}", CURRENT_INSTALLER_VERSION);
    let client = Client::new();
    let latest_installer_version = get_latest_installer_version(&client).await?;

    if latest_installer_version != current_installer_version {
        println!("A new version of the installer is available: {}", latest_installer_version);
        println!("Please download the latest version from https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/tag/{}", latest_installer_version);
    }

    let action = prompt_with_default("Do you want to (i)nstall or (u)pdate?", "install")?
        .to_lowercase();

    let client = Client::new();
    let latest_version = get_latest_version(&client).await?;
    println!("Latest version: {}", latest_version);

    let (download_url, install_path) = if cfg!(target_os = "windows") {
        (
            format!("https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/{}/audiobookshelf-discord-rpc.exe", latest_version),
            PathBuf::from(std::env::var("LOCALAPPDATA")?).join("AudiobookshelfDiscordRPC").join("audiobookshelf-discord-rpc.exe")
        )
    } else if cfg!(target_os = "macos") {
        (
            format!("https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/{}/audiobookshelf-discord-rpc-macos-arm64", latest_version),
            PathBuf::from(std::env::var("HOME")?).join(".local").join("bin").join("audiobookshelf-discord-rpc")
        )
    } else {
        (
            format!("https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/{}/audiobookshelf-discord-rpc-linux-x64", latest_version),
            PathBuf::from(std::env::var("HOME")?).join(".local").join("bin").join("audiobookshelf-discord-rpc")
        )
    };

    if action.starts_with('u') {
        println!("Stopping existing service...");
        stop_service()?;

        println!("Updating binary...");
        update_binary(&client, &download_url, &install_path).await?;

        println!("Starting service...");
        start_service()?;

        println!("Update complete!");
    } else {
        if let Some(parent) = install_path.parent() {
            fs::create_dir_all(parent)?;
        }

        println!("Downloading binary...");
        let content = download_file(&client, &download_url).await?;
        fs::write(&install_path, content)?;

        #[cfg(target_family = "unix")]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&install_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&install_path, perms)?;
        }

        println!("Binary installed to: {}", install_path.display());

        let should_generate_config = prompt_with_default("Do you want to generate a config.json file?", "yes")?
            .to_lowercase();
        
        if should_generate_config.starts_with('y') {
            println!("Generating config.json...");
            let config = generate_config()?;
        
            let config_path = install_path.with_file_name("config.json");
            fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
            println!("Config file created at: {}", config_path.display());
        } else {
            println!("Skipping config.json generation.");
            println!("Note: You'll need to create a config.json file manually before running the service.");
        }

        let should_install_service = prompt_with_default("Do you want to install an autostart service?", "yes")?
            .to_lowercase();
        
        if should_install_service.starts_with('y') {
            #[cfg(target_os = "windows")]
            create_windows_service(&install_path)?;
        
            #[cfg(target_os = "macos")]
            create_macos_service(&install_path)?;
        
            #[cfg(all(target_family = "unix", not(target_os = "macos")))]
            create_linux_service(&install_path)?;
        } else {
            println!("Skipping service installation.");
            println!("Note: You'll need to start the service manually or create a service");
        }
        
        println!("Installation complete!");
    }

    println!("Operation complete!");
    wait_for_key_press();
    Ok(())
}

async fn get_latest_version(client: &Client) -> Result<String, Box<dyn std::error::Error>> {
    let url = "https://api.github.com/repos/0xGingi/audiobookshelf-discord-rpc/releases/latest";
    let resp = client.get(url)
        .header("User-Agent", "Audiobookshelf-Discord-RPC-Installer")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API request failed with status: {}", resp.status()).into());
    }

    let json: serde_json::Value = resp.json().await?;

    json.get("tag_name")
        .and_then(|tag| tag.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No tag_name found in the response".into())
}

fn generate_config() -> Result<serde_json::Value, io::Error> {
    println!("Please enter the following information:");

    let audiobookshelf_url = prompt("Audiobookshelf URL (Don't forget to include port if not reverse proxying)")?;
    let audiobookshelf_token = prompt("Audiobookshelf API Key (Find this when clicking on your user in settings)")?;
    let default_discord_client_id = "1283070638088650752";
    let discord_client_id = prompt_with_default("Discord Client ID", default_discord_client_id)?;
    let show_chapters = prompt_with_default("Show chapters instead of genres? (yes/no)", "no")?
        .to_lowercase()
        .starts_with('y');
    
    Ok(json!({
        "discord_client_id": discord_client_id,
        "audiobookshelf_url": audiobookshelf_url,
        "audiobookshelf_token": audiobookshelf_token,
        "show_chapters": show_chapters,
    }))
}

fn prompt_with_default(prompt: &str, default: &str) -> Result<String, io::Error> {
    print!("{} [{}]: ", prompt, default);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input.to_string())
    }
}

fn prompt(prompt: &str) -> Result<String, io::Error> {
    print!("{}: ", prompt);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

#[cfg(target_os = "windows")]
fn create_windows_service(install_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("Creating Windows Task Scheduler task...");

    let task_name = "AudiobookshelfDiscordRPC";
    let task_program = install_path.display().to_string();
    
    let config_path_buf = install_path.with_file_name("config.json");
    let config_path_display = config_path_buf.display();
    
    let task_arguments = format!("-c \"{}\"", config_path_display);

    let powershell_command = format!(
        "$action = New-ScheduledTaskAction -Execute '{}' -Argument '{}'; \
         $trigger = New-ScheduledTaskTrigger -AtLogon; \
         $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest; \
         Register-ScheduledTask -TaskName '{}' -Action $action -Trigger $trigger -Principal $principal -Force; \
         Start-ScheduledTask -TaskName '{}'",
        task_program,
        task_arguments,
        task_name,
        task_name
    );

    let output = Command::new("powershell")
        .args(&["-Command", &powershell_command])
        .output()?;

    if output.status.success() {
        println!("Windows Task Scheduler task created and started successfully.");
    } else {
        println!("Failed to create or start Windows Task Scheduler task.");
        println!("PowerShell error: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn create_macos_service(install_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("Creating macOS LaunchAgent...");

    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.audiobookshelf.discord-rpc</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>-c</string>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/audiobookshelf-discord-rpc.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/audiobookshelf-discord-rpc.log</string>
</dict>
</plist>
"#,
        install_path.display(),
        install_path.with_file_name("config.json").display(),
    );

    let launch_agents_dir = PathBuf::from(std::env::var("HOME")?).join("Library").join("LaunchAgents");
    let plist_path = launch_agents_dir.join("com.audiobookshelf.discord-rpc.plist");
    fs::create_dir_all(&launch_agents_dir)?;
    fs::write(&plist_path, plist_content)?;

    Command::new("launchctl")
        .args(&["load", "-w", &plist_path.display().to_string()])
        .status()?;

    println!("macOS LaunchAgent created and loaded successfully.");
    Ok(())
}

#[cfg(all(target_family = "unix", not(target_os = "macos")))]
fn create_linux_service(install_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("Creating Linux systemd service...");

    let service_content = format!(
        r#"[Unit]
Description=Audiobookshelf Discord RPC
After=network.target
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={} -c {}
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
"#,
        install_path.display(),
        install_path.with_file_name("config.json").display(),
    );

    let config_dir = PathBuf::from(std::env::var("HOME")?).join(".config").join("systemd").join("user");
    let service_path = config_dir.join("audiobookshelf-discord-rpc.service");
    fs::create_dir_all(&config_dir)?;
    fs::write(&service_path, service_content)?;

    Command::new("systemctl")
        .args(&["--user", "daemon-reload"])
        .status()?;
    Command::new("systemctl")
        .args(&["--user", "enable", "--now", "audiobookshelf-discord-rpc"])
        .status()?;

    println!("Linux systemd service created and started successfully.");
    Ok(())
}

fn wait_for_key_press() {
    println!("Press Enter to exit...");
    let mut _input = String::new();
    let _ = io::stdin().read_line(&mut _input);
}

fn stop_service() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        Command::new("schtasks")
            .args(&["/End", "/TN", "AudiobookshelfDiscordRPC"])
            .status()?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("launchctl")
            .args(&["unload", &format!("{}/Library/LaunchAgents/com.audiobookshelf.discord-rpc.plist", std::env::var("HOME")?)])
            .status()?;
    }

    #[cfg(all(target_family = "unix", not(target_os = "macos")))]
    {
        Command::new("systemctl")
            .args(&["--user", "stop", "audiobookshelf-discord-rpc"])
            .status()?;
    }

    Ok(())
}

fn start_service() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        Command::new("schtasks")
            .args(&["/Run", "/TN", "AudiobookshelfDiscordRPC"])
            .status()?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("launchctl")
            .args(&["load", "-w", &format!("{}/Library/LaunchAgents/com.audiobookshelf.discord-rpc.plist", std::env::var("HOME")?)])
            .status()?;
    }

    #[cfg(all(target_family = "unix", not(target_os = "macos")))]
    {
        Command::new("systemctl")
            .args(&["--user", "start", "audiobookshelf-discord-rpc"])
            .status()?;
    }

    Ok(())
}

async fn update_binary(client: &Client, download_url: &str, install_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let content = download_file(client, download_url).await?;
    fs::write(install_path, content)?;

    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&install_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(install_path, perms)?;
    }

    Ok(())
}

async fn download_file(client: &Client, url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(format!("Failed to download file: HTTP {}", response.status()).into());
    }
    let bytes = response.bytes().await?;
    Ok(bytes.to_vec())
}

async fn get_latest_installer_version(client: &Client) -> Result<String, Box<dyn std::error::Error>> {
    let url = "https://api.github.com/repos/0xGingi/audiobookshelf-discord-rpc/releases";
    let resp = client.get(url)
        .header("User-Agent", "Audiobookshelf-Discord-RPC-Installer")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API request failed with status: {}", resp.status()).into());
    }

    let releases: Vec<Value> = resp.json().await?;
    let latest_installer_release = releases.iter()
        .filter_map(|release| release.get("tag_name").and_then(Value::as_str))
        .filter(|tag| tag.starts_with("installer-v"))
        .max()
        .ok_or_else(|| Box::<dyn std::error::Error>::from("No installer release found"))?;

    Ok(latest_installer_release.to_string())
}
