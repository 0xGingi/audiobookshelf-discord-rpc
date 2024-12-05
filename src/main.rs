use discord_rich_presence::{activity, DiscordIpcClient, DiscordIpc};
use futures::future::join_all;
use serde::Deserialize;
use std::fs;
use std::time::Duration;
use tokio::time;
use reqwest::Client;
use url::Url;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct Config {
    discord_client_id: String,
    audiobookshelf_url: String,
    audiobookshelf_token: String,
}

#[derive(Debug)]
struct Book {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    tag_name: String,
}

#[derive(Debug, Deserialize)]
struct ListeningSessionsResponse {
    sessions: Vec<Session>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct Session {
    displayTitle: String,
    displayAuthor: String,
    currentTime: f64,
    duration: f64,
    mediaMetadata: MediaMetadata,
}

#[derive(Debug, Deserialize)]
struct MediaMetadata {
    genres: Vec<String>,
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

#[allow(non_snake_case)]
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
        .json::<ListeningSessionsResponse>()
        .await?;

    if resp.sessions.is_empty() {
        println!("No active listening session");
        discord.clear_activity()?;
        return Ok(());
    }

    let session = &resp.sessions[0];
    let book_name = &session.displayTitle;
    let author = &session.displayAuthor;
    let current_time = session.currentTime;
    let duration = session.duration;

    let genres = session.mediaMetadata.genres.get(0).map(|s| s.as_str()).unwrap_or("Unknown Genre");

    if current_book.as_ref().map_or(true, |book| book.name != *book_name) {
        *current_book = Some(Book {
            name: book_name.clone(),
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

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs() as i64;

        let current_position = current_time.max(0.0) as i64;
        let total_duration = duration.max(0.0) as i64;
    
        let start_time = now.saturating_sub(current_position);
        let end_time = now.saturating_add(total_duration.saturating_sub(current_position));
            

    let mut activity_builder = activity::Activity::new()
        .details(book_name)
        .state(author)
        .timestamps(
            activity::Timestamps::new()
                .start(start_time)
                .end(end_time)
        )
        .activity_type(activity::ActivityType::Listening);

        println!("Setting activity with timestamps: start={}, end={}", start_time, end_time);


    if let Some(ref url) = cover_url {
        activity_builder = activity_builder.assets(
            activity::Assets::new()
                .large_image(url)
                .large_text(genres)
        );
    }

    discord.set_activity(activity_builder)?;
    Ok(())
}

async fn get_cover_path(
    client: &Client,
    config: &Config,
    title: &str,
    author: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let providers = vec![
        "audible",
        "google",
        "audible.jp",
        "openlibrary",
        "itunes",
        "audible.ca",
        "audible.uk",
        "audible.au",
        "audible.fr",
        "audible.de",
        "audible.it",
        "audible.in",
        "audible.es",
        "fantlab",
    ];

    let futures = providers.iter().map(|provider| {
        let client = client.clone();
        let config = config;
        let title = title.to_string();
        let author = author.to_string();
        async move {
            let url = Url::parse_with_params(
                &format!("{}/api/search/covers", config.audiobookshelf_url),
                &[("title", title.as_str()), ("author", author.as_str()), ("provider", *provider)],
            )?;
            let resp: CoverResponse = client
                .get(url)
                .bearer_auth(&config.audiobookshelf_token)
                .send()
                .await?
                .json()
                .await?;
            if let Some(cover_url) = resp.results.get(0) {
                return Ok(Some(cover_url.clone()));
            }
            Ok(None)
        }
    });

    let results: Vec<Result<Option<String>, Box<dyn std::error::Error>>> = join_all(futures).await;
    for result in results {
        if let Ok(Some(url)) = result {
            return Ok(Some(url));
        }
    }

    Ok(None)
}

#[derive(Debug, Deserialize)]
struct CoverResponse {
    results: Vec<String>,
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