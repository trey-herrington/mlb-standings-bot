use anyhow::{Context, Result};
use chrono::{Datelike, Utc};
use serenity::all::ChannelId;

/// Bot configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// Discord bot token.
    pub discord_token: String,
    /// balldontlie API key.
    pub balldontlie_api_key: String,
    /// Discord channel ID for daily standings posts.
    pub channel_id: ChannelId,
    /// Cron schedule expression for daily posting.
    pub cron_schedule: String,
    /// Optional: override the MLB season year.
    pub mlb_season: Option<u32>,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        let discord_token =
            std::env::var("DISCORD_TOKEN").context("DISCORD_TOKEN env var is required")?;

        let balldontlie_api_key = std::env::var("BALLDONTLIE_API_KEY")
            .context("BALLDONTLIE_API_KEY env var is required")?;

        let channel_id_raw =
            std::env::var("CHANNEL_ID").context("CHANNEL_ID env var is required")?;
        let channel_id = ChannelId::new(
            channel_id_raw
                .parse::<u64>()
                .context("CHANNEL_ID must be a valid u64")?,
        );

        let cron_schedule =
            std::env::var("CRON_SCHEDULE").unwrap_or_else(|_| "0 0 15 * * *".to_string());

        let mlb_season = std::env::var("MLB_SEASON")
            .ok()
            .and_then(|s| s.parse::<u32>().ok());

        Ok(Self {
            discord_token,
            balldontlie_api_key,
            channel_id,
            cron_schedule,
            mlb_season,
        })
    }
}

/// Determine the current MLB season year.
///
/// The MLB season runs from March to October within a single calendar year.
/// If we're in March-December, the season year equals the current year.
/// If we're in January-February, it equals the previous year (tail end
/// of postseason / offseason data from the prior year).
pub fn current_mlb_season() -> u32 {
    let now = Utc::now();
    let year = now.year() as u32;
    let month = now.month();

    if month >= 3 {
        year
    } else {
        year - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_mlb_season_returns_a_plausible_value() {
        let season = current_mlb_season();
        assert!(
            season >= 2020 && season <= 2040,
            "Season {season} seems implausible"
        );
    }
}
