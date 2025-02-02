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
use log::{info, error};
use env_logger;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const TIME_OFFSET_CORRECTION: f64 = -16.0;

#[derive(Debug, Deserialize)]
struct Config {
    discord_client_id: String,
    audiobookshelf_url: String,
    audiobookshelf_token: String,
    show_chapters: Option<bool>,
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
    libraryItemId: String,
    chapters: Option<Vec<Chapter>>,
    libraryItem: Option<LibraryItem>,
}

#[derive(Debug, Deserialize)]
struct MediaMetadata {
    genres: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Chapter {
    title: String,
    start: f64,
    end: f64,
}

#[derive(Debug, Deserialize)]
struct LibraryItem {
    media: Option<Media>,
}

#[derive(Debug, Deserialize)]
struct Media {
    chapters: Option<Vec<Chapter>>,
}

#[derive(Debug, Deserialize)]
struct LibraryItemResponse {
    media: MediaResponse,
}

#[derive(Debug, Deserialize)]
struct MediaResponse {
    chapters: Vec<Chapter>,
}

#[derive(Debug)]
struct PlaybackState {
    last_api_time: SystemTime,
    last_position: f64,
    is_playing: bool,
}

#[derive(Debug)]
struct TimingInfo {
    last_api_time: Option<SystemTime>,
    last_position: Option<f64>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let client = Client::new();

    if let Some(latest_version) = check_for_update(&client).await? {
        info!(
            "A new version is available: {}. You're currently running version {}.",
            latest_version, CURRENT_VERSION
        );
        info!("Please re-run the installer or visit https://github.com/0xGingi/audiobookshelf-discord-rpc/releases to download the latest version.");
    } else {
        info!("You're running the latest version: {}", CURRENT_VERSION);
    }

    let config_file = parse_args()?;
    info!("Using config file: {}", config_file);

    let config = load_config(&config_file)?;
    let mut discord = DiscordIpcClient::new(&config.discord_client_id)?;
    discord.connect()?;
    info!("Audiobookshelf Discord RPC Connected!");

    let mut playback_state = PlaybackState {
        last_api_time: SystemTime::now(),
        last_position: 0.0,
        is_playing: false,
    };
    let mut current_book: Option<Book> = None;
    let mut timing_info = TimingInfo {
        last_api_time: None,
        last_position: None,
    };

    loop {
        if let Err(e) = set_activity(
            &client,
            &config,
            &mut discord,
            &mut playback_state,
            &mut current_book,
            &mut timing_info,
        )
        .await
        {
            error!("Error setting activity: {}", e);
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
    playback_state: &mut PlaybackState,
    current_book: &mut Option<Book>,
    timing_info: &mut TimingInfo,
) -> Result<(), Box<dyn std::error::Error>> {

    let sessions_url = format!(
        "{}/api/me/listening-sessions?itemsPerPage=1", 
        config.audiobookshelf_url
    );
    
    let resp = client
        .get(&sessions_url)
        .bearer_auth(&config.audiobookshelf_token)
        .send()
        .await?
        .json::<ListeningSessionsResponse>()
        .await?;

    if resp.sessions.is_empty() {
        info!("No active listening session");
        discord.clear_activity()?;
        return Ok(());
    }

    let session = &resp.sessions[0];
    
    let library_item_url = format!(
        "{}/api/items/{}?include=chapters", 
        config.audiobookshelf_url,
        session.libraryItemId
    );
    
    let library_item: LibraryItemResponse = client
        .get(&library_item_url)
        .bearer_auth(&config.audiobookshelf_token)
        .send()
        .await?
        .json()
        .await?;

    let current_time = session.currentTime;
    let genres = session.mediaMetadata.genres.get(0).map(|s| s.as_str()).unwrap_or("Unknown Genre");
    
    let large_text = if config.show_chapters.unwrap_or(false) {
        if let Some(current_chapter) = library_item.media.chapters.iter().find(|ch| {
            current_time >= ch.start && current_time <= ch.end
        }) {
            if current_chapter.title.to_lowercase().contains("chapter") {
                current_chapter.title.to_string()
            } else {
                format!("Chapter {}", current_chapter.title)
            }
        } else {
            genres.to_string()
        }
    } else {
        genres.to_string()
    };

    let book_name = &session.displayTitle;
    let author = &session.displayAuthor;
    let duration = session.duration;

    if current_book.as_ref().map_or(true, |book| book.name != *book_name) {
        *current_book = Some(Book {
            name: book_name.clone(),
        });
        *playback_state = PlaybackState {
            last_api_time: SystemTime::now(),
            last_position: 0.0,
            is_playing: false,
        };
    }

    let now = SystemTime::now();
    let current_position = if playback_state.is_playing {
        let elapsed = now
            .duration_since(playback_state.last_api_time)
            .unwrap_or(Duration::from_secs(0))
            .as_secs_f64();
        
        current_time + elapsed + TIME_OFFSET_CORRECTION
    } else {
        current_time
    };

    playback_state.last_api_time = now;
    playback_state.last_position = current_time;
    playback_state.is_playing = true;

    let now_secs = now.duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let current_pos = current_position.max(0.0) as i64;
    let total_dur = duration.max(0.0) as i64;

    let start_time = now_secs.saturating_sub(current_pos);
    let end_time = now_secs.saturating_add(total_dur.saturating_sub(current_pos));

    let mut activity_builder = activity::Activity::new()
        .details(book_name)
        .state(author)
        .timestamps(
            activity::Timestamps::new()
                .start(start_time)
                .end(end_time)
        )
        .activity_type(activity::ActivityType::Listening);

    let cover_url = get_cover_path(client, config, book_name, author).await?;

    if let Some(ref url) = cover_url {
        activity_builder = activity_builder.assets(
            activity::Assets::new()
                .large_image(url)
                .large_text(&large_text)
        );
    }

    discord.set_activity(activity_builder)?;

    if let (Some(last_time), Some(last_api_time)) = (timing_info.last_position, timing_info.last_api_time) {
        if (current_time - last_time).abs() > f64::EPSILON {
            let elapsed = SystemTime::now()
                .duration_since(last_api_time)
                .unwrap_or(Duration::from_secs(0));
            info!(
                "API position updated: previous={:.2}s, current={:.2}s, time since last update={:.2}s",
                last_time,
                current_time,
                elapsed.as_secs_f64()
            );
        }
    }
    
    timing_info.last_position = Some(current_time);
    timing_info.last_api_time = Some(SystemTime::now());

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