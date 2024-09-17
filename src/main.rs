use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::time::Duration;
use tokio::time;
use discord_rich_presence::{activity, DiscordIpcClient, DiscordIpc};
use reqwest::Client;
use url::Url;
use std::env;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct Config {
    discord_client_id: String,
    audiobookshelf_url: String,
    audiobookshelf_token: String,
    audiobookshelf_user_id: String,
}

#[derive(Debug)]
struct Book {
    name: String,
    author: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    tag_name: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();

    if let Some(latest_version) = check_for_update(&client).await? {
        println!(
            "A new version is available: {}. You're currently running version {}.",
            latest_version, CURRENT_VERSION
        );
        println!("Please re-run the installer or visit https://github.com/0xGingi/audiobookshelf-discord-rpc/releases to download the latest version.");
    } else {
        println!("You're running the latest version: {}", CURRENT_VERSION);
    }

    let config_file = parse_args()?;
    println!("Using config file: {}", config_file);

    let config = load_config(&config_file)?;
    let mut discord = DiscordIpcClient::new(&config.discord_client_id)?;
    discord.connect()?;
    println!("Audiobookshelf Discord RPC Connected!");

    let mut last_known_time: Option<f64> = None;
    let mut is_paused = false;
    let mut current_book: Option<Book> = None;

    loop {
        if let Err(e) = set_activity(
            &client,
            &config,
            &mut discord,
            &mut last_known_time,
            &mut is_paused,
            &mut current_book,
        )
        .await
        {
            eprintln!("Error setting activity: {}", e);
        }
        time::sleep(Duration::from_secs(15)).await;
    }
}

fn parse_args() -> Result<String, Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "-c") {
        if index + 1 < args.len() {
            Ok(args[index + 1].clone())
        } else {
            Err("Error: missing argument for -c option".into())
        }
    } else {
        Ok("config.json".to_string())
    }
}

fn load_config(config_file: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let config_str = fs::read_to_string(config_file)?;
    let config: Config = serde_json::from_str(&config_str)?;
    Ok(config)
}

async fn set_activity(
    client: &Client,
    config: &Config,
    discord: &mut DiscordIpcClient,
    last_known_time: &mut Option<f64>,
    is_paused: &mut bool,
    current_book: &mut Option<Book>,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/api/me/listening-sessions?itemsPerPage=1", config.audiobookshelf_url);
    let resp = client
        .get(&url)
        .bearer_auth(&config.audiobookshelf_token)
        .send()
        .await?
        .json::<Value>()
        .await?;

    let sessions = resp["sessions"].as_array().ok_or("No sessions found")?;
    if sessions.is_empty() {
        println!("No active listening session");
        discord.clear_activity()?;
        return Ok(());
    }

    let session = &sessions[0];
    let book_name = session["displayTitle"].as_str().ok_or("Missing display title")?;
    let author = session["displayAuthor"].as_str().ok_or("Missing author")?;
    let current_time = session["currentTime"].as_f64().ok_or("Missing current time")?;
    let duration = session["duration"].as_f64().ok_or("Missing duration")?;
    let total_time = format_time(duration);

    if current_book.as_ref().map_or(true, |book| book.name != book_name) {
        *current_book = Some(Book {
            name: book_name.to_string(),
            author: author.to_string(),
        });
        *last_known_time = None;
        *is_paused = false;
    }

    if let Some(last_time) = last_known_time {
        if (current_time - *last_time).abs() < f64::EPSILON {
            if !*is_paused {
                println!("Book paused. Clearing activity.");
                discord.clear_activity()?;
                *is_paused = true;
            }
            return Ok(());
        }
    }

    *last_known_time = Some(current_time);
    *is_paused = false;

    let cover_url = get_cover_path(client, config, book_name, author).await?;
    let duration_str = format!("{} / {}", format_time(current_time), total_time);
    let activity = activity::Activity::new()
        .details(book_name)
        .state(author)
        .assets(
            activity::Assets::new()
                .large_image(&cover_url)
                .large_text(&duration_str),
        )
        .activity_type(activity::ActivityType::Listening);

    discord.set_activity(activity)?;
    Ok(())
}

fn format_time(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let remaining_seconds = total_seconds % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, remaining_seconds)
}

async fn get_cover_path(
    client: &Client,
    config: &Config,
    title: &str,
    author: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let url = Url::parse_with_params(
        &format!("{}/api/search/covers", config.audiobookshelf_url),
        &[("title", title), ("author", author), ("provider", "audible")],
    )?;

    let resp = client
        .get(url)
        .bearer_auth(&config.audiobookshelf_token)
        .send()
        .await?
        .json::<Value>()
        .await?;

    let results = resp["results"].as_array().ok_or("No cover results found")?;
    if let Some(cover_url) = results.get(0).and_then(Value::as_str) {
        Ok(cover_url.to_string())
    } else {
        Err("No valid cover URL found".into())
    }
}

async fn check_for_update(client: &Client) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let url = "https://api.github.com/repos/0xGingi/audiobookshelf-discord-rpc/releases/latest";
    let resp = client
        .get(url)
        .header("User-Agent", "Audiobookshelf-Discord-RPC")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API request failed with status: {}", resp.status()).into());
    }

    let release_info: ReleaseInfo = resp.json().await?;
    let latest_version = release_info.tag_name.trim_start_matches('v');

    if latest_version != CURRENT_VERSION {
        Ok(Some(latest_version.to_string()))
    } else {
        Ok(None)
    }
}