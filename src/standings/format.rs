use chrono::Utc;
use serenity::all::Colour;
use serenity::builder::CreateEmbed;

use super::compute::{DivisionStandings, Standings, TeamRecord, WildCardStandings};

/// Format the season display string (e.g., "2025").
/// MLB seasons are within a single calendar year, unlike NBA.
fn season_display(season: u32) -> String {
    format!("{season}")
}

/// Format games behind as a display string.
/// Leader shows "-", half games show ".5", whole games show no decimal.
fn format_gb(gb: f64) -> String {
    if gb.abs() < f64::EPSILON {
        "-".to_string()
    } else if (gb - gb.floor()).abs() > f64::EPSILON {
        // Has a .5 component
        format!("{:.1}", gb)
    } else {
        format!("{:.0}", gb)
    }
}

/// Build a formatted table string from a list of team records.
fn build_division_table(records: &[TeamRecord]) -> String {
    let mut lines = Vec::with_capacity(records.len() + 2);

    // Header
    lines.push(format!(
        "{:<4} {:<4} {:>3} {:>3}  {:>5}  {:>4}",
        "#", "Team", "W", "L", "PCT", "GB"
    ));
    lines.push("\u{2500}".repeat(30));

    // Team rows
    for (i, record) in records.iter().enumerate() {
        lines.push(format!(
            "{:<4} {:<4} {:>3} {:>3}  {:.3}  {:>4}",
            i + 1,
            record.team.abbreviation,
            record.wins,
            record.losses,
            record.win_pct,
            format_gb(record.games_behind)
        ));
    }

    lines.join("\n")
}

/// Build a formatted table for wild card standings.
/// Teams in a WC spot get a separator line after them.
fn build_wild_card_table(records: &[TeamRecord], wc_spots: usize) -> String {
    let mut lines = Vec::with_capacity(records.len() + 4);

    // Header
    lines.push(format!(
        "{:<4} {:<4} {:>3} {:>3}  {:>5}  {:>4}",
        "#", "Team", "W", "L", "PCT", "GB"
    ));
    lines.push("\u{2500}".repeat(30));

    for (i, record) in records.iter().enumerate() {
        lines.push(format!(
            "{:<4} {:<4} {:>3} {:>3}  {:.3}  {:>4}",
            i + 1,
            record.team.abbreviation,
            record.wins,
            record.losses,
            record.win_pct,
            format_gb(record.games_behind)
        ));

        // Draw a separator after the last wild card spot
        if i + 1 == wc_spots && i + 1 < records.len() {
            lines.push("\u{2504}".repeat(30));
        }
    }

    lines.join("\n")
}

/// AL embed colour (MLB navy).
const AL_COLOUR: Colour = Colour::new(0x002D72);
/// NL embed colour (MLB red).
const NL_COLOUR: Colour = Colour::new(0xD50032);

/// Number of wild card spots per league.
const WILD_CARD_SPOTS: usize = 3;

fn league_colour(league: &str) -> Colour {
    if league == "American" {
        AL_COLOUR
    } else {
        NL_COLOUR
    }
}

/// Build a Discord embed for a division standings.
fn build_division_embed(div: &DivisionStandings, season_str: &str, timestamp: &str) -> CreateEmbed {
    let table = build_division_table(&div.teams);

    CreateEmbed::new()
        .title(format!("{} \u{2014} {season_str}", div.name))
        .description(format!("```\n{table}\n```"))
        .colour(league_colour(&div.league))
        .footer(serenity::builder::CreateEmbedFooter::new(format!(
            "Updated {timestamp}"
        )))
}

/// Build a Discord embed for a wild card race.
fn build_wild_card_embed(wc: &WildCardStandings, season_str: &str, timestamp: &str) -> CreateEmbed {
    let table = build_wild_card_table(&wc.teams, WILD_CARD_SPOTS);

    CreateEmbed::new()
        .title(format!("{} \u{2014} {season_str}", wc.name))
        .description(format!("```\n{table}\n```"))
        .colour(league_colour(&wc.league))
        .footer(serenity::builder::CreateEmbedFooter::new(format!(
            "Updated {timestamp}"
        )))
}

/// Build Discord embeds for the standings (6 divisions + 2 wild card = 8 embeds).
/// Discord allows max 10 embeds per message, so 8 is within limits.
pub fn build_standings_embeds(standings: &Standings) -> Vec<CreateEmbed> {
    let season_str = season_display(standings.season);
    let timestamp = Utc::now().format("%B %-d, %Y at %-I:%M %p UTC").to_string();

    let mut embeds = Vec::with_capacity(8);

    // Division embeds
    for div in &standings.divisions {
        embeds.push(build_division_embed(div, &season_str, &timestamp));
    }

    // Wild card embeds
    for wc in &standings.wild_cards {
        embeds.push(build_wild_card_embed(wc, &season_str, &timestamp));
    }

    embeds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::Team;
    use crate::standings::compute::{DivisionStandings, Standings, TeamRecord, WildCardStandings};

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

    fn make_record(team: Team, wins: u32, losses: u32, gb: f64) -> TeamRecord {
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
            games_behind: gb,
        }
    }

    fn empty_standings() -> Standings {
        Standings {
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
        }
    }

    // ── season_display ──────────────────────────────────────────────

    #[test]
    fn season_display_normal() {
        assert_eq!(season_display(2025), "2025");
    }

    #[test]
    fn season_display_century_boundary() {
        assert_eq!(season_display(2099), "2099");
    }

    // ── format_gb ───────────────────────────────────────────────────

    #[test]
    fn format_gb_leader() {
        assert_eq!(format_gb(0.0), "-");
    }

    #[test]
    fn format_gb_whole_number() {
        assert_eq!(format_gb(5.0), "5");
    }

    #[test]
    fn format_gb_half_game() {
        assert_eq!(format_gb(2.5), "2.5");
    }

    #[test]
    fn format_gb_half_game_only() {
        assert_eq!(format_gb(0.5), "0.5");
    }

    #[test]
    fn format_gb_large_number() {
        assert_eq!(format_gb(25.0), "25");
    }

    // ── build_division_table ────────────────────────────────────────

    #[test]
    fn build_division_table_empty_records() {
        let table = build_division_table(&[]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 2); // header + separator
    }

    #[test]
    fn build_division_table_contains_header_with_gb() {
        let table = build_division_table(&[]);
        assert!(table.contains("#"));
        assert!(table.contains("Team"));
        assert!(table.contains("W"));
        assert!(table.contains("L"));
        assert!(table.contains("PCT"));
        assert!(table.contains("GB"));
    }

    #[test]
    fn build_division_table_single_team() {
        let team = make_team(1, "Yankees", "NYY", "American", "East");
        let record = make_record(team, 50, 20, 0.0);
        let table = build_division_table(&[record]);
        let lines: Vec<&str> = table.lines().collect();

        assert_eq!(lines.len(), 3); // header + separator + 1 row
        assert!(lines[2].contains("NYY"));
        assert!(lines[2].contains("50"));
        assert!(lines[2].contains("20"));
    }

    #[test]
    fn build_division_table_shows_gb() {
        let team_a = make_team(1, "A", "AAA", "American", "East");
        let team_b = make_team(2, "B", "BBB", "American", "East");
        let records = vec![
            make_record(team_a, 60, 30, 0.0),
            make_record(team_b, 50, 40, 10.0),
        ];
        let table = build_division_table(&records);

        assert!(table.contains("-"), "Leader should show '-' for GB");
        assert!(table.contains("10"), "Trailer should show 10 GB");
    }

    // ── build_wild_card_table ───────────────────────────────────────

    #[test]
    fn build_wild_card_table_has_separator_after_wc_spots() {
        let teams: Vec<TeamRecord> = (0..5)
            .map(|i| {
                let team = make_team(
                    i + 1,
                    &format!("T{}", i + 1),
                    &format!("T{:02}", i + 1),
                    "American",
                    "East",
                );
                make_record(team, 50 - i as u32, 30 + i as u32, i as f64)
            })
            .collect();

        let table = build_wild_card_table(&teams, 3);
        // The dashed separator should appear after the 3rd team
        assert!(
            table.contains("\u{2504}"),
            "Should have a WC cutoff separator"
        );
    }

    // ── build_standings_embeds ───────────────────────────────────────

    #[test]
    fn build_standings_embeds_returns_eight_embeds() {
        let standings = empty_standings();
        let embeds = build_standings_embeds(&standings);
        assert_eq!(embeds.len(), 8);
    }

    #[test]
    fn build_standings_embeds_within_discord_limit() {
        let standings = empty_standings();
        let embeds = build_standings_embeds(&standings);
        assert!(
            embeds.len() <= 10,
            "Discord allows max 10 embeds per message"
        );
    }

    #[test]
    fn build_standings_embeds_with_team_data() {
        let record = make_record(
            make_team(1, "Yankees", "NYY", "American", "East"),
            50,
            20,
            0.0,
        );

        let mut standings = empty_standings();
        standings.divisions[0].teams.push(record);

        let embeds = build_standings_embeds(&standings);
        assert_eq!(embeds.len(), 8);
    }
}
