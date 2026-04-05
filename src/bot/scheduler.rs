use std::sync::Arc;

use anyhow::{Context as _, Result};
use serenity::all::Http;
use serenity::builder::CreateMessage;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

use crate::config::Config;
use crate::standings::cache::StandingsCache;
use crate::standings::format::build_standings_embeds;

/// Start the daily standings scheduler.
///
/// Spawns a background task that refreshes the shared cache and posts
/// standings to the configured Discord channel on the configured cron schedule.
pub async fn start_scheduler(
    http: Arc<Http>,
    config: Config,
    cache: Arc<StandingsCache>,
) -> Result<JobScheduler> {
    let scheduler = JobScheduler::new()
        .await
        .context("Failed to create job scheduler")?;

    let cron_expr = config.cron_schedule.clone();
    info!("Scheduling daily standings post with cron: {cron_expr}");

    let job = Job::new_async(cron_expr.as_str(), move |_uuid, _lock| {
        let http = http.clone();
        let config = config.clone();
        let cache = cache.clone();

        Box::pin(async move {
            info!("Cron job triggered: refreshing cache and posting daily standings");
            if let Err(e) = post_standings_to_channel(&http, &config, &cache).await {
                error!("Failed to post daily standings: {e:#}");
            }
        })
    })
    .context("Failed to create cron job")?;

    scheduler
        .add(job)
        .await
        .context("Failed to add job to scheduler")?;

    scheduler
        .start()
        .await
        .context("Failed to start scheduler")?;

    info!("Scheduler started successfully");
    Ok(scheduler)
}

/// Build the Discord message containing standings embeds.
///
/// Extracted so it can be tested independently of the Discord HTTP layer.
fn build_standings_message(standings: &crate::standings::compute::Standings) -> CreateMessage {
    let embeds = build_standings_embeds(standings);
    CreateMessage::new().embeds(embeds)
}

/// Refresh the cache and post standings to the configured Discord channel.
async fn post_standings_to_channel(
    http: &Http,
    config: &Config,
    cache: &StandingsCache,
) -> Result<()> {
    // Force a refresh so the daily post always has the latest data
    let standings = cache.refresh().await?;
    let message = build_standings_message(&standings);

    config
        .channel_id
        .send_message(http, message)
        .await
        .context("Failed to send standings message to channel")?;

    let stats = cache.stats().await;
    info!(
        "Posted standings to channel {} (cache: {} games, latest: {:?})",
        config.channel_id, stats.game_count, stats.latest_game_date
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::Team;
    use crate::standings::compute::{DivisionStandings, Standings, TeamRecord, WildCardStandings};

    // ── Helpers ─────────────────────────────────────────────────────

    fn make_team(id: u64, name: &str, abbr: &str, league: &str, division: &str) -> Team {
        Team {
            id,
            slug: None,
            abbreviation: abbr.to_string(),
            display_name: Some(format!("Test {name}")),
            short_display_name: Some(name.to_string()),
            name: name.to_string(),
            location: Some("Test City".to_string()),
            league: league.to_string(),
            division: division.to_string(),
        }
    }

    fn make_record(team: Team, wins: u32, losses: u32) -> TeamRecord {
        let total = wins + losses;
        let win_pct = if total > 0 {
            wins as f64 / total as f64
        } else {
            0.0
        };
        TeamRecord {
            team,
            wins,
            losses,
            win_pct,
            games_behind: 0.0,
        }
    }

    fn make_standings() -> Standings {
        let nyy = make_record(make_team(1, "Yankees", "NYY", "American", "East"), 50, 20);
        let lad = make_record(make_team(2, "Dodgers", "LAD", "National", "West"), 45, 25);

        Standings {
            divisions: vec![
                DivisionStandings {
                    name: "AL East".to_string(),
                    league: "American".to_string(),
                    teams: vec![nyy],
                },
                DivisionStandings {
                    name: "AL Central".to_string(),
                    league: "American".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "AL West".to_string(),
                    league: "American".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "NL East".to_string(),
                    league: "National".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "NL Central".to_string(),
                    league: "National".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "NL West".to_string(),
                    league: "National".to_string(),
                    teams: vec![lad],
                },
            ],
            wild_cards: vec![
                WildCardStandings {
                    name: "AL Wild Card".to_string(),
                    league: "American".to_string(),
                    teams: vec![],
                },
                WildCardStandings {
                    name: "NL Wild Card".to_string(),
                    league: "National".to_string(),
                    teams: vec![],
                },
            ],
            season: 2025,
        }
    }

    // ── build_standings_message ──────────────────────────────────────

    #[test]
    fn message_contains_eight_embeds() {
        let standings = make_standings();
        let message = build_standings_message(&standings);

        let json = serde_json::to_value(&message).expect("message should serialize");
        let embeds = json["embeds"]
            .as_array()
            .expect("embeds should be an array");

        assert_eq!(
            embeds.len(),
            8,
            "Scheduled message must contain 8 embeds (6 divisions + 2 wild card), got {}",
            embeds.len()
        );
    }

    #[test]
    fn message_embeds_contain_division_titles() {
        let standings = make_standings();
        let message = build_standings_message(&standings);

        let json = serde_json::to_value(&message).expect("message should serialize");
        let embeds = json["embeds"].as_array().unwrap();

        let titles: Vec<&str> = embeds.iter().filter_map(|e| e["title"].as_str()).collect();

        assert!(
            titles.iter().any(|t| t.contains("AL East")),
            "Missing AL East. Titles: {titles:?}"
        );
        assert!(
            titles.iter().any(|t| t.contains("NL West")),
            "Missing NL West. Titles: {titles:?}"
        );
        assert!(
            titles.iter().any(|t| t.contains("AL Wild Card")),
            "Missing AL Wild Card. Titles: {titles:?}"
        );
        assert!(
            titles.iter().any(|t| t.contains("NL Wild Card")),
            "Missing NL Wild Card. Titles: {titles:?}"
        );
    }

    #[test]
    fn message_embeds_contain_team_data() {
        let standings = make_standings();
        let message = build_standings_message(&standings);
        let json = serde_json::to_value(&message).expect("message should serialize");
        let embeds = json["embeds"].as_array().unwrap();

        let descriptions: Vec<&str> = embeds
            .iter()
            .filter_map(|e| e["description"].as_str())
            .collect();

        assert!(
            descriptions.iter().any(|d| d.contains("NYY")),
            "Should contain NYY team data"
        );
        assert!(
            descriptions.iter().any(|d| d.contains("LAD")),
            "Should contain LAD team data"
        );
    }

    #[test]
    fn message_with_empty_standings_still_has_eight_embeds() {
        let standings = Standings {
            divisions: vec![
                DivisionStandings {
                    name: "AL East".to_string(),
                    league: "American".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "AL Central".to_string(),
                    league: "American".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "AL West".to_string(),
                    league: "American".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "NL East".to_string(),
                    league: "National".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "NL Central".to_string(),
                    league: "National".to_string(),
                    teams: vec![],
                },
                DivisionStandings {
                    name: "NL West".to_string(),
                    league: "National".to_string(),
                    teams: vec![],
                },
            ],
            wild_cards: vec![
                WildCardStandings {
                    name: "AL Wild Card".to_string(),
                    league: "American".to_string(),
                    teams: vec![],
                },
                WildCardStandings {
                    name: "NL Wild Card".to_string(),
                    league: "National".to_string(),
                    teams: vec![],
                },
            ],
            season: 2025,
        };

        let message = build_standings_message(&standings);
        let json = serde_json::to_value(&message).expect("message should serialize");
        let embeds = json["embeds"]
            .as_array()
            .expect("embeds should be an array");

        assert_eq!(
            embeds.len(),
            8,
            "Even with no teams, message must have all 8 embeds"
        );
    }

    /// Regression test: verify we use .embeds() (plural) not .embed() in a loop.
    /// serenity's embed() replaces all existing embeds; embeds() sets them all at once.
    #[test]
    fn embeds_plural_preserves_all_eight() {
        let standings = make_standings();
        let embeds = build_standings_embeds(&standings);
        assert_eq!(
            embeds.len(),
            8,
            "build_standings_embeds should return 8 embeds"
        );

        // The correct approach: .embeds() sets all at once
        let fixed_message = build_standings_message(&standings);
        let fixed_json = serde_json::to_value(&fixed_message).unwrap();
        let fixed_embeds = fixed_json["embeds"].as_array().unwrap();
        assert_eq!(
            fixed_embeds.len(),
            8,
            "The .embeds() call must preserve all 8 embeds"
        );
    }
}
