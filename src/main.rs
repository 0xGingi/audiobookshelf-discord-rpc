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
use log::{info, error, warn};
use env_logger;
use std::io::ErrorKind;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct Config {
    discord_client_id: String,
    audiobookshelf_url: String,
    audiobookshelf_token: String,
    show_chapters: Option<bool>,
    use_abs_cover: Option<bool>,
    imgur_client_id: Option<String>,
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
    libraryItemId: String
}

#[derive(Debug, Deserialize)]
struct MediaMetadata {
    #[serde(default)]
    genres: Vec<String>,
    #[serde(rename = "podcastTitle")]
    podcast_title: Option<String>,
    title: Option<String>,
    season: Option<String>,
    episode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chapter {
    title: String,
    start: f64,
    end: f64,
}

#[derive(Debug, Deserialize)]
struct LibraryItemResponse {
    media: MediaResponse,
    #[serde(rename = "mediaType")]
    media_type: Option<String>,
    #[serde(rename = "mediaMetadata")]
    media_metadata: Option<LibraryItemMetadata>,
}

#[derive(Debug, Deserialize)]
struct LibraryItemMetadata {
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MediaResponse {
    #[serde(default)]
    chapters: Vec<Chapter>,
}

#[derive(Debug)]
struct PlaybackState {
    last_api_time: SystemTime,
    is_playing: bool,
}

#[derive(Debug)]
struct TimingInfo {
    last_api_time: Option<SystemTime>,
    last_position: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CoverResponse {
    results: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ImgurResponse {
    data: ImgurData,
    success: bool,
}

#[derive(Debug, Deserialize)]
struct ImgurData {
    link: String,
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
    info!("Using Audiobookshelf authentication (API Key recommended for v2.26.0+)");
    let mut discord = DiscordIpcClient::new(&config.discord_client_id);
    discord.connect()?;
    info!("Audiobookshelf Discord RPC Connected!");

    let mut playback_state = PlaybackState {
        last_api_time: SystemTime::now(),
        is_playing: false,
    };
    let mut current_book: Option<Book> = None;
    let mut timing_info = TimingInfo {
        last_api_time: None,
        last_position: None,
    };
    let cache_file = cache_file_path(&config_file);
    let mut imgur_cache: HashMap<String, String> = load_imgur_cache_with_fallback(&cache_file);

    loop {
        if let Err(e) = set_activity(
            &client,
            &config,
            &mut discord,
            &mut playback_state,
            &mut current_book,
            &mut timing_info,
            &mut imgur_cache,
            &cache_file,
        )
        .await
        {
            let mut is_pipe_error = false;
            if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                if io_err.kind() == ErrorKind::BrokenPipe || io_err.raw_os_error() == Some(232) || io_err.raw_os_error() == Some(32) {
                    is_pipe_error = true;
                }
            }

            if !is_pipe_error {
                let mut source = e.source();
                while let Some(err) = source {
                    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                        if io_err.kind() == ErrorKind::BrokenPipe || io_err.raw_os_error() == Some(232) || io_err.raw_os_error() == Some(32) {
                            is_pipe_error = true;
                            break;
                        }
                    }
                    source = err.source();
                }
            }

            if is_pipe_error {
                warn!("Connection to Discord lost (pipe closed). Attempting to reconnect...");
                if let Err(close_err) = discord.close() {
                    error!("Error closing old Discord client (connection likely already broken): {}", close_err);
                }
                time::sleep(Duration::from_secs(5)).await;
                let mut new_discord = DiscordIpcClient::new(&config.discord_client_id);
                if let Err(connect_err) = new_discord.connect() {
                    error!("Failed to reconnect to Discord: {}", connect_err);
                } else {
                    info!("Successfully reconnected to Discord.");
                    discord = new_discord;
                }
            } else {
                error!("Error setting activity (not identified as pipe error): {}", e);
                error!("Full error details: {:?}", e);
            }
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
    imgur_cache: &mut HashMap<String, String>,
    cache_file: &Path,
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
    
    if timing_info.last_position.is_none() {
        playback_state.is_playing = false;
        discord.clear_activity()?;
        timing_info.last_position = Some(session.currentTime);
        timing_info.last_api_time = Some(SystemTime::now());
        return Ok(());
    }

    let current_time = session.currentTime;
    
    if let (Some(last_time), Some(last_api_time)) = (timing_info.last_position, timing_info.last_api_time) {
        let elapsed = SystemTime::now().duration_since(last_api_time).unwrap_or(Duration::from_secs(0));
        if elapsed.as_secs() >= 2 && (current_time - last_time).abs() < f64::EPSILON {
            playback_state.is_playing = false;
            discord.clear_activity()?;
            timing_info.last_position = Some(current_time);
            timing_info.last_api_time = Some(SystemTime::now());
            return Ok(());
        } else if (current_time - last_time).abs() > f64::EPSILON {
            playback_state.is_playing = true;
        }
    }

    if !playback_state.is_playing {
        discord.clear_activity()?;
        timing_info.last_position = Some(current_time);
        timing_info.last_api_time = Some(SystemTime::now());
        return Ok(());
    }

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

    let is_podcast = library_item.media_type.as_deref() == Some("podcast") || 
                     session.mediaMetadata.podcast_title.is_some();
    
    let genres = session.mediaMetadata.genres.get(0).map(|s| s.as_str()).unwrap_or("Unknown Genre");
    
    let now = SystemTime::now();

    let large_text = if is_podcast {
        if let Some(podcast_title) = &session.mediaMetadata.podcast_title {
            if let (Some(season), Some(episode)) = (&session.mediaMetadata.season, &session.mediaMetadata.episode) {
                if !season.is_empty() && !episode.is_empty() {
                    format!("{} - S{}E{}", podcast_title, season, episode)
                } else if !episode.is_empty() {
                    format!("{} - Episode {}", podcast_title, episode)
                } else {
                    podcast_title.clone()
                }
            } else {
                podcast_title.clone()
            }
        } else {
            genres.to_string()
        }
    } else if config.show_chapters.unwrap_or(false) {
        if let Some(current_chapter) = library_item.media.chapters.iter().find(|ch| {
            current_time >= ch.start && current_time <= ch.end
        }) {
            if has_chapter_prefix(&current_chapter.title) {
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

    let (book_name, author) = if is_podcast {
        let podcast_title = session.mediaMetadata.title.as_ref()
            .or_else(|| session.mediaMetadata.podcast_title.as_ref())
            .or_else(|| {
                library_item.media_metadata.as_ref()
                    .and_then(|meta| meta.title.as_ref())
            });
        
        info!("Podcast detected - title from session: {:?}, podcast_title: {:?}", 
              session.mediaMetadata.title,
              session.mediaMetadata.podcast_title);
        
        if let Some(title) = podcast_title {
            info!("Using podcast title: '{}', episode: '{}'", title, session.displayTitle);
            (title.clone(), session.displayTitle.clone())
        } else {
            info!("No podcast title found, using displayTitle: '{}'", session.displayTitle);
            (session.displayTitle.clone(), session.displayAuthor.clone())
        }
    } else {
        (session.displayTitle.clone(), session.displayAuthor.clone())
    };
    let duration = session.duration;

    if current_book.as_ref().map_or(true, |book| book.name != book_name) {
        *current_book = Some(Book {
            name: book_name.clone(),
        });
        *playback_state = PlaybackState {
            last_api_time: SystemTime::now(),
            is_playing: false,
        };
    }

    let current_position = if playback_state.is_playing {
        let elapsed = now
            .duration_since(playback_state.last_api_time)
            .unwrap_or(Duration::from_secs(0))
            .as_secs_f64();
        
        let adjusted_elapsed = elapsed * 0.8;
        
        if (current_time - timing_info.last_position.unwrap_or(0.0)).abs() > f64::EPSILON {
            playback_state.last_api_time = now;
            current_time
        } else {
            timing_info.last_position.unwrap_or(current_time) + adjusted_elapsed
        }
    } else {
        current_time
    };

    let activity_type = if is_podcast {
        activity::ActivityType::Listening
    } else {
        activity::ActivityType::Listening
    };

    let mut activity_builder = if playback_state.is_playing {
        let now_secs = now.duration_since(UNIX_EPOCH)?.as_secs() as i64;
        let current_pos = current_position.max(0.0) as i64;
        let total_dur = duration.max(0.0) as i64;

        let start_time = now_secs.saturating_sub(current_pos);
        let end_time = start_time.saturating_add(total_dur);

        activity::Activity::new()
            .details(&book_name)
            .state(&author)
            .timestamps(
                activity::Timestamps::new()
                    .start(start_time)
                    .end(end_time)
            )
            .activity_type(activity_type)
    } else {
        activity::Activity::new()
            .details(&book_name)
            .state(&author)
            .activity_type(activity_type)
    };

    let cover_url = get_cover_path(
        client,
        config,
        &book_name,
        &author,
        &session.libraryItemId,
        imgur_cache,
        is_podcast,
        cache_file,
    )
    .await?;

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
    library_item_id: &str,
    imgur_cache: &mut HashMap<String, String>,
    is_podcast: bool,
    cache_file: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if let Some(cached) = imgur_cache.get(library_item_id) {
        info!("Using cached cover URL for {}", library_item_id);
        return Ok(Some(cached.clone()));
    }
    if let Some(abs_cover_url) = get_cover_from_abs(
        client,
        config,
        library_item_id,
        imgur_cache,
        cache_file,
    )
    .await?
    {
        return Ok(Some(abs_cover_url));
    }

    if is_podcast {
        return Ok(None);
    }

    let search_title = if let Some(book_num) = extract_book_number(title) {
        format!("{} {}", get_base_title(title), book_num)
    } else {
        get_base_title(title).to_string()
    };

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
        let title = search_title.clone();
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
            imgur_cache.insert(library_item_id.to_string(), url.clone());
            if let Err(e) = save_imgur_cache(cache_file, imgur_cache) {
                warn!("Failed to persist urls.json: {}", e);
            }
            return Ok(Some(url));
        }
    }

    Ok(None)
}

fn extract_book_number(title: &str) -> Option<String> {
    if let Some(idx) = title.find("Book ") {
        let after_book = &title[idx + 5..];
        if let Some(end) = after_book.find(|c: char| !c.is_numeric()) {
            return Some(format!("Book {}", &after_book[..end]));
        }
    }
    None
}

fn get_base_title(title: &str) -> &str {
    if let Some(idx) = title.find(|c| c == ':' || c == '(') {
        title[..idx].trim()
    } else {
        title.trim()
    }
}

fn has_chapter_prefix(title: &str) -> bool {
    let title_lower = title.to_lowercase();
    let chapter_words = vec![
        "chapter", "chap", "ch",
        "hoofdstuk", "hfdst", 
        "kapitel", "kap",
        "chapitre",
        "capitulo", "capítulo", "cap",
        "capitolo",
        "rozdział", "rozd",
        "глава",
        "章", "第",
        "luku",
        "poglavlje",
        "fejezet",
        "bölüm",
        "part", "partie", "parte", "deel", "teil"
    ];
    
    for word in chapter_words {
        if title_lower.starts_with(&format!("{} ", word)) || 
           title_lower.starts_with(&format!("{}.", word)) ||
           title_lower.starts_with(&format!("{}-", word)) {
            return true;
        }
    }
    false
}

async fn get_cover_from_abs(
    client: &Client,
    config: &Config,
    library_item_id: &str,
    imgur_cache: &mut HashMap<String, String>,
    cache_file: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let want_imgur = !config.use_abs_cover.unwrap_or(false);

    if let Some(cached_url) = imgur_cache.get(library_item_id) {
        return Ok(Some(cached_url.clone()));
    }

    let cover_url = format!(
        "{}/api/items/{}/cover?width=400&format=jpeg",
        config.audiobookshelf_url,
        library_item_id
    );

    let response = client
        .get(&cover_url)
        .bearer_auth(&config.audiobookshelf_token)
        .send()
        .await?;

    if !response.status().is_success() {
        info!("No cover found for library item: {}", library_item_id);
        return Ok(None);
    }

    if want_imgur {
        if let Some(imgur_client_id) = &config.imgur_client_id {
            let image_bytes = response.bytes().await?;
            match upload_to_imgur(client, imgur_client_id, &image_bytes).await {
                Ok(imgur_url) => {
                    info!("Successfully uploaded cover to Imgur: {}", imgur_url);
                    imgur_cache.insert(library_item_id.to_string(), imgur_url.clone());
                    if let Err(e) = save_imgur_cache(cache_file, imgur_cache) {
                        warn!("Failed to persist urls.json: {}", e);
                    }
                    return Ok(Some(imgur_url));
                }
                Err(e) => {
                    warn!("Failed to upload to Imgur: {}", e);
                    imgur_cache.insert(library_item_id.to_string(), cover_url.clone());
                    if let Err(e2) = save_imgur_cache(cache_file, imgur_cache) {
                        warn!("Failed to persist urls.json: {}", e2);
                    }
                    return Ok(Some(cover_url));
                }
            }
        } else {
            warn!("use_abs_cover is false but imgur_client_id is missing; using ABS URL instead.");
            imgur_cache.insert(library_item_id.to_string(), cover_url.clone());
            if let Err(e) = save_imgur_cache(cache_file, imgur_cache) {
                warn!("Failed to persist urls.json: {}", e);
            }
            return Ok(Some(cover_url));
        }
    } else {
        imgur_cache.insert(library_item_id.to_string(), cover_url.clone());
        if let Err(e) = save_imgur_cache(cache_file, imgur_cache) {
            warn!("Failed to persist urls.json: {}", e);
        }
        return Ok(Some(cover_url));
    }
}

async fn upload_to_imgur(
    client: &Client,
    client_id: &str,
    image_data: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    let part = reqwest::multipart::Part::bytes(image_data.to_vec())
        .file_name("cover.jpg")
        .mime_str("image/jpeg")?;
    
    let form = reqwest::multipart::Form::new()
        .part("image", part);

    let response = client
        .post("https://api.imgur.com/3/image")
        .header("Authorization", format!("Client-ID {}", client_id))
        .multipart(form)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("Imgur upload failed with status: {} - {}", status, error_text).into());
    }

    let imgur_response: ImgurResponse = response.json().await?;
    
    if !imgur_response.success {
        return Err("Imgur upload was not successful".into());
    }

    Ok(imgur_response.data.link)
}

async fn check_for_update(client: &Client) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let url = "https://api.github.com/repos/0xGingi/audiobookshelf-discord-rpc/releases/latest";
    let resp = client
        .get(url)
        .header("User-Agent", "Audiobookshelf-Discord-RPC")
        .send()
        .await?;

    if !resp.status().is_success() {
        if resp.status() == 403 {
            warn!("GitHub API rate limit exceeded (403 Forbidden). Skipping version check.");
            return Ok(None);
        }
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

fn cache_file_path(config_file: &str) -> PathBuf {
    let path = Path::new(config_file);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    dir.join("urls.json")
}

fn load_imgur_cache_with_fallback(primary: &Path) -> HashMap<String, String> {
    if primary.exists() {
        return load_imgur_cache(primary);
    }
    let fallback = Path::new("urls.json");
    if fallback.exists() {
        info!(
            "Primary cache not found at {:?}; loading fallback from {:?}",
            primary, fallback
        );
        return load_imgur_cache(fallback);
    }
    load_imgur_cache(primary)
}

fn load_imgur_cache(path: &Path) -> HashMap<String, String> {
    match fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<HashMap<String, String>>(&contents) {
            Ok(map) => {
                info!("Loaded {} cached cover URLs from {:?}", map.len(), path);
                map
            }
            Err(e) => {
                warn!("Failed to parse {:?} (starting empty cache): {}", path, e);
                HashMap::new()
            }
        },
        Err(err) => {
            if err.kind() != ErrorKind::NotFound {
                warn!("Failed to read {:?} (starting empty cache): {}", path, err);
            } else {
                info!("No existing cache at {:?}; starting fresh", path);
            }
            HashMap::new()
        }
    }
}

fn save_imgur_cache(path: &Path, cache: &HashMap<String, String>) -> Result<(), Box<dyn std::error::Error>> {
    let data = serde_json::to_string_pretty(cache)?;
    fs::write(path, data)?;
    Ok(())
}
