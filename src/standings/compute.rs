use std::collections::HashMap;

use crate::api::models::{Game, Team};

/// A team's win-loss record for a season.
#[derive(Debug, Clone)]
pub struct TeamRecord {
    pub team: Team,
    pub wins: u32,
    pub losses: u32,
    pub win_pct: f64,
    /// Games behind the division leader. 0.0 for the leader.
    pub games_behind: f64,
}

/// A single division's standings.
#[derive(Debug, Clone)]
pub struct DivisionStandings {
    /// Division label, e.g. "AL East".
    pub name: String,
    /// League: "American" or "National".
    pub league: String,
    /// Teams sorted by win percentage descending with GB calculated.
    pub teams: Vec<TeamRecord>,
}

/// Wild card standings for one league.
#[derive(Debug, Clone)]
pub struct WildCardStandings {
    /// League label, e.g. "AL Wild Card" or "NL Wild Card".
    pub name: String,
    /// League: "American" or "National".
    pub league: String,
    /// Non-division-winners sorted by record, with GB from last WC spot.
    pub teams: Vec<TeamRecord>,
}

/// Complete MLB standings.
#[derive(Debug, Clone)]
pub struct Standings {
    /// 6 division standings (AL East, AL Central, AL West, NL East, NL Central, NL West).
    pub divisions: Vec<DivisionStandings>,
    /// 2 wild card races (AL Wild Card, NL Wild Card).
    pub wild_cards: Vec<WildCardStandings>,
    pub season: u32,
}

/// The 6 MLB divisions in display order.
const DIVISIONS: [(&str, &str); 6] = [
    ("American", "East"),
    ("American", "Central"),
    ("American", "West"),
    ("National", "East"),
    ("National", "Central"),
    ("National", "West"),
];

/// Number of wild card spots per league (current MLB format).
const WILD_CARD_SPOTS: usize = 3;

/// Calculate games behind: ((leader_wins - team_wins) + (team_losses - leader_losses)) / 2.0
fn games_behind(leader: &TeamRecord, team: &TeamRecord) -> f64 {
    let diff =
        (leader.wins as f64 - team.wins as f64) + (team.losses as f64 - leader.losses as f64);
    diff / 2.0
}

/// Sort function: win percentage descending, then wins descending as tiebreaker.
fn sort_by_record(a: &TeamRecord, b: &TeamRecord) -> std::cmp::Ordering {
    b.win_pct
        .partial_cmp(&a.win_pct)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(b.wins.cmp(&a.wins))
}

/// Compute standings from a list of teams and games.
///
/// Only games with status "STATUS_FINAL" are counted. Scores are extracted
/// from `home_team_data.runs` and `away_team_data.runs`. Results are split
/// into 6 divisions with GB calculated, plus 2 wild card races.
pub fn compute_standings(teams: &[Team], games: &[Game], season: u32) -> Standings {
    // Initialize records for all teams
    let mut records: HashMap<u64, TeamRecord> = teams
        .iter()
        .map(|team| {
            (
                team.id,
                TeamRecord {
                    team: team.clone(),
                    wins: 0,
                    losses: 0,
                    win_pct: 0.0,
                    games_behind: 0.0,
                },
            )
        })
        .collect();

    // Tally wins and losses from completed games
    for game in games {
        if game.status != "STATUS_FINAL" {
            continue;
        }

        let (home_runs, away_runs) = match (&game.home_team_data, &game.away_team_data) {
            (Some(home_data), Some(away_data)) => match (home_data.runs, away_data.runs) {
                (Some(h), Some(a)) => (h, a),
                _ => continue,
            },
            _ => continue,
        };

        if home_runs > away_runs {
            // Home team won
            if let Some(record) = records.get_mut(&game.home_team.id) {
                record.wins += 1;
            }
            if let Some(record) = records.get_mut(&game.away_team.id) {
                record.losses += 1;
            }
        } else if away_runs > home_runs {
            // Away team won
            if let Some(record) = records.get_mut(&game.away_team.id) {
                record.wins += 1;
            }
            if let Some(record) = records.get_mut(&game.home_team.id) {
                record.losses += 1;
            }
        }
        // Ties are extremely rare in MLB (only suspended games), skip them
    }

    // Calculate win percentages
    for record in records.values_mut() {
        let total = record.wins + record.losses;
        record.win_pct = if total > 0 {
            record.wins as f64 / total as f64
        } else {
            0.0
        };
    }

    // Build division standings
    let mut divisions = Vec::with_capacity(6);
    let mut division_winners: HashMap<String, TeamRecord> = HashMap::new();

    for (league, division) in &DIVISIONS {
        let mut div_teams: Vec<TeamRecord> = records
            .values()
            .filter(|r| r.team.league == *league && r.team.division == *division)
            .cloned()
            .collect();

        div_teams.sort_by(sort_by_record);

        // Calculate GB relative to division leader
        if let Some(leader) = div_teams.first().cloned() {
            for team in &mut div_teams {
                team.games_behind = games_behind(&leader, team);
            }
            division_winners.insert(league.to_string(), leader);
        }

        let league_abbr = if *league == "American" { "AL" } else { "NL" };
        divisions.push(DivisionStandings {
            name: format!("{league_abbr} {division}"),
            league: league.to_string(),
            teams: div_teams,
        });
    }

    // Build wild card standings per league
    let mut wild_cards = Vec::with_capacity(2);

    for league in &["American", "National"] {
        let league_abbr = if *league == "American" { "AL" } else { "NL" };

        // Collect division winners for this league
        let winner_ids: Vec<u64> = divisions
            .iter()
            .filter(|d| d.league == *league)
            .filter_map(|d| d.teams.first())
            .map(|r| r.team.id)
            .collect();

        // All non-division-winners in this league
        let mut wc_teams: Vec<TeamRecord> = records
            .values()
            .filter(|r| r.team.league == *league && !winner_ids.contains(&r.team.id))
            .cloned()
            .collect();

        wc_teams.sort_by(sort_by_record);

        // Calculate GB relative to the last wild card spot
        if wc_teams.len() > WILD_CARD_SPOTS {
            let wc_cutoff = wc_teams[WILD_CARD_SPOTS - 1].clone();
            for team in &mut wc_teams {
                team.games_behind = games_behind(&wc_cutoff, team);
            }
        } else if !wc_teams.is_empty() {
            // Fewer teams than spots: everyone is "in", GB = 0 relative to leader
            let leader = wc_teams[0].clone();
            for team in &mut wc_teams {
                team.games_behind = games_behind(&leader, team);
            }
        }

        wild_cards.push(WildCardStandings {
            name: format!("{league_abbr} Wild Card"),
            league: league.to_string(),
            teams: wc_teams,
        });
    }

    Standings {
        divisions,
        wild_cards,
        season,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ─────────────────────────────────────────────────────

    fn make_team(id: u64, name: &str, abbreviation: &str, league: &str, division: &str) -> Team {
        Team {
            id,
            slug: None,
            abbreviation: abbreviation.to_string(),
            display_name: Some(format!("Test {name}")),
            short_display_name: Some(name.to_string()),
            name: name.to_string(),
            location: Some("Test City".to_string()),
            league: league.to_string(),
            division: division.to_string(),
        }
    }

    fn make_game(
        id: u64,
        home: &Team,
        away: &Team,
        home_runs: Option<u32>,
        away_runs: Option<u32>,
        status: &str,
    ) -> Game {
        use crate::api::models::TeamGameData;
        Game {
            id,
            date: "2025-06-15".to_string(),
            season: 2025,
            status: status.to_string(),
            season_type: Some("regular".to_string()),
            home_team: home.clone(),
            away_team: away.clone(),
            home_team_data: Some(TeamGameData {
                runs: home_runs,
                hits: Some(10),
                errors: Some(0),
            }),
            away_team_data: Some(TeamGameData {
                runs: away_runs,
                hits: Some(8),
                errors: Some(1),
            }),
        }
    }

    fn al_east_team(id: u64, name: &str, abbr: &str) -> Team {
        make_team(id, name, abbr, "American", "East")
    }

    fn nl_east_team(id: u64, name: &str, abbr: &str) -> Team {
        make_team(id, name, abbr, "National", "East")
    }

    fn al_west_team(id: u64, name: &str, abbr: &str) -> Team {
        make_team(id, name, abbr, "American", "West")
    }

    // ── compute_standings: basic tallying ────────────────────────────

    #[test]
    fn empty_games_produces_zero_records() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let standings = compute_standings(&[yankees, redsox], &[], 2025);

        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();
        assert_eq!(al_east.teams.len(), 2);
        assert_eq!(al_east.teams[0].wins, 0);
        assert_eq!(al_east.teams[0].losses, 0);
        assert_eq!(al_east.teams[0].win_pct, 0.0);
    }

    #[test]
    fn home_win_tallied_correctly() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let game = make_game(100, &yankees, &redsox, Some(5), Some(3), "STATUS_FINAL");
        let standings = compute_standings(&[yankees, redsox], &[game], 2025);

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
        let bos = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "BOS")
            .unwrap();

        assert_eq!(nyy.wins, 1);
        assert_eq!(nyy.losses, 0);
        assert_eq!(bos.wins, 0);
        assert_eq!(bos.losses, 1);
    }

    #[test]
    fn away_win_tallied_correctly() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let game = make_game(100, &yankees, &redsox, Some(2), Some(7), "STATUS_FINAL");
        let standings = compute_standings(&[yankees, redsox], &[game], 2025);

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
        let bos = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "BOS")
            .unwrap();

        assert_eq!(nyy.wins, 0);
        assert_eq!(nyy.losses, 1);
        assert_eq!(bos.wins, 1);
        assert_eq!(bos.losses, 0);
    }

    #[test]
    fn multiple_games_accumulate() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let games = vec![
            make_game(100, &yankees, &redsox, Some(5), Some(3), "STATUS_FINAL"),
            make_game(101, &yankees, &redsox, Some(2), Some(4), "STATUS_FINAL"),
            make_game(102, &redsox, &yankees, Some(1), Some(6), "STATUS_FINAL"),
        ];

        let standings = compute_standings(&[yankees, redsox], &games, 2025);
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
        let bos = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "BOS")
            .unwrap();

        // NYY: won game 100 (home), lost game 101 (home), won game 102 (away) = 2-1
        assert_eq!(nyy.wins, 2);
        assert_eq!(nyy.losses, 1);
        // BOS: lost game 100 (away), won game 101 (away), lost game 102 (home) = 1-2
        assert_eq!(bos.wins, 1);
        assert_eq!(bos.losses, 2);
    }

    // ── compute_standings: filtering ────────────────────────────────

    #[test]
    fn non_final_games_are_ignored() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let games = vec![
            make_game(100, &yankees, &redsox, Some(3), Some(2), "In Progress"),
            make_game(101, &yankees, &redsox, None, None, "Scheduled"),
        ];

        let standings = compute_standings(&[yankees, redsox], &games, 2025);
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
    }

    #[test]
    fn games_with_missing_team_data_are_ignored() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let mut game = make_game(100, &yankees, &redsox, Some(5), Some(3), "STATUS_FINAL");
        game.home_team_data = None;

        let standings = compute_standings(&[yankees, redsox], &[game], 2025);
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
    }

    #[test]
    fn games_with_missing_runs_are_ignored() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let mut game = make_game(100, &yankees, &redsox, None, Some(3), "STATUS_FINAL");
        // home_team_data exists but runs is None
        game.home_team_data = Some(crate::api::models::TeamGameData {
            runs: None,
            hits: Some(5),
            errors: Some(0),
        });

        let standings = compute_standings(&[yankees, redsox], &[game], 2025);
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
    }

    #[test]
    fn tied_scores_are_ignored() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let game = make_game(100, &yankees, &redsox, Some(3), Some(3), "STATUS_FINAL");
        let standings = compute_standings(&[yankees, redsox], &[game], 2025);

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
        let bos = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "BOS")
            .unwrap();

        assert_eq!(nyy.wins, 0);
        assert_eq!(nyy.losses, 0);
        assert_eq!(bos.wins, 0);
        assert_eq!(bos.losses, 0);
    }

    #[test]
    fn games_for_unknown_teams_are_silently_skipped() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let unknown = al_east_team(999, "Unknowns", "UNK");

        let game = make_game(100, &yankees, &unknown, Some(5), Some(3), "STATUS_FINAL");
        let standings = compute_standings(&[yankees], &[game], 2025);

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
    }

    // ── compute_standings: win percentage ────────────────────────────

    #[test]
    fn win_percentage_calculated_correctly() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let games = vec![
            make_game(100, &yankees, &redsox, Some(5), Some(3), "STATUS_FINAL"),
            make_game(101, &yankees, &redsox, Some(4), Some(2), "STATUS_FINAL"),
            make_game(102, &redsox, &yankees, Some(7), Some(1), "STATUS_FINAL"),
        ];

        let standings = compute_standings(&[yankees, redsox], &games, 2025);
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

        // NYY: 2 wins, 1 loss = 0.667
        assert!((nyy.win_pct - 2.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn zero_games_produces_zero_win_pct() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let standings = compute_standings(&[yankees], &[], 2025);
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();
        assert_eq!(al_east.teams[0].win_pct, 0.0);
    }

    #[test]
    fn perfect_record_is_one_point_zero() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let redsox = al_east_team(2, "Red Sox", "BOS");

        let games = vec![
            make_game(100, &yankees, &redsox, Some(5), Some(3), "STATUS_FINAL"),
            make_game(101, &yankees, &redsox, Some(4), Some(2), "STATUS_FINAL"),
        ];

        let standings = compute_standings(&[yankees, redsox], &games, 2025);
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
        assert!((nyy.win_pct - 1.0).abs() < f64::EPSILON);
    }

    // ── compute_standings: division splitting ───────────────────────

    #[test]
    fn teams_split_into_correct_divisions() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let astros = al_west_team(2, "Astros", "HOU");
        let mets = nl_east_team(3, "Mets", "NYM");

        let standings = compute_standings(&[yankees, astros, mets], &[], 2025);

        assert_eq!(standings.divisions.len(), 6);

        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();
        let al_west = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL West")
            .unwrap();
        let nl_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "NL East")
            .unwrap();

        assert_eq!(al_east.teams.len(), 1);
        assert_eq!(al_east.teams[0].team.abbreviation, "NYY");
        assert_eq!(al_west.teams.len(), 1);
        assert_eq!(al_west.teams[0].team.abbreviation, "HOU");
        assert_eq!(nl_east.teams.len(), 1);
        assert_eq!(nl_east.teams[0].team.abbreviation, "NYM");
    }

    #[test]
    fn cross_division_game_tallied_for_both_teams() {
        let yankees = al_east_team(1, "Yankees", "NYY");
        let astros = al_west_team(2, "Astros", "HOU");

        let game = make_game(100, &yankees, &astros, Some(5), Some(3), "STATUS_FINAL");
        let standings = compute_standings(&[yankees, astros], &[game], 2025);

        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();
        let al_west = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL West")
            .unwrap();
        let nyy = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "NYY")
            .unwrap();
        let hou = al_west
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "HOU")
            .unwrap();

        assert_eq!(nyy.wins, 1);
        assert_eq!(nyy.losses, 0);
        assert_eq!(hou.wins, 0);
        assert_eq!(hou.losses, 1);
    }

    // ── compute_standings: sorting ──────────────────────────────────

    #[test]
    fn teams_sorted_by_win_pct_descending() {
        let team_a = al_east_team(1, "TeamA", "AAA");
        let team_b = al_east_team(2, "TeamB", "BBB");
        let team_c = al_east_team(3, "TeamC", "CCC");

        // A beats B twice, C beats A once, B beats C once
        // A: 2-1 (.667), B: 1-2 (.333), C: 1-1 (.500)
        let games = vec![
            make_game(100, &team_a, &team_b, Some(5), Some(3), "STATUS_FINAL"),
            make_game(101, &team_a, &team_b, Some(4), Some(2), "STATUS_FINAL"),
            make_game(102, &team_c, &team_a, Some(7), Some(1), "STATUS_FINAL"),
            make_game(103, &team_b, &team_c, Some(6), Some(4), "STATUS_FINAL"),
        ];

        let standings = compute_standings(&[team_a, team_b, team_c], &games, 2025);
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();

        assert_eq!(al_east.teams[0].team.abbreviation, "AAA"); // .667
        assert_eq!(al_east.teams[1].team.abbreviation, "CCC"); // .500
        assert_eq!(al_east.teams[2].team.abbreviation, "BBB"); // .333
    }

    #[test]
    fn tiebreaker_uses_total_wins() {
        let team_a = al_east_team(1, "TeamA", "AAA");
        let team_b = al_east_team(2, "TeamB", "BBB");
        let team_c = al_east_team(3, "TeamC", "CCC");

        // A: 2-2 (.500), B: 1-1 (.500), C: 1-1 (.500)
        let games = vec![
            make_game(100, &team_a, &team_b, Some(5), Some(3), "STATUS_FINAL"),
            make_game(101, &team_a, &team_c, Some(4), Some(2), "STATUS_FINAL"),
            make_game(102, &team_b, &team_a, Some(7), Some(1), "STATUS_FINAL"),
            make_game(103, &team_c, &team_a, Some(6), Some(4), "STATUS_FINAL"),
        ];

        let standings = compute_standings(&[team_a, team_b, team_c], &games, 2025);
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();

        assert_eq!(al_east.teams[0].team.abbreviation, "AAA"); // .500 with 2 wins
    }

    // ── compute_standings: games behind ─────────────────────────────

    #[test]
    fn division_leader_has_zero_gb() {
        let team_a = al_east_team(1, "TeamA", "AAA");
        let team_b = al_east_team(2, "TeamB", "BBB");

        let games = vec![make_game(
            100,
            &team_a,
            &team_b,
            Some(5),
            Some(3),
            "STATUS_FINAL",
        )];

        let standings = compute_standings(&[team_a, team_b], &games, 2025);
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();

        assert_eq!(al_east.teams[0].team.abbreviation, "AAA");
        assert!((al_east.teams[0].games_behind - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn games_behind_calculated_correctly() {
        let team_a = al_east_team(1, "TeamA", "AAA");
        let team_b = al_east_team(2, "TeamB", "BBB");

        // A: 3-0, B: 0-3 => GB = ((3-0) + (3-0)) / 2 = 3.0
        let games = vec![
            make_game(100, &team_a, &team_b, Some(5), Some(3), "STATUS_FINAL"),
            make_game(101, &team_a, &team_b, Some(4), Some(2), "STATUS_FINAL"),
            make_game(102, &team_a, &team_b, Some(6), Some(1), "STATUS_FINAL"),
        ];

        let standings = compute_standings(&[team_a, team_b], &games, 2025);
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();

        let bbb = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "BBB")
            .unwrap();
        assert!(
            (bbb.games_behind - 3.0).abs() < f64::EPSILON,
            "Expected 3.0 GB, got {}",
            bbb.games_behind
        );
    }

    #[test]
    fn games_behind_one_game_head_to_head() {
        let team_a = al_east_team(1, "TeamA", "AAA");
        let team_b = al_east_team(2, "TeamB", "BBB");

        // A: 1-0, B: 0-1 => GB = ((1-0) + (1-0)) / 2 = 1.0
        let games = vec![make_game(
            100,
            &team_a,
            &team_b,
            Some(5),
            Some(3),
            "STATUS_FINAL",
        )];

        let standings = compute_standings(&[team_a, team_b.clone()], &games, 2025);
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();

        let bbb = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "BBB")
            .unwrap();
        assert!(
            (bbb.games_behind - 1.0).abs() < f64::EPSILON,
            "Expected 1.0 GB, got {}",
            bbb.games_behind
        );
    }

    #[test]
    fn games_behind_half_game() {
        let team_a = al_east_team(1, "TeamA", "AAA");
        let team_b = al_east_team(2, "TeamB", "BBB");
        let team_c = al_east_team(3, "TeamC", "CCC");

        // A beats C: A is 1-0, B is 0-0, C is 0-1
        // B's GB = ((1-0) + (0-0)) / 2 = 0.5
        let games = vec![make_game(
            100,
            &team_a,
            &team_c,
            Some(5),
            Some(3),
            "STATUS_FINAL",
        )];

        let standings = compute_standings(&[team_a, team_b, team_c], &games, 2025);
        let al_east = standings
            .divisions
            .iter()
            .find(|d| d.name == "AL East")
            .unwrap();

        let bbb = al_east
            .teams
            .iter()
            .find(|r| r.team.abbreviation == "BBB")
            .unwrap();
        assert!(
            (bbb.games_behind - 0.5).abs() < f64::EPSILON,
            "Expected 0.5 GB, got {}",
            bbb.games_behind
        );
    }

    // ── compute_standings: wild card ────────────────────────────────

    #[test]
    fn wild_card_excludes_division_winners() {
        // Create 3 AL divisions with 2 teams each = 6 teams
        // Division winners should NOT appear in wild card
        let al_e1 = al_east_team(1, "E1", "AE1");
        let al_e2 = al_east_team(2, "E2", "AE2");
        let al_c1 = make_team(3, "C1", "AC1", "American", "Central");
        let al_c2 = make_team(4, "C2", "AC2", "American", "Central");
        let al_w1 = al_west_team(5, "W1", "AW1");
        let al_w2 = al_west_team(6, "W2", "AW2");

        // Make division winners: AE1, AC1, AW1 each beat their division mates
        let games = vec![
            make_game(100, &al_e1, &al_e2, Some(5), Some(3), "STATUS_FINAL"),
            make_game(101, &al_c1, &al_c2, Some(4), Some(2), "STATUS_FINAL"),
            make_game(102, &al_w1, &al_w2, Some(6), Some(1), "STATUS_FINAL"),
        ];

        let standings =
            compute_standings(&[al_e1, al_e2, al_c1, al_c2, al_w1, al_w2], &games, 2025);

        let al_wc = standings
            .wild_cards
            .iter()
            .find(|w| w.name == "AL Wild Card")
            .unwrap();
        let wc_abbrs: Vec<&str> = al_wc
            .teams
            .iter()
            .map(|r| r.team.abbreviation.as_str())
            .collect();

        // Division winners should not be in wild card
        assert!(!wc_abbrs.contains(&"AE1"));
        assert!(!wc_abbrs.contains(&"AC1"));
        assert!(!wc_abbrs.contains(&"AW1"));

        // Division losers should be in wild card
        assert!(wc_abbrs.contains(&"AE2"));
        assert!(wc_abbrs.contains(&"AC2"));
        assert!(wc_abbrs.contains(&"AW2"));
    }

    #[test]
    fn wild_card_has_two_leagues() {
        let standings = compute_standings(&[], &[], 2025);
        assert_eq!(standings.wild_cards.len(), 2);
        assert!(standings
            .wild_cards
            .iter()
            .any(|w| w.name == "AL Wild Card"));
        assert!(standings
            .wild_cards
            .iter()
            .any(|w| w.name == "NL Wild Card"));
    }

    // ── compute_standings: season passthrough ───────────────────────

    #[test]
    fn standings_carries_season() {
        let standings = compute_standings(&[], &[], 2025);
        assert_eq!(standings.season, 2025);
    }

    // ── compute_standings: empty inputs ─────────────────────────────

    #[test]
    fn no_teams_produces_empty_standings() {
        let standings = compute_standings(&[], &[], 2025);
        assert_eq!(standings.divisions.len(), 6);
        for div in &standings.divisions {
            assert!(div.teams.is_empty());
        }
    }

    #[test]
    fn six_divisions_always_present() {
        let standings = compute_standings(&[], &[], 2025);
        assert_eq!(standings.divisions.len(), 6);

        let names: Vec<&str> = standings
            .divisions
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert!(names.contains(&"AL East"));
        assert!(names.contains(&"AL Central"));
        assert!(names.contains(&"AL West"));
        assert!(names.contains(&"NL East"));
        assert!(names.contains(&"NL Central"));
        assert!(names.contains(&"NL West"));
    }

    // ── games_behind helper ─────────────────────────────────────────

    #[test]
    fn games_behind_identical_records_is_zero() {
        let team = al_east_team(1, "Test", "TST");
        let record = TeamRecord {
            team,
            wins: 50,
            losses: 30,
            win_pct: 0.625,
            games_behind: 0.0,
        };
        assert!((games_behind(&record, &record) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn games_behind_formula() {
        let team_a = al_east_team(1, "A", "AAA");
        let team_b = al_east_team(2, "B", "BBB");

        let leader = TeamRecord {
            team: team_a,
            wins: 60,
            losses: 30,
            win_pct: 0.667,
            games_behind: 0.0,
        };
        let trailer = TeamRecord {
            team: team_b,
            wins: 50,
            losses: 40,
            win_pct: 0.556,
            games_behind: 0.0,
        };

        // GB = ((60-50) + (40-30)) / 2 = 10.0
        assert!((games_behind(&leader, &trailer) - 10.0).abs() < f64::EPSILON);
    }
}
