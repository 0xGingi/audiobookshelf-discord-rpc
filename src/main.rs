use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use futures::future::join_all;
use log::{debug, error, info, warn};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, ErrorKind, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time;
use url::Url;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_DISCORD_CLIENT_ID: &str = "1283070638088650752";
const DISCORD_RETRY_SECS: u64 = 15;

const COVER_PROVIDERS: [&str; 14] = [
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

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn FreeConsole() -> i32;
}

// A session counts as "playing" if the server saw a client sync within this window.
// ABS clients sync every ~10s (web) to ~15s (mobile) while playing, and go silent
// when paused/stopped, so freshness of `updatedAt` is the playing signal.
// ponytail: assumes this machine and the ABS server have reasonably synced clocks.
const PAUSE_THRESHOLD_SECS: f64 = 35.0;
const ACTIVE_POLL_SECS: u64 = 5;
const IDLE_POLL_SECS: u64 = 30;
// set_activity never reads from the socket, so an unchanged activity is re-sent this
// often: the write failing is how we notice Discord restarted.
const REFRESH_SECS: i64 = 60;
// Slack statuses expire on their own, so a crash or lost network can't leave one stuck;
// we re-send well before expiry while playing.
const SLACK_EXPIRY_SECS: i64 = 600;
const SLACK_REFRESH_SECS: i64 = 300;

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default = "default_discord_client_id")]
    discord_client_id: String,
    audiobookshelf_url: String,
    audiobookshelf_token: String,
    show_chapters: Option<bool>,
    use_abs_cover: Option<bool>,
    imgbb_api_key: Option<String>,
    /// Defaults to true; set false to run Slack-only on a machine without Discord.
    discord: Option<bool>,
    slack_token: Option<String>,
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
    /// Podcast artwork from the RSS feed; already publicly hosted.
    #[serde(rename = "imageUrl")]
    image_url: Option<String>,
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
    #[serde(rename = "imageUrl")]
    image_url: Option<String>,
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
    sent_at: i64,
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
    /// Last Slack status text we set ("" = none) and when.
    slack_text: String,
    slack_sent_at: i64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let has_flag = |flag: &str| args.iter().any(|a| a == flag);
    let config_file = match args.iter().position(|a| a == "-c") {
        Some(i) => args
            .get(i + 1)
            .ok_or("missing argument for -c option")?
            .clone(),
        None => "config.json".to_string(),
    };

    let mut logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if has_flag("--background") {
        // Autostart mode: close the console window Windows opens for us, log to a file instead.
        #[cfg(windows)]
        unsafe {
            FreeConsole();
        }
        let log_path =
            cache_file_path(&config_file).with_file_name("audiobookshelf-discord-rpc.log");
        if let Ok(file) = fs::File::create(log_path) {
            logger.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }
    logger.init();

    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

    let setup_only = has_flag("--setup");
    if setup_only || (!Path::new(&config_file).exists() && io::stdin().is_terminal()) {
        setup(&client, &config_file).await?;
        if setup_only {
            return Ok(());
        }
    }

    match check_for_update(&client).await {
        Ok(Some(latest_version)) => {
            info!(
                "A new version is available: {}. You're currently running version {}.",
                latest_version, CURRENT_VERSION
            );
            info!("To update, re-run the install command: https://github.com/0xGingi/audiobookshelf-discord-rpc#install");
        }
        Ok(None) => info!("You're running the latest version: {}", CURRENT_VERSION),
        Err(e) => warn!("Version check failed (continuing anyway): {}", e),
    }

    info!("Using config file: {}", config_file);
    let config = load_config(&config_file)
        .map_err(|e| format!("couldn't load {config_file}: {e} (run with --setup to create it)"))
        .inspect_err(|e| error!("{e}"))?;

    let cache_file = cache_file_path(&config_file);
    let mut cover_url_cache = load_cover_url_cache_with_fallback(&cache_file);
    // Older versions cached private ABS cover links that Discord can't load; drop them.
    cover_url_cache.retain(|_, url| !url.contains("/api/items/"));
    let mut state = State {
        cover_url_cache,
        ..State::default()
    };

    // ponytail: with Discord enabled, this (and reconnects) block until Discord runs, so
    // Slack stalls meanwhile; make the connect non-blocking if that ever bites.
    let mut discord = if config.discord.unwrap_or(true) {
        Some(connect_discord(&config.discord_client_id).await)
    } else {
        None
    };
    loop {
        let playing =
            match set_activity(&client, &config, &mut discord, &mut state, &cache_file).await {
                Ok(playing) => playing,
                // Discord restarted, updated, or quit: wait for it to come back.
                Err(e) if e.is::<discord_rich_presence::error::Error>() => {
                    warn!("Lost connection to Discord ({}); reconnecting...", e);
                    if let Some(d) = discord.as_mut() {
                        let _ = d.close();
                    }
                    // Force a full re-send once reconnected.
                    state.last_activity = None;
                    state.activity_cleared = false;
                    discord = Some(connect_discord(&config.discord_client_id).await);
                    continue;
                }
                Err(e) => {
                    error!("Error updating activity: {}", e);
                    false
                }
            };
        let sleep_secs = if playing {
            ACTIVE_POLL_SECS
        } else {
            IDLE_POLL_SECS
        };
        time::sleep(Duration::from_secs(sleep_secs)).await;
    }
}

fn default_discord_client_id() -> String {
    DEFAULT_DISCORD_CLIENT_ID.to_string()
}

fn load_config(config_file: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let config_str = fs::read_to_string(config_file)?;
    let mut config: Config = serde_json::from_str(&config_str)?;
    config.audiobookshelf_url = config.audiobookshelf_url.trim_end_matches('/').to_string();
    Ok(config)
}

/// Waits until Discord is running and accepts the IPC handshake.
async fn connect_discord(client_id: &str) -> DiscordIpcClient {
    let mut warned = false;
    loop {
        let mut discord = DiscordIpcClient::new(client_id);
        match discord.connect() {
            Ok(()) => {
                info!("Connected to Discord");
                return discord;
            }
            Err(e) if !warned => {
                warn!(
                    "Can't reach Discord ({}); is it running? Retrying every {}s...",
                    e, DISCORD_RETRY_SECS
                );
                warned = true;
            }
            Err(_) => {}
        }
        time::sleep(Duration::from_secs(DISCORD_RETRY_SECS)).await;
    }
}

/// Interactive first-run wizard: asks for the few things we can't guess and writes the config.
async fn setup(client: &Client, config_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("\nAudiobookshelf Discord RPC setup (saving to {config_file})\n");
    let (url, token) = loop {
        let mut url = prompt("Audiobookshelf URL, e.g. http://192.168.1.10:13378")?;
        if !url.contains("://") {
            url = format!("http://{url}");
        }
        let url = url.trim_end_matches('/').to_string();
        let token = prompt("API key (ABS > Settings > API Keys)")?;
        let check = client
            .get(format!("{url}/api/me"))
            .bearer_auth(&token)
            .send()
            .await
            .and_then(|r| r.error_for_status());
        match check {
            Ok(_) => break (url, token),
            Err(e) => println!(
                "Couldn't log in to Audiobookshelf: {e}\nCheck the URL and key and try again.\n"
            ),
        }
    };
    println!("Logged in!\n");

    let show_chapters = !prompt("Show the current chapter instead of the genre? [Y/n]")?
        .to_lowercase()
        .starts_with('n');

    println!("\nCovers: Discord can't load images from a private server. With a free ImgBB key");
    println!("(https://api.imgbb.com/) your exact ABS covers are uploaded there; without one,");
    println!("covers are looked up on Audible and other metadata providers.");
    let imgbb_api_key = prompt("ImgBB API key (Enter to skip)")?;

    let mut config = serde_json::json!({
        "audiobookshelf_url": url,
        "audiobookshelf_token": token,
        "show_chapters": show_chapters,
    });
    if !imgbb_api_key.is_empty() {
        config["imgbb_api_key"] = imgbb_api_key.into();
    }
    if let Some(dir) = Path::new(config_file).parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(config_file, serde_json::to_string_pretty(&config)?)?;
    // The file holds your ABS API key.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(config_file, fs::Permissions::from_mode(0o600))?;
    }
    println!("\nSaved {config_file}\n");
    Ok(())
}

fn prompt(label: &str) -> io::Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        return Err(io::Error::new(
            ErrorKind::UnexpectedEof,
            "no input (stdin closed)",
        ));
    }
    Ok(input.trim().to_string())
}

/// Discord rejects activities with text fields over 128 characters.
fn clip(s: &str) -> String {
    s.chars().take(128).collect()
}

fn clear_activity_if_needed(
    discord: &mut Option<DiscordIpcClient>,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error>> {
    if !state.activity_cleared {
        if let Some(discord) = discord {
            info!("No active playback; clearing Discord activity");
            discord.clear_activity()?;
        }
        state.activity_cleared = true;
        state.last_activity = None;
    }
    Ok(())
}

/// Returns whether a session is actively playing (drives the poll interval).
async fn set_activity(
    client: &Client,
    config: &Config,
    discord: &mut Option<DiscordIpcClient>,
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
        .error_for_status()?
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
                update_slack(client, config, state, "", now_ms / 1000).await;
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
    update_slack(
        client,
        config,
        state,
        &slack_status_text(&book_name, &author),
        now_secs,
    )
    .await;
    let Some(discord) = discord else {
        return Ok(true);
    };
    let start_time = now_secs - position.max(0.0) as i64;
    let end_time = start_time.saturating_add(session.duration.max(0.0) as i64);

    // Discord ticks the bar on its own; only re-send when the book, chapter,
    // or anchor moved (a seek or a fresh sync shifts start_time by >2s).
    let unchanged = state.last_activity.as_ref().is_some_and(|last| {
        last.book == book_name
            && last.large_text == large_text
            && (last.start_time - start_time).abs() <= 2
    });
    let refresh_due = state
        .last_activity
        .as_ref()
        .is_none_or(|last| now_secs - last.sent_at >= REFRESH_SECS);
    if unchanged && !refresh_due {
        return Ok(true);
    }

    let mut activity_builder = activity::Activity::new()
        .details(clip(&book_name))
        .state(clip(&author))
        .timestamps(activity::Timestamps::new().start(start_time).end(end_time))
        .activity_type(activity::ActivityType::Listening)
        // Member list reads "Listening to <book>" instead of the app name.
        .status_display_type(activity::StatusDisplayType::Details);

    let cover_url = get_cover_url(
        client,
        config,
        &session,
        is_podcast,
        &mut state.cover_url_cache,
        cache_file,
    )
    .await
    .unwrap_or_else(|e| {
        warn!("Cover lookup failed (will retry): {}", e);
        None
    });

    if let Some(ref url) = cover_url {
        activity_builder = activity_builder.assets(
            activity::Assets::new()
                .large_image(url)
                .large_text(clip(&large_text)),
        );
    }

    discord.set_activity(activity_builder)?;
    if unchanged {
        debug!("Refreshed activity: \"{}\"", book_name);
    } else {
        info!(
            "Updated activity: \"{}\" at {:.0}s / {:.0}s",
            book_name, position, session.duration
        );
    }
    state.activity_cleared = false;
    state.last_activity = Some(LastActivity {
        book: book_name,
        large_text,
        start_time,
        sent_at: now_secs,
    });

    Ok(true)
}

/// Status text is capped at 100 characters by Slack.
fn slack_status_text(book: &str, author: &str) -> String {
    let text = if author.is_empty() {
        book.to_string()
    } else {
        format!("{book} — {author}")
    };
    text.chars().take(100).collect()
}

/// Sets the Slack status to `text` (or clears it, for "") when it changed or is near expiry.
/// Failures are logged, not returned, so Slack trouble never blocks Discord.
async fn update_slack(client: &Client, config: &Config, state: &mut State, text: &str, now: i64) {
    let Some(token) = &config.slack_token else {
        return;
    };
    let due = state.slack_text != text
        || (!text.is_empty() && now - state.slack_sent_at >= SLACK_REFRESH_SECS);
    if !due {
        return;
    }
    let (emoji, expiration) = if text.is_empty() {
        ("", 0)
    } else {
        (":headphones:", now + SLACK_EXPIRY_SECS)
    };
    let body = serde_json::json!({"profile": {
        "status_text": text,
        "status_emoji": emoji,
        "status_expiration": expiration,
    }});
    let result: Result<serde_json::Value, reqwest::Error> = async {
        client
            .post("https://slack.com/api/users.profile.set")
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
    .await;
    // Slack reports most errors as HTTP 200 with {"ok": false, "error": "..."}.
    match result {
        Ok(resp) if resp["ok"] == true => {
            if text.is_empty() {
                info!("Cleared Slack status");
            } else if state.slack_text != text {
                info!("Updated Slack status: \"{}\"", text);
            }
            state.slack_text = text.to_string();
            state.slack_sent_at = now;
        }
        Ok(resp) => warn!("Couldn't update Slack status: {}", resp["error"]),
        Err(e) => warn!("Couldn't update Slack status: {}", e),
    }
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
        .error_for_status()?
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
                image_url: item.media.metadata.image_url,
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

/// Returns a cover URL Discord can actually load, in order of preference:
/// direct ABS link (opt-in, public servers only), podcast feed art, cache,
/// ABS cover re-hosted on ImgBB, then a metadata-provider search for books.
async fn get_cover_url(
    client: &Client,
    config: &Config,
    session: &Session,
    is_podcast: bool,
    cover_url_cache: &mut HashMap<String, String>,
    cache_file: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let item_id = &session.libraryItemId;
    let abs_cover = format!(
        "{}/api/items/{}/cover?width=400&format=jpeg",
        config.audiobookshelf_url, item_id
    );
    if config.use_abs_cover.unwrap_or(false) {
        return Ok(Some(abs_cover));
    }
    if let Some(feed_image) = session
        .mediaMetadata
        .image_url
        .as_ref()
        .filter(|_| is_podcast)
    {
        return Ok(Some(feed_image.clone()));
    }
    if let Some(cached) = cover_url_cache.get(item_id) {
        return Ok(Some(cached.clone()));
    }

    let mut url = None;
    if let Some(key) = config
        .imgbb_api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        let resp = client
            .get(&abs_cover)
            .bearer_auth(&config.audiobookshelf_token)
            .send()
            .await?;
        // A 404 just means the item has no cover in ABS; fall through to the search.
        if resp.status().is_success() {
            // Upload errors propagate and stay uncached, so the next update retries.
            let uploaded = upload_to_imgbb(client, key, &resp.bytes().await?).await?;
            info!("Uploaded cover to ImgBB: {}", uploaded);
            url = Some(uploaded);
        }
    }
    if url.is_none() && !is_podcast {
        url = search_cover_providers(
            client,
            config,
            &session.displayTitle,
            &session.displayAuthor,
        )
        .await;
    }

    if let Some(url) = &url {
        cover_url_cache.insert(item_id.clone(), url.clone());
        if let Err(e) = save_cover_url_cache(cache_file, cover_url_cache) {
            warn!("Failed to persist urls.json: {}", e);
        }
    }
    Ok(url)
}

/// First hit across ABS's metadata providers (in priority order); their image URLs are public.
async fn search_cover_providers(
    client: &Client,
    config: &Config,
    title: &str,
    author: &str,
) -> Option<String> {
    let search_title = match extract_book_number(title) {
        Some(book_num) => format!("{} {}", get_base_title(title), book_num),
        None => get_base_title(title).to_string(),
    };

    let searches = COVER_PROVIDERS.iter().map(|provider| {
        let search_title = &search_title;
        async move {
            let url = Url::parse_with_params(
                &format!("{}/api/search/covers", config.audiobookshelf_url),
                &[
                    ("title", search_title.as_str()),
                    ("author", author),
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
            Ok::<_, Box<dyn std::error::Error>>(resp.results.into_iter().next())
        }
    });

    join_all(searches)
        .await
        .into_iter()
        .find_map(|result| result.ok().flatten())
}

fn extract_book_number(title: &str) -> Option<String> {
    let after_book = &title[title.find("Book ")? + 5..];
    let number: String = after_book
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!number.is_empty()).then(|| format!("Book {}", number))
}

fn get_base_title(title: &str) -> &str {
    if let Some(idx) = title.find([':', '(']) {
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

    let parse = |v: &str| {
        v.split('.')
            .map(|n| n.parse::<u32>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    if parse(latest_version) > parse(CURRENT_VERSION) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_parsing() {
        assert_eq!(
            extract_book_number("Dungeon Crawler Carl: Book 1").as_deref(),
            Some("Book 1")
        );
        assert_eq!(
            extract_book_number("Book 12 of the Series").as_deref(),
            Some("Book 12")
        );
        assert_eq!(extract_book_number("The Book Thief"), None);
        assert_eq!(
            get_base_title("Dungeon Crawler Carl: Book 1"),
            "Dungeon Crawler Carl"
        );
        assert!(has_chapter_prefix("Chapter 3"));
        assert!(!has_chapter_prefix("Champions"));
        assert_eq!(clip(&"é".repeat(200)).chars().count(), 128);
        assert_eq!(
            slack_status_text("Dune", "Frank Herbert"),
            "Dune — Frank Herbert"
        );
        assert_eq!(slack_status_text("Dune", ""), "Dune");
        assert_eq!(
            slack_status_text(&"é".repeat(200), "x").chars().count(),
            100
        );
    }
}
