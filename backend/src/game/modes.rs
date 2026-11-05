//! Presets, resolved mode definitions and the settings schema served to clients.
use super::rules::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Every setting the engine reads. A mode's schema exposes only the keys its rules use;
/// the rest keep their defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Settings {
    /// Rounds to play; 0 means until the host stops the game.
    pub rounds: i64,
    pub collect_seconds: i64,
    pub vote_seconds: i64,
    /// How much of each winning song plays; 0 plays the full song.
    pub play_seconds: i64,
    pub clip_start_seconds: i64,
    pub votes_per_player: i64,
    pub songs_per_player: i64,
    pub self_vote: bool,
    pub overlap_next_round: bool,
}

/// A mode as the engine runs it: rule choices plus concrete settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Definition {
    pub name: String,
    pub rules: Rules,
    pub settings: Settings,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettingType {
    Integer,
    Boolean,
}

/// One customizable setting. Integers satisfy min <= value <= max and (value - min) % step == 0.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SettingSchema {
    pub key: &'static str,
    pub label: &'static str,
    #[serde(rename = "type")]
    pub kind: SettingType,
    #[schema(value_type = Object)]
    pub default: serde_json::Value,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub step: Option<i64>,
    pub unit: Option<&'static str>,
}

struct Spec {
    key: &'static str,
    label: &'static str,
    kind: SettingType,
    min: i64,
    max: i64,
    step: i64,
    unit: Option<&'static str>,
}

const fn int(
    key: &'static str,
    label: &'static str,
    min: i64,
    max: i64,
    step: i64,
    unit: Option<&'static str>,
) -> Spec {
    Spec {
        key,
        label,
        kind: SettingType::Integer,
        min,
        max,
        step,
        unit,
    }
}

const fn boolean(key: &'static str, label: &'static str) -> Spec {
    Spec {
        key,
        label,
        kind: SettingType::Boolean,
        min: 0,
        max: 1,
        step: 1,
        unit: None,
    }
}

const SPECS: &[Spec] = &[
    int(
        "rounds",
        "Rounds (0 = until stopped)",
        0,
        50,
        1,
        Some("rounds"),
    ),
    int(
        "collect_seconds",
        "Time to pick",
        10,
        300,
        5,
        Some("seconds"),
    ),
    int("vote_seconds", "Voting time", 10, 120, 5, Some("seconds")),
    int(
        "play_seconds",
        "Song length (0 = full song)",
        0,
        600,
        5,
        Some("seconds"),
    ),
    int(
        "clip_start_seconds",
        "Start songs at",
        0,
        300,
        5,
        Some("seconds"),
    ),
    int("votes_per_player", "Votes per player", 1, 5, 1, None),
    int("songs_per_player", "Songs per player", 1, 3, 1, None),
    boolean("self_vote", "Players can vote for their own song"),
    boolean(
        "overlap_next_round",
        "Next round picks while the winner plays",
    ),
];

impl Settings {
    fn get(&self, key: &str) -> serde_json::Value {
        match key {
            "rounds" => self.rounds.into(),
            "collect_seconds" => self.collect_seconds.into(),
            "vote_seconds" => self.vote_seconds.into(),
            "play_seconds" => self.play_seconds.into(),
            "clip_start_seconds" => self.clip_start_seconds.into(),
            "votes_per_player" => self.votes_per_player.into(),
            "songs_per_player" => self.songs_per_player.into(),
            "self_vote" => self.self_vote.into(),
            "overlap_next_round" => self.overlap_next_round.into(),
            _ => serde_json::Value::Null,
        }
    }
}

/// The keys a rule set exposes, in display order.
pub fn keys(_rules: &Rules) -> Vec<&'static str> {
    SPECS.iter().map(|s| s.key).collect()
}

pub fn schema(definition: &Definition) -> Vec<SettingSchema> {
    let keys = keys(&definition.rules);
    SPECS
        .iter()
        .filter(|spec| keys.contains(&spec.key))
        .map(|spec| {
            let integer = matches!(spec.kind, SettingType::Integer);
            SettingSchema {
                key: spec.key,
                label: spec.label,
                kind: spec.kind,
                default: definition.settings.get(spec.key),
                min: integer.then_some(spec.min),
                max: integer.then_some(spec.max),
                step: integer.then_some(spec.step),
                unit: spec.unit,
            }
        })
        .collect()
}

pub struct Preset {
    pub id: &'static str,
    pub description: &'static str,
    pub definition: fn() -> Definition,
}

fn party() -> Definition {
    Definition {
        name: "Party".into(),
        rules: Rules {
            prompt: PromptRule::None,
            collect: CollectRule::Pick,
            ballot: BallotRule::All,
            winner: WinnerRule::MostVotes,
            challenge: ChallengeRule::Listen,
            scoring: ScoringRule::VotesToPicker,
        },
        settings: Settings {
            rounds: 5,
            collect_seconds: 45,
            vote_seconds: 30,
            play_seconds: 0,
            clip_start_seconds: 0,
            votes_per_player: 1,
            songs_per_player: 1,
            self_vote: false,
            overlap_next_round: true,
        },
    }
}

pub const PRESETS: &[Preset] = &[Preset {
    id: "party",
    description: "Everyone picks a song, everyone votes, the winner plays.",
    definition: party,
}];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_schema_defaults_match_settings() {
        for preset in PRESETS {
            let definition = (preset.definition)();
            definition.rules.validate().unwrap();
            for setting in schema(&definition) {
                assert_eq!(setting.default, definition.settings.get(setting.key));
                if let (Some(min), Some(max), Some(step)) = (setting.min, setting.max, setting.step)
                {
                    let value = setting.default.as_i64().unwrap();
                    assert!(
                        (min..=max).contains(&value),
                        "{} {}",
                        preset.id,
                        setting.key
                    );
                    assert_eq!((value - min) % step, 0, "{} {}", preset.id, setting.key);
                }
            }
        }
    }
}
