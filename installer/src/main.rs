use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use reqwest;
use serde_json::json;
use std::process::Command;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Audiobookshelf Discord RPC Installer");

    let latest_version = get_latest_version().await?;
    println!("Latest version: {}", latest_version);

    let (download_url, install_path) = if cfg!(target_os = "windows") {
        (
            format!("https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/{}/audiobookshelf-discord-rpc.exe", latest_version),
            PathBuf::from(std::env::var("LOCALAPPDATA")?).join("AudiobookshelfDiscordRPC").join("audiobookshelf-discord-rpc.exe")
        )
    } else {
        (
            format!("https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/download/{}/audiobookshelf-discord-rpc-linux-x64", latest_version),
            PathBuf::from(std::env::var("HOME")?).join(".local").join("bin").join("audiobookshelf-discord-rpc")

        )
    };

    if let Some(parent) = install_path.parent() {
        fs::create_dir_all(parent)?;
    }

    println!("Downloading binary...");
    let response = reqwest::get(&download_url).await?;
    let content = response.bytes().await?;
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
    
    if should_generate_config == "yes" || should_generate_config == "y" {
        println!("Generating config.json...");
        let config = generate_config()?;
    
        let config_path = install_path.with_file_name("config.json");
        fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
        println!("Config file created at: {}", config_path.display());
    
        #[cfg(target_os = "windows")]
        create_windows_service(&install_path)?;
        
        #[cfg(not(target_os = "windows"))]
        create_linux_service(&install_path)?;
    } else {
        println!("Skipping config.json generation.");
        println!("Note: You'll need to create a config.json file manually before running the service.");
    }
    
    println!("Installation and configuration complete!");
    Ok(())
}

async fn get_latest_version() -> Result<String, Box<dyn std::error::Error>> {
    let url = "https://api.github.com/repos/0xGingi/audiobookshelf-discord-rpc/releases/latest";
    let client = reqwest::Client::new();
    let resp = client.get(url)
        .header("User-Agent", "Audiobookshelf-Discord-RPC-Installer")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API request failed with status: {}", resp.status()).into());
    }

    let body = resp.text().await?;

    let json: serde_json::Value = serde_json::from_str(&body)?;

    match json.get("tag_name") {
        Some(tag) => Ok(tag.as_str().unwrap_or_default().to_string()),
        None => Err("No tag_name found in the response".into()),
    }
}

fn generate_config() -> Result<serde_json::Value, io::Error> {
    println!("Please enter the following information:");

    let audiobookshelf_url = prompt("Audiobookshelf URL (Don't forget to include port if not reverse proxying)")?;
    let audiobookshelf_token = prompt("Audiobookshelf API Key (Find this when clicking on your user in settings)")?;
    let audiobookshelf_user_id = prompt("Audiobookshelf User Name")?;
    let default_discord_client_id = "1283070638088650752";
    let discord_client_id = prompt_with_default("Discord Client ID", default_discord_client_id)?;


    Ok(json!({
        "audiobookshelfUrl": audiobookshelf_url,
        "audiobookshelfToken": audiobookshelf_token,
        "audiobookshelfUserId": audiobookshelf_user_id,
        "discordClientId": discord_client_id
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
    println!("Creating Windows service...");
    
    let bat_path = install_path.with_file_name("run_audiobookshelf_discord_rpc.bat");
    let bat_content = format!("@echo off\n\"{}\" -c \"{}\"", 
        install_path.display(), 
        install_path.with_file_name("config.json").display());
    fs::write(&bat_path, bat_content)?;

    let output = Command::new("sc")
        .args(&[
            "create", "AudiobookshelfDiscordRPC",
            "binPath=", &bat_path.to_string_lossy(),
            "start=", "auto",
            "displayname=", "Audiobookshelf Discord RPC"
        ])
        .output()?;

    if !output.status.success() {
        return Err(format!("Failed to create Windows service: {:?}", output).into());
    }

    println!("Windows service created successfully.");
    Ok(())
}

#[cfg(target_family = "unix")]
fn create_linux_service(install_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("Creating Linux systemd service...");

    let service_content = format!(
        r#"[Unit]
Description=Audiobookshelf Discord RPC
After=network.target

[Service]
ExecStart={} -c {}
Restart=always

[Install]
WantedBy=default.target
"#,
        install_path.display(),
        install_path.with_file_name("config.json").display(),
    );

    let service_path = PathBuf::from(std::env::var("HOME")?).join(".config").join("systemd").join("user").join("audiobookshelf-discord-rpc.service");
    fs::create_dir_all(service_path.parent().unwrap())?;
    fs::write(&service_path, service_content)?;

    Command::new("systemctl").args(&["--user", "daemon-reload"]).status()?;
    Command::new("systemctl").args(&["--user", "enable", "--now", "audiobookshelf-discord-rpc"]).status()?;

    println!("Linux systemd service created and started successfully.");
    Ok(())
}
