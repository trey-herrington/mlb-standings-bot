use serde::Deserialize;

/// Wrapper for paginated API responses from balldontlie.
#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    pub data: Vec<T>,
    pub meta: Option<Meta>,
}

/// Pagination metadata using cursor-based pagination.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Meta {
    pub next_cursor: Option<u64>,
    pub per_page: Option<u32>,
}

/// An MLB team as returned by the balldontlie MLB API.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Team {
    pub id: u64,
    pub slug: Option<String>,
    pub abbreviation: String,
    pub display_name: Option<String>,
    pub short_display_name: Option<String>,
    pub name: String,
    pub location: Option<String>,
    pub league: String,
    pub division: String,
}

/// Score data for a team in a game.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TeamGameData {
    pub runs: Option<u32>,
    pub hits: Option<u32>,
    pub errors: Option<u32>,
}

/// An MLB game as returned by the balldontlie MLB API.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Game {
    pub id: u64,
    pub date: String,
    pub season: u32,
    pub status: String,
    pub season_type: Option<String>,
    pub home_team: Team,
    pub away_team: Team,
    pub home_team_data: Option<TeamGameData>,
    pub away_team_data: Option<TeamGameData>,
}
