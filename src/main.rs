use serde_json::Value;
use std::fs;
use std::time::Duration;
use tokio::time;
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use reqwest::Client;
use url::Url;
use std::env;

#[derive(Debug)]
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let config_file = if let Some(index) = args.iter().position(|arg| arg == "-c") {
        if index + 1 < args.len() {
            &args[index + 1]
        } else {
            eprintln!("Error: missing argument for -c option");
            return Err("missing argument for -c option".into());
        }
    } else {
        "config.json"
    };

    println!("Using config file: {}", config_file);

    let config = load_config(config_file).await?;
    let mut discord = DiscordIpcClient::new(&config.discord_client_id)?;
    discord.connect()?;

    println!("Audiobookshelf Discord RPC Connected!");

    let client = Client::new();
    let mut last_known_time: Option<f64> = None;
    let mut is_paused = false;
    let mut current_book: Option<Book> = None;

    loop {
        match set_activity(&config, &client, &mut discord, &mut last_known_time, &mut is_paused, &mut current_book).await {
            Ok(_) => (),
            Err(e) => eprintln!("Error setting activity: {}", e),
        }
        time::sleep(Duration::from_secs(15)).await;
    }
}

async fn load_config(config_file: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let config_str = fs::read_to_string(config_file)?;
    let config: Value = serde_json::from_str(&config_str)?;
    
    Ok(Config {
        discord_client_id: config["discordClientId"].as_str().unwrap().to_string(),
        audiobookshelf_url: config["audiobookshelfUrl"].as_str().unwrap().to_string(),
        audiobookshelf_token: config["audiobookshelfToken"].as_str().unwrap().to_string(),
        audiobookshelf_user_id: config["audiobookshelfUserId"].as_str().unwrap().to_string(),
    })
}

async fn set_activity(
    config: &Config,
    client: &Client,
    discord: &mut DiscordIpcClient,
    last_known_time: &mut Option<f64>,
    is_paused: &mut bool,
    current_book: &mut Option<Book>,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/api/me/listening-sessions?itemsPerPage=1", config.audiobookshelf_url);
    let resp = client.get(&url)
        .header("Authorization", format!("Bearer {}", config.audiobookshelf_token))
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

    if last_known_time.is_none() {
        *last_known_time = Some(current_time);
    } else if current_time == last_known_time.unwrap() {
        if !*is_paused {
            println!("Book paused. Clearing activity.");
            discord.clear_activity()?;
            *is_paused = true;
        }
        return Ok(());
    } else {
        *is_paused = false;
    }

    *last_known_time = Some(current_time);

    if !*is_paused {
        let cover_url = get_cover_path(config, book_name, author).await?;
        let state = format!("{} / {}", format_time(current_time), total_time);
        let activity = activity::Activity::new()
            .details(book_name)
            .state(&state)
            .assets(activity::Assets::new()
                .large_image(&cover_url)
                .large_text(book_name))
            .activity_type(activity::ActivityType::Listening);
        
        discord.set_activity(activity)?;
    }

    Ok(())
}

fn format_time(seconds: f64) -> String {
    let hours = (seconds / 3600.0).floor();
    let minutes = ((seconds % 3600.0) / 60.0).floor();
    let remaining_seconds = (seconds % 60.0).floor();
    format!("{:02}:{:02}:{:02}", hours, minutes, remaining_seconds)
}

async fn get_cover_path(config: &Config, title: &str, author: &str) -> Result<String, Box<dyn std::error::Error>> {
    let url = Url::parse_with_params(
        &format!("{}/api/search/covers", config.audiobookshelf_url),
        &[("title", title), ("author", author), ("provider", "audible")]
    )?;

    let resp = Client::new().get(url)
        .header("Authorization", format!("Bearer {}", config.audiobookshelf_token))
        .send()
        .await?
        .json::<Value>()
        .await?;

    let results = resp["results"].as_array().ok_or("No cover results found")?;
    if results.is_empty() {
        return Err("No cover URL found".into());
    }
    
    results[0].as_str().ok_or("Invalid cover URL").map(|s| s.to_string()).map_err(|e| e.into())
}