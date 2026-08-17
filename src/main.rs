use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use env_logger;
use futures::future::join_all;
use log::{error, info, warn};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time;
use url::Url;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// A session counts as "playing" if the server saw a client sync within this window.
// ABS clients sync every ~10s (web) to ~15s (mobile) while playing, and go silent
// when paused/stopped, so freshness of `updatedAt` is the playing signal.
// ponytail: assumes this machine and the ABS server have reasonably synced clocks.
const PAUSE_THRESHOLD_SECS: f64 = 35.0;
const ACTIVE_POLL_SECS: u64 = 5;
const IDLE_POLL_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct Config {
    discord_client_id: String,
    audiobookshelf_url: String,
    audiobookshelf_token: String,
    show_chapters: Option<bool>,
    use_abs_cover: Option<bool>,
    imgbb_api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    tag_name: String,
}

#[derive(Debug, Deserialize)]
struct ListeningSessionsResponse {
    sessions: Vec<Session>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
struct Session {
    displayTitle: String,
    displayAuthor: String,
    currentTime: f64,
    duration: f64,
    mediaMetadata: MediaMetadata,
    libraryItemId: String,
    #[serde(rename = "mediaType")]
    media_type: Option<String>,
    /// Millisecond epoch of the last client progress sync for this session.
    updatedAt: i64,
}

#[derive(Debug, Clone, Deserialize)]
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
}

#[derive(Debug, Deserialize)]
struct MediaResponse {
    #[serde(default)]
    chapters: Vec<Chapter>,
}

// Fallback playback signal for 3rd-party clients (e.g. Voca) that sync
// /api/me media progress directly without ever opening a listening session.
#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct MeResponse {
    #[serde(default)]
    mediaProgress: Vec<MediaProgress>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct MediaProgress {
    libraryItemId: String,
    episodeId: Option<String>,
    #[serde(default)]
    currentTime: f64,
    #[serde(default)]
    duration: f64,
    /// Millisecond epoch of the last progress sync.
    lastUpdate: i64,
}

#[derive(Debug, Deserialize)]
struct ItemDetailsResponse {
    #[serde(rename = "mediaType")]
    media_type: Option<String>,
    media: ItemDetailsMedia,
}

#[derive(Debug, Deserialize)]
struct ItemDetailsMedia {
    metadata: ItemDetailsMetadata,
    #[serde(default)]
    episodes: Vec<PodcastEpisode>,
}

#[derive(Debug, Deserialize)]
struct ItemDetailsMetadata {
    title: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    authors: Vec<AuthorRef>,
    author: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorRef {
    name: String,
}

#[derive(Debug, Deserialize)]
struct PodcastEpisode {
    id: String,
    title: Option<String>,
    season: Option<String>,
    episode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoverResponse {
    results: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ImgbbResponse {
    data: ImgbbData,
    success: bool,
}

#[derive(Debug, Deserialize)]
struct ImgbbData {
    url: String,
    display_url: Option<String>,
}

/// The last activity we sent to Discord, so we only write over IPC when
/// something visible changed. Discord ticks the progress bar client-side.
#[derive(Debug)]
struct LastActivity {
    book: String,
    large_text: String,
    start_time: i64,
}

#[derive(Debug, Default)]
struct State {
    activity_cleared: bool,
    last_activity: Option<LastActivity>,
    chapters_item_id: String,
    chapters: Vec<Chapter>,
    cover_url_cache: HashMap<String, String>,
    /// Item metadata for the progress-based fallback, fetched once per
    /// item/episode and reused with fresh position numbers each poll.
    progress_key: String,
    progress_template: Option<Session>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let client = Client::new();

    match check_for_update(&client).await {
        Ok(Some(latest_version)) => {
            info!(
                "A new version is available: {}. You're currently running version {}.",
                latest_version, CURRENT_VERSION
            );
            info!("Please re-run the installer or visit https://github.com/0xGingi/audiobookshelf-discord-rpc/releases to download the latest version.");
        }
        Ok(None) => info!("You're running the latest version: {}", CURRENT_VERSION),
        Err(e) => warn!("Version check failed (continuing anyway): {}", e),
    }

    let config_file = parse_args()?;
    info!("Using config file: {}", config_file);

    let config = load_config(&config_file)?;
    info!("Using Audiobookshelf authentication (API Key recommended for v2.26.0+)");
    let mut discord = DiscordIpcClient::new(&config.discord_client_id);
    discord.connect()?;
    info!("Audiobookshelf Discord RPC Connected!");

    let cache_file = cache_file_path(&config_file);
    let mut state = State {
        cover_url_cache: load_cover_url_cache_with_fallback(&cache_file),
        ..State::default()
    };

    let mut playing = false;
    loop {
        match set_activity(&client, &config, &mut discord, &mut state, &cache_file).await {
            Ok(is_playing) => playing = is_playing,
            Err(e) => {
                let mut is_pipe_error = false;
                if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                    if io_err.kind() == ErrorKind::BrokenPipe
                        || io_err.raw_os_error() == Some(232)
                        || io_err.raw_os_error() == Some(32)
                    {
                        is_pipe_error = true;
                    }
                }

                if !is_pipe_error {
                    let mut source = e.source();
                    while let Some(err) = source {
                        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                            if io_err.kind() == ErrorKind::BrokenPipe
                                || io_err.raw_os_error() == Some(232)
                                || io_err.raw_os_error() == Some(32)
                            {
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
                        error!(
                            "Error closing old Discord client (connection likely already broken): {}",
                            close_err
                        );
                    }
                    // Force a full re-send once reconnected.
                    state.last_activity = None;
                    state.activity_cleared = false;
                    time::sleep(Duration::from_secs(5)).await;
                    let mut new_discord = DiscordIpcClient::new(&config.discord_client_id);
                    if let Err(connect_err) = new_discord.connect() {
                        error!("Failed to reconnect to Discord: {}", connect_err);
                    } else {
                        info!("Successfully reconnected to Discord.");
                        discord = new_discord;
                    }
                } else {
                    error!(
                        "Error setting activity (not identified as pipe error): {}",
                        e
                    );
                    error!("Full error details: {:?}", e);
                }
            }
        }
        let sleep_secs = if playing {
            ACTIVE_POLL_SECS
        } else {
            IDLE_POLL_SECS
        };
        time::sleep(Duration::from_secs(sleep_secs)).await;
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

fn clear_activity_if_needed(
    discord: &mut DiscordIpcClient,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error>> {
    if !state.activity_cleared {
        info!("No active playback; clearing Discord activity");
        discord.clear_activity()?;
        state.activity_cleared = true;
        state.last_activity = None;
    }
    Ok(())
}

/// Returns whether a session is actively playing (drives the poll interval).
async fn set_activity(
    client: &Client,
    config: &Config,
    discord: &mut DiscordIpcClient,
    state: &mut State,
    cache_file: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
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

    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

    let fresh_session = resp
        .sessions
        .into_iter()
        .next()
        .filter(|s| ((now_ms - s.updatedAt) as f64 / 1000.0) < PAUSE_THRESHOLD_SECS);

    // Some 3rd-party clients (e.g. Voca) never open a listening session and
    // only sync media progress, so fall back to progress freshness.
    let session = match fresh_session {
        Some(session) => session,
        None => match fetch_progress_session(client, config, state, now_ms).await? {
            Some(session) => session,
            None => {
                clear_activity_if_needed(discord, state)?;
                return Ok(false);
            }
        },
    };

    let secs_since_sync = (now_ms - session.updatedAt) as f64 / 1000.0;

    // Server-anchored position: last synced position plus time elapsed since
    // the server recorded it. Accurate to within one client sync interval.
    let position = (session.currentTime + secs_since_sync.max(0.0)).min(session.duration);

    let is_podcast = session.media_type.as_deref() == Some("podcast")
        || session.mediaMetadata.podcast_title.is_some();

    // Chapters are not included in the listening-sessions payload, so fetch
    // them from the library item -- once per book, not every poll.
    if config.show_chapters.unwrap_or(false)
        && !is_podcast
        && state.chapters_item_id != session.libraryItemId
    {
        match fetch_chapters(client, config, &session.libraryItemId).await {
            Ok(chapters) => {
                state.chapters = chapters;
                state.chapters_item_id = session.libraryItemId.clone();
            }
            Err(e) => warn!("Failed to fetch chapters (will retry): {}", e),
        }
    }

    let genres = session
        .mediaMetadata
        .genres
        .first()
        .map(|s| s.as_str())
        .unwrap_or("Unknown Genre");

    let large_text = if is_podcast {
        if let Some(podcast_title) = &session.mediaMetadata.podcast_title {
            if let (Some(season), Some(episode)) = (
                &session.mediaMetadata.season,
                &session.mediaMetadata.episode,
            ) {
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
        if let Some(current_chapter) = state
            .chapters
            .iter()
            .find(|ch| position >= ch.start && position <= ch.end)
        {
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
        let podcast_title = session
            .mediaMetadata
            .title
            .as_ref()
            .or(session.mediaMetadata.podcast_title.as_ref());
        if let Some(title) = podcast_title {
            (title.clone(), session.displayTitle.clone())
        } else {
            (session.displayTitle.clone(), session.displayAuthor.clone())
        }
    } else {
        (session.displayTitle.clone(), session.displayAuthor.clone())
    };

    let now_secs = now_ms / 1000;
    let start_time = now_secs - position.max(0.0) as i64;
    let end_time = start_time.saturating_add(session.duration.max(0.0) as i64);

    // Discord ticks the bar on its own; only re-send when the book, chapter,
    // or anchor moved (a seek or a fresh sync shifts start_time by >2s).
    if let Some(last) = &state.last_activity {
        if last.book == book_name
            && last.large_text == large_text
            && (last.start_time - start_time).abs() <= 2
        {
            return Ok(true);
        }
    }

    let mut activity_builder = activity::Activity::new()
        .details(&book_name)
        .state(&author)
        .timestamps(activity::Timestamps::new().start(start_time).end(end_time))
        .activity_type(activity::ActivityType::Listening);

    let cover_url = get_cover_path(
        client,
        config,
        &book_name,
        &author,
        &session.libraryItemId,
        &mut state.cover_url_cache,
        is_podcast,
        cache_file,
    )
    .await?;

    if let Some(ref url) = cover_url {
        activity_builder = activity_builder.assets(
            activity::Assets::new()
                .large_image(url)
                .large_text(&large_text),
        );
    }

    discord.set_activity(activity_builder)?;
    info!(
        "Updated activity: \"{}\" at {:.0}s / {:.0}s",
        book_name, position, session.duration
    );
    state.activity_cleared = false;
    state.last_activity = Some(LastActivity {
        book: book_name,
        large_text,
        start_time,
    });

    Ok(true)
}

/// Builds a synthetic Session from the freshest /api/me media-progress entry,
/// or None if nothing synced within the pause threshold.
async fn fetch_progress_session(
    client: &Client,
    config: &Config,
    state: &mut State,
    now_ms: i64,
) -> Result<Option<Session>, Box<dyn std::error::Error>> {
    let me: MeResponse = client
        .get(format!("{}/api/me", config.audiobookshelf_url))
        .bearer_auth(&config.audiobookshelf_token)
        .send()
        .await?
        .json()
        .await?;

    let progress = match me.mediaProgress.into_iter().max_by_key(|p| p.lastUpdate) {
        Some(p) => p,
        None => return Ok(None),
    };

    if (now_ms - progress.lastUpdate) as f64 / 1000.0 >= PAUSE_THRESHOLD_SECS {
        return Ok(None);
    }

    let key = format!(
        "{}/{}",
        progress.libraryItemId,
        progress.episodeId.as_deref().unwrap_or("")
    );
    if state.progress_key != key || state.progress_template.is_none() {
        let item: ItemDetailsResponse = client
            .get(format!(
                "{}/api/items/{}",
                config.audiobookshelf_url, progress.libraryItemId
            ))
            .bearer_auth(&config.audiobookshelf_token)
            .send()
            .await?
            .json()
            .await?;

        let is_podcast = item.media_type.as_deref() == Some("podcast");
        let episode = progress
            .episodeId
            .as_ref()
            .and_then(|id| item.media.episodes.iter().find(|e| &e.id == id));
        let (ep_title, ep_season, ep_episode) = match episode {
            Some(e) => (e.title.clone(), e.season.clone(), e.episode.clone()),
            None => (None, None, None),
        };

        let item_title = item.media.metadata.title.clone();
        let display_title = if is_podcast {
            ep_title.or_else(|| item_title.clone())
        } else {
            item_title.clone()
        }
        .unwrap_or_else(|| "Unknown".to_string());
        let display_author = if is_podcast {
            item.media.metadata.author.clone().unwrap_or_default()
        } else {
            item.media
                .metadata
                .authors
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };

        state.progress_template = Some(Session {
            displayTitle: display_title,
            displayAuthor: display_author,
            currentTime: 0.0,
            duration: 0.0,
            mediaMetadata: MediaMetadata {
                genres: item.media.metadata.genres,
                podcast_title: if is_podcast { item_title.clone() } else { None },
                title: if is_podcast { item_title } else { None },
                season: ep_season,
                episode: ep_episode,
            },
            libraryItemId: progress.libraryItemId.clone(),
            media_type: item.media_type,
            updatedAt: 0,
        });
        state.progress_key = key;
    }

    let mut session = state.progress_template.clone().unwrap();
    session.currentTime = progress.currentTime;
    session.duration = progress.duration;
    session.updatedAt = progress.lastUpdate;
    Ok(Some(session))
}

async fn fetch_chapters(
    client: &Client,
    config: &Config,
    library_item_id: &str,
) -> Result<Vec<Chapter>, Box<dyn std::error::Error>> {
    let library_item_url = format!(
        "{}/api/items/{}?include=chapters",
        config.audiobookshelf_url, library_item_id
    );

    let library_item: LibraryItemResponse = client
        .get(&library_item_url)
        .bearer_auth(&config.audiobookshelf_token)
        .send()
        .await?
        .json()
        .await?;

    Ok(library_item.media.chapters)
}

async fn get_cover_path(
    client: &Client,
    config: &Config,
    title: &str,
    author: &str,
    library_item_id: &str,
    cover_url_cache: &mut HashMap<String, String>,
    is_podcast: bool,
    cache_file: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if let Some(cached) = cover_url_cache.get(library_item_id) {
        return Ok(Some(cached.clone()));
    }
    if let Some(abs_cover_url) =
        get_cover_from_abs(client, config, library_item_id, cover_url_cache, cache_file).await?
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
                &[
                    ("title", title.as_str()),
                    ("author", author.as_str()),
                    ("provider", *provider),
                ],
            )?;
            let resp: CoverResponse = client
                .get(url)
                .bearer_auth(&config.audiobookshelf_token)
                .send()
                .await?
                .json()
                .await?;
            if let Some(cover_url) = resp.results.first() {
                return Ok(Some(cover_url.clone()));
            }
            Ok(None)
        }
    });

    let results: Vec<Result<Option<String>, Box<dyn std::error::Error>>> = join_all(futures).await;
    for result in results {
        if let Ok(Some(url)) = result {
            cover_url_cache.insert(library_item_id.to_string(), url.clone());
            if let Err(e) = save_cover_url_cache(cache_file, cover_url_cache) {
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
        "chapter",
        "chap",
        "ch",
        "hoofdstuk",
        "hfdst",
        "kapitel",
        "kap",
        "chapitre",
        "capitulo",
        "capítulo",
        "cap",
        "capitolo",
        "rozdział",
        "rozd",
        "глава",
        "章",
        "第",
        "luku",
        "poglavlje",
        "fejezet",
        "bölüm",
        "part",
        "partie",
        "parte",
        "deel",
        "teil",
    ];

    for word in chapter_words {
        if title_lower.starts_with(&format!("{} ", word))
            || title_lower.starts_with(&format!("{}.", word))
            || title_lower.starts_with(&format!("{}-", word))
        {
            return true;
        }
    }
    false
}

async fn get_cover_from_abs(
    client: &Client,
    config: &Config,
    library_item_id: &str,
    cover_url_cache: &mut HashMap<String, String>,
    cache_file: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let want_image_host_upload = !config.use_abs_cover.unwrap_or(false);

    let cover_url = format!(
        "{}/api/items/{}/cover?width=400&format=jpeg",
        config.audiobookshelf_url, library_item_id
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

    if want_image_host_upload {
        if let Some(imgbb_api_key) = &config.imgbb_api_key {
            let image_bytes = response.bytes().await?;
            match upload_to_imgbb(client, imgbb_api_key, &image_bytes).await {
                Ok(imgbb_url) => {
                    info!("Successfully uploaded cover to ImgBB: {}", imgbb_url);
                    cover_url_cache.insert(library_item_id.to_string(), imgbb_url.clone());
                    if let Err(e) = save_cover_url_cache(cache_file, cover_url_cache) {
                        warn!("Failed to persist urls.json: {}", e);
                    }
                    return Ok(Some(imgbb_url));
                }
                Err(e) => {
                    warn!("Failed to upload to ImgBB: {}", e);
                    // Do NOT cache the ABS URL on failure — Discord cannot access
                    // private/authenticated ABS endpoints. Leave the cache empty so
                    // the upload is retried on the next cycle.
                    return Ok(None);
                }
            }
        } else {
            warn!("use_abs_cover is false but imgbb_api_key is missing; using ABS URL instead.");
            cover_url_cache.insert(library_item_id.to_string(), cover_url.clone());
            if let Err(e) = save_cover_url_cache(cache_file, cover_url_cache) {
                warn!("Failed to persist urls.json: {}", e);
            }
            return Ok(Some(cover_url));
        }
    } else {
        cover_url_cache.insert(library_item_id.to_string(), cover_url.clone());
        if let Err(e) = save_cover_url_cache(cache_file, cover_url_cache) {
            warn!("Failed to persist urls.json: {}", e);
        }
        return Ok(Some(cover_url));
    }
}

async fn upload_to_imgbb(
    client: &Client,
    api_key: &str,
    image_data: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    let part = reqwest::multipart::Part::bytes(image_data.to_vec())
        .file_name("cover.jpg")
        .mime_str("image/jpeg")?;

    let form = reqwest::multipart::Form::new().part("image", part);

    let upload_url = Url::parse_with_params("https://api.imgbb.com/1/upload", &[("key", api_key)])?;

    let response = client.post(upload_url).multipart(form).send().await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!(
            "ImgBB upload failed with status: {} - {}",
            status, error_text
        )
        .into());
    }

    let imgbb_response: ImgbbResponse = response.json().await?;

    if !imgbb_response.success {
        return Err("ImgBB upload was not successful".into());
    }

    Ok(imgbb_response
        .data
        .display_url
        .unwrap_or(imgbb_response.data.url))
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

fn load_cover_url_cache_with_fallback(primary: &Path) -> HashMap<String, String> {
    if primary.exists() {
        return load_cover_url_cache(primary);
    }
    let fallback = Path::new("urls.json");
    if fallback.exists() {
        info!(
            "Primary cache not found at {:?}; loading fallback from {:?}",
            primary, fallback
        );
        return load_cover_url_cache(fallback);
    }
    load_cover_url_cache(primary)
}

fn load_cover_url_cache(path: &Path) -> HashMap<String, String> {
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

fn save_cover_url_cache(
    path: &Path,
    cache: &HashMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let data = serde_json::to_string_pretty(cache)?;
    fs::write(path, data)?;
    Ok(())
}
