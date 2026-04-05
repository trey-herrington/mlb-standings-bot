use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{Duration as ChronoDuration, NaiveDate};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info};

use crate::api::client::BallDontLieClient;
use crate::api::models::{Game, Team};
use crate::config::current_mlb_season;
use crate::standings::compute::{compute_standings, Standings};

/// How long cached standings remain fresh before an incremental refresh
/// is triggered on the next request. Default: 1 hour.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// Inner cache state, protected by a RwLock.
struct CacheInner {
    /// Cached teams (rarely changes, fetched once).
    teams: Vec<Team>,
    /// All cached games for the current season, keyed by game ID to avoid duplicates.
    games: HashMap<u64, Game>,
    /// The most recent game date we've seen (YYYY-MM-DD), used for incremental fetches.
    latest_game_date: Option<String>,
    /// The season these games belong to.
    season: u32,
    /// Pre-computed standings from the cached data.
    standings: Option<Standings>,
    /// When the cache was last refreshed.
    last_refresh: Option<std::time::Instant>,
}

/// Thread-safe, async-friendly cache for MLB standings data.
///
/// Stores teams and games in memory, supports incremental updates
/// (only fetching games newer than what's already cached), and
/// pre-computes standings so `/standings` responses are near-instant.
///
/// A `refresh_mutex` ensures that only one refresh runs at a time.
/// If the pre-warm task, a `/standings` command, and the cron scheduler
/// all try to refresh concurrently, only the first one does actual API
/// work; the others wait for it to finish and use its result.
pub struct StandingsCache {
    inner: RwLock<CacheInner>,
    /// Serializes refresh operations so only one runs at a time.
    refresh_mutex: Mutex<()>,
    api_client: Arc<BallDontLieClient>,
    season_override: Option<u32>,
}

impl StandingsCache {
    /// Create a new empty cache.
    pub fn new(api_client: Arc<BallDontLieClient>, season_override: Option<u32>) -> Self {
        let season = season_override.unwrap_or_else(current_mlb_season);

        Self {
            inner: RwLock::new(CacheInner {
                teams: Vec::new(),
                games: HashMap::new(),
                latest_game_date: None,
                season,
                standings: None,
                last_refresh: None,
            }),
            refresh_mutex: Mutex::new(()),
            api_client,
            season_override,
        }
    }

    /// Get standings, refreshing the cache incrementally if stale.
    ///
    /// - If the cache has never been populated, does a full fetch.
    /// - If the cache is older than the TTL, does an incremental fetch
    ///   (only games since the last known date).
    /// - If the cache is fresh, returns the pre-computed standings instantly.
    pub async fn get_standings(&self) -> Result<Standings> {
        // Fast path: check if cache is fresh under a read lock
        {
            let inner = self.inner.read().await;
            if let (Some(standings), Some(last_refresh)) = (&inner.standings, inner.last_refresh) {
                if last_refresh.elapsed() < CACHE_TTL {
                    debug!(
                        "Cache hit: standings are {:.0}s old (TTL: {}s)",
                        last_refresh.elapsed().as_secs_f64(),
                        CACHE_TTL.as_secs()
                    );
                    return Ok(standings.clone());
                }
            }
        }

        // Cache is stale or empty -- refresh
        self.refresh().await
    }

    /// Force a full or incremental refresh of the cache.
    ///
    /// Returns the newly computed standings. This is called by the daily
    /// scheduler and when the cache TTL expires.
    ///
    /// Only one refresh runs at a time. If another task is already refreshing,
    /// this call waits for it to finish and then returns the cached result
    /// rather than starting a second concurrent refresh.
    pub async fn refresh(&self) -> Result<Standings> {
        // Serialize refreshes: only one runs at a time
        let _refresh_guard = self.refresh_mutex.lock().await;

        // After acquiring the lock, check if someone else already refreshed
        // while we were waiting. If so, their result is fresh enough.
        {
            let inner = self.inner.read().await;
            if let (Some(standings), Some(last_refresh)) = (&inner.standings, inner.last_refresh) {
                if last_refresh.elapsed() < CACHE_TTL {
                    debug!(
                        "Cache was refreshed while waiting for lock ({:.0}s old), using cached result",
                        last_refresh.elapsed().as_secs_f64()
                    );
                    return Ok(standings.clone());
                }
            }
        }

        let season = self.season_override.unwrap_or_else(current_mlb_season);

        // Determine if this is a full or incremental fetch
        let (needs_teams, start_date, old_season) = {
            let inner = self.inner.read().await;
            let needs_teams = inner.teams.is_empty();
            let start_date = inner.latest_game_date.clone();
            (needs_teams, start_date, inner.season)
        };

        // If the season changed, do a full fetch
        let season_changed = season != old_season;

        // Fetch teams if we don't have them or season changed
        let teams = if needs_teams || season_changed {
            info!("Fetching teams from API");
            self.api_client.get_teams().await?
        } else {
            self.inner.read().await.teams.clone()
        };

        // Fetch games: incremental if we have a latest date and same season.
        // Offset start_date by +1 day so we don't re-fetch the full page of
        // games from the last known date (the HashMap deduplicates, but this
        // avoids a wasted API page when the last date had many games).
        let new_games = if let (Some(ref date), false) = (&start_date, season_changed) {
            let incremental_start = NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .ok()
                .map(|d| (d + ChronoDuration::days(1)).format("%Y-%m-%d").to_string());

            match incremental_start {
                Some(ref next_day) => {
                    info!("Incremental refresh: fetching games since {next_day}");
                    self.api_client
                        .get_games_since(season, Some(next_day))
                        .await?
                }
                None => {
                    // Couldn't parse the stored date; fall back to using it as-is
                    info!("Incremental refresh: fetching games since {date}");
                    self.api_client.get_games_since(season, Some(date)).await?
                }
            }
        } else {
            info!("Full refresh: fetching all games for season {season}");
            self.api_client.get_season_games(season).await?
        };

        // Update the cache under a write lock
        let standings = {
            let mut inner = self.inner.write().await;

            // Reset if season changed
            if season_changed {
                inner.games.clear();
                inner.latest_game_date = None;
                inner.season = season;
            }

            inner.teams = teams;

            // Merge new games (upsert by game ID to handle score updates
            // for games that were in-progress during the previous fetch)
            let mut latest_date: Option<NaiveDate> = inner
                .latest_game_date
                .as_ref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());

            for game in new_games {
                // Only advance latest_game_date for completed games so that
                // incremental refreshes start from the day after the last
                // *finished* game, not the last *scheduled* one. Without this,
                // the initial full fetch picks up future-dated scheduled games,
                // pushing the incremental start_date past today and causing
                // every subsequent refresh to return 0 results.
                if game.status == "STATUS_FINAL" {
                    if let Ok(game_date) = NaiveDate::parse_from_str(&game.date, "%Y-%m-%d") {
                        match &latest_date {
                            Some(current) if game_date > *current => {
                                latest_date = Some(game_date);
                            }
                            None => {
                                latest_date = Some(game_date);
                            }
                            _ => {}
                        }
                    }
                }
                inner.games.insert(game.id, game);
            }

            inner.latest_game_date = latest_date.map(|d| d.format("%Y-%m-%d").to_string());
            inner.last_refresh = Some(std::time::Instant::now());

            // Recompute standings from all cached games
            let all_games: Vec<Game> = inner.games.values().cloned().collect();
            let standings = compute_standings(&inner.teams, &all_games, inner.season);

            info!(
                "Cache refreshed: {} teams, {} games, latest date: {:?}",
                inner.teams.len(),
                inner.games.len(),
                inner.latest_game_date
            );

            inner.standings = Some(standings.clone());
            standings
        };

        Ok(standings)
    }

    /// Get cache stats for logging/debugging.
    pub async fn stats(&self) -> CacheStats {
        let inner = self.inner.read().await;
        CacheStats {
            team_count: inner.teams.len(),
            game_count: inner.games.len(),
            season: inner.season,
            latest_game_date: inner.latest_game_date.clone(),
            age_secs: inner.last_refresh.map(|t| t.elapsed().as_secs()),
        }
    }
}

/// Summary of cache state for logging.
#[derive(Debug)]
#[allow(dead_code)]
pub struct CacheStats {
    pub team_count: usize,
    pub game_count: usize,
    pub season: u32,
    pub latest_game_date: Option<String>,
    pub age_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::Team;

    // ── Helpers ─────────────────────────────────────────────────────

    fn mock_team(id: u64, name: &str, abbr: &str, league: &str, division: &str) -> Team {
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

    fn teams_json(teams: &[Team]) -> String {
        let data: Vec<String> = teams
            .iter()
            .map(|t| {
                format!(
                    r#"{{"id":{},"slug":null,"abbreviation":"{}","display_name":"Test {}","short_display_name":"{}","name":"{}","location":"Test City","league":"{}","division":"{}"}}"#,
                    t.id, t.abbreviation, t.name, t.name, t.name, t.league, t.division
                )
            })
            .collect();
        format!(r#"{{"data":[{}],"meta":{{}}}}"#, data.join(","))
    }

    fn games_json(games: &[serde_json::Value]) -> String {
        let data = serde_json::Value::Array(games.to_vec());
        format!(r#"{{"data":{},"meta":{{}}}}"#, data)
    }

    fn game_json(
        id: u64,
        home_team: &Team,
        away_team: &Team,
        home_runs: u32,
        away_runs: u32,
        date: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "date": date,
            "season": 2025,
            "status": "STATUS_FINAL",
            "season_type": "regular",
            "home_team": {
                "id": home_team.id,
                "slug": null,
                "abbreviation": home_team.abbreviation,
                "display_name": format!("Test {}", home_team.name),
                "short_display_name": home_team.name,
                "name": home_team.name,
                "location": "Test City",
                "league": home_team.league,
                "division": home_team.division,
            },
            "away_team": {
                "id": away_team.id,
                "slug": null,
                "abbreviation": away_team.abbreviation,
                "display_name": format!("Test {}", away_team.name),
                "short_display_name": away_team.name,
                "name": away_team.name,
                "location": "Test City",
                "league": away_team.league,
                "division": away_team.division,
            },
            "home_team_data": {
                "runs": home_runs,
                "hits": 10,
                "errors": 0
            },
            "away_team_data": {
                "runs": away_runs,
                "hits": 8,
                "errors": 1
            }
        })
    }

    fn make_cache(server_url: &str) -> StandingsCache {
        let client =
            BallDontLieClient::with_base_url("test-key".to_string(), server_url.to_string())
                .unwrap();
        StandingsCache::new(Arc::new(client), Some(2025))
    }

    // ── Cache creation ──────────────────────────────────────────────

    #[tokio::test]
    async fn new_cache_is_empty() {
        let client = BallDontLieClient::new("test".to_string()).unwrap();
        let cache = StandingsCache::new(Arc::new(client), Some(2025));
        let stats = cache.stats().await;
        assert_eq!(stats.team_count, 0);
        assert_eq!(stats.game_count, 0);
        assert!(stats.age_secs.is_none());
        assert!(stats.latest_game_date.is_none());
        assert_eq!(stats.season, 2025);
    }

    #[tokio::test]
    async fn new_cache_uses_season_override() {
        let client = BallDontLieClient::new("test".to_string()).unwrap();
        let cache = StandingsCache::new(Arc::new(client), Some(2020));
        let stats = cache.stats().await;
        assert_eq!(stats.season, 2020);
    }

    // ── Full refresh via mock server ────────────────────────────────

    #[tokio::test]
    async fn refresh_fetches_teams_and_games() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");
        let dodgers = mock_team(2, "Dodgers", "LAD", "National", "West");

        let teams_mock = server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees.clone(), dodgers.clone()]))
            .create_async()
            .await;

        let game = game_json(100, &yankees, &dodgers, 5, 3, "2025-06-15");
        let games_mock = server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[game]))
            .expect_at_least(1)
            .create_async()
            .await;

        let cache = make_cache(&server.url());
        let standings = cache.refresh().await.unwrap();

        // Yankees should be in AL East with 1 win
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();
        assert_eq!(al_east.teams.len(), 1);
        assert_eq!(al_east.teams[0].team.abbreviation, "NYY");
        assert_eq!(al_east.teams[0].wins, 1);

        // Dodgers should be in NL West with 1 loss
        let nl_west = standings
            .divisions
            .iter()
            .find(|d| d.name == "NL West")
            .unwrap();
        assert_eq!(nl_west.teams.len(), 1);
        assert_eq!(nl_west.teams[0].team.abbreviation, "LAD");
        assert_eq!(nl_west.teams[0].losses, 1);

        teams_mock.assert_async().await;
        games_mock.assert_async().await;

        let stats = cache.stats().await;
        assert_eq!(stats.team_count, 2);
        assert_eq!(stats.game_count, 1);
        assert_eq!(stats.latest_game_date, Some("2025-06-15".to_string()));
    }

    // ── get_standings TTL: fresh cache returns instantly ─────────────

    #[tokio::test]
    async fn get_standings_returns_cached_when_fresh() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");

        let teams_mock = server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees.clone()]))
            .expect(1)
            .create_async()
            .await;

        let games_mock = server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[]))
            .expect(4) // 4 date ranges on first full refresh
            .create_async()
            .await;

        let cache = make_cache(&server.url());

        // First call triggers refresh
        let _ = cache.get_standings().await.unwrap();

        // Second call should return cached (no new API calls)
        let _ = cache.get_standings().await.unwrap();

        teams_mock.assert_async().await;
        games_mock.assert_async().await;
    }

    // ── Game upsert: same ID updates scores ─────────────────────────

    #[tokio::test]
    async fn refresh_upserts_games_by_id() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");
        let redsox = mock_team(2, "Red Sox", "BOS", "American", "East");

        server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees.clone(), redsox.clone()]))
            .create_async()
            .await;

        // First refresh: game in-progress (no winner yet)
        let game_v1 = serde_json::json!({
            "id": 100,
            "date": "2025-06-15",
            "season": 2025,
            "status": "In Progress",
            "season_type": "regular",
            "home_team": {
                "id": 1, "slug": null, "abbreviation": "NYY",
                "display_name": "Test Yankees", "short_display_name": "Yankees",
                "name": "Yankees", "location": "Test City",
                "league": "American", "division": "East"
            },
            "away_team": {
                "id": 2, "slug": null, "abbreviation": "BOS",
                "display_name": "Test Red Sox", "short_display_name": "Red Sox",
                "name": "Red Sox", "location": "Test City",
                "league": "American", "division": "East"
            },
            "home_team_data": { "runs": 3, "hits": 5, "errors": 0 },
            "away_team_data": { "runs": 2, "hits": 4, "errors": 1 }
        });

        server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[game_v1]))
            .create_async()
            .await;

        let cache = make_cache(&server.url());
        let standings = cache.refresh().await.unwrap();

        // Game not final, so no wins/losses
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();
        let nyy = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "NYY")
            .unwrap();
        assert_eq!(nyy.wins, 0);
        assert_eq!(nyy.losses, 0);

        let stats = cache.stats().await;
        assert_eq!(stats.game_count, 1);

        // Second refresh: same game ID, now Final
        server.reset();

        let game_v2 = game_json(100, &yankees, &redsox, 5, 3, "2025-06-15");
        server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[game_v2]))
            .create_async()
            .await;

        // Manually expire the cache to force a refresh
        {
            let mut inner = cache.inner.write().await;
            inner.last_refresh =
                Some(std::time::Instant::now() - Duration::from_secs(CACHE_TTL.as_secs() + 1));
        }

        let standings = cache.refresh().await.unwrap();

        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();
        let nyy = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "NYY")
            .unwrap();
        assert_eq!(nyy.wins, 1);
        assert_eq!(nyy.losses, 0);

        // Still only 1 game (upserted, not duplicated)
        let stats = cache.stats().await;
        assert_eq!(stats.game_count, 1);
    }

    // ── Latest game date tracking ───────────────────────────────────

    #[tokio::test]
    async fn refresh_tracks_latest_game_date() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");
        let redsox = mock_team(2, "Red Sox", "BOS", "American", "East");

        server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees.clone(), redsox.clone()]))
            .create_async()
            .await;

        let games = vec![
            game_json(100, &yankees, &redsox, 5, 3, "2025-06-01"),
            game_json(101, &yankees, &redsox, 4, 2, "2025-08-15"),
            game_json(102, &yankees, &redsox, 7, 1, "2025-07-10"),
        ];

        server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&games))
            .create_async()
            .await;

        let cache = make_cache(&server.url());
        cache.refresh().await.unwrap();

        let stats = cache.stats().await;
        assert_eq!(
            stats.latest_game_date,
            Some("2025-08-15".to_string()),
            "Should track the latest date across all games"
        );
    }

    // ── Refresh mutex: concurrent refreshes coalesce ────────────────

    #[tokio::test]
    async fn concurrent_refreshes_coalesce() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");

        let teams_mock = server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees.clone()]))
            .expect(1)
            .create_async()
            .await;

        let games_mock = server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[]))
            .expect(4) // 4 date ranges, only from the first refresh
            .create_async()
            .await;

        let cache = Arc::new(make_cache(&server.url()));

        // Spawn 3 concurrent refreshes
        let mut handles = Vec::new();
        for _ in 0..3 {
            let cache = cache.clone();
            handles.push(tokio::spawn(async move { cache.refresh().await }));
        }

        // All should succeed
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
        }

        teams_mock.assert_async().await;
        games_mock.assert_async().await;
    }

    // ── Incremental refresh uses +1 day offset ──────────────────────

    #[tokio::test]
    async fn incremental_refresh_offsets_start_date() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");
        let redsox = mock_team(2, "Red Sox", "BOS", "American", "East");

        // First full refresh
        server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees.clone(), redsox.clone()]))
            .create_async()
            .await;

        let game = game_json(100, &yankees, &redsox, 5, 3, "2025-06-15");
        server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[game]))
            .create_async()
            .await;

        let cache = make_cache(&server.url());
        cache.refresh().await.unwrap();

        let stats = cache.stats().await;
        assert_eq!(stats.latest_game_date, Some("2025-06-15".to_string()));

        // Reset the server for incremental refresh
        server.reset();

        // The incremental refresh should use start_date = 2025-06-16 (latest + 1)
        let incremental_games_mock = server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "start_date".to_string(),
                "2025-06-16".to_string(),
            )]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[]))
            .create_async()
            .await;

        // Expire the cache to trigger refresh
        {
            let mut inner = cache.inner.write().await;
            inner.last_refresh =
                Some(std::time::Instant::now() - Duration::from_secs(CACHE_TTL.as_secs() + 1));
        }

        cache.refresh().await.unwrap();

        incremental_games_mock.assert_async().await;
    }

    // ── Scheduled games don't advance latest_game_date ────────────

    #[tokio::test]
    async fn scheduled_games_do_not_advance_latest_game_date() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");
        let redsox = mock_team(2, "Red Sox", "BOS", "American", "East");

        server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees.clone(), redsox.clone()]))
            .create_async()
            .await;

        let final_game = game_json(100, &yankees, &redsox, 5, 3, "2025-06-15");
        let scheduled_game = serde_json::json!({
            "id": 200,
            "date": "2025-09-20",
            "season": 2025,
            "status": "Scheduled",
            "season_type": "regular",
            "home_team": {
                "id": 1, "slug": null, "abbreviation": "NYY",
                "display_name": "Test Yankees", "short_display_name": "Yankees",
                "name": "Yankees", "location": "Test City",
                "league": "American", "division": "East"
            },
            "away_team": {
                "id": 2, "slug": null, "abbreviation": "BOS",
                "display_name": "Test Red Sox", "short_display_name": "Red Sox",
                "name": "Red Sox", "location": "Test City",
                "league": "American", "division": "East"
            },
            "home_team_data": null,
            "away_team_data": null
        });

        server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[final_game, scheduled_game]))
            .create_async()
            .await;

        let cache = make_cache(&server.url());
        cache.refresh().await.unwrap();

        let stats = cache.stats().await;
        assert_eq!(
            stats.latest_game_date,
            Some("2025-06-15".to_string()),
            "Scheduled games must not advance latest_game_date"
        );
        assert_eq!(stats.game_count, 2);
    }

    // ── Cache stats ─────────────────────────────────────────────────

    #[tokio::test]
    async fn stats_reports_age_after_refresh() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");

        server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees]))
            .create_async()
            .await;

        server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(games_json(&[]))
            .create_async()
            .await;

        let cache = make_cache(&server.url());
        cache.refresh().await.unwrap();

        let stats = cache.stats().await;
        assert!(stats.age_secs.is_some());
        assert!(stats.age_secs.unwrap() < 5, "Cache should be very fresh");
    }

    // ── API error propagation ───────────────────────────────────────

    #[tokio::test]
    async fn refresh_propagates_teams_api_error() {
        let mut server = mockito::Server::new_async().await;

        server
            .mock("GET", "/teams")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let cache = make_cache(&server.url());
        let result = cache.refresh().await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("500"),
            "Expected error to mention 500, got: {err}"
        );
    }

    #[tokio::test]
    async fn refresh_propagates_games_api_error() {
        let mut server = mockito::Server::new_async().await;
        let yankees = mock_team(1, "Yankees", "NYY", "American", "East");

        server
            .mock("GET", "/teams")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(teams_json(&[yankees]))
            .create_async()
            .await;

        server
            .mock("GET", "/games")
            .match_query(mockito::Matcher::Any)
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let cache = make_cache(&server.url());
        let result = cache.refresh().await;

        assert!(result.is_err());
    }
}
