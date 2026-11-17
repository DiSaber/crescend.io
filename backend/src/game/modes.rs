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
    /// How long the answer shows after a Name That Tune round.
    #[serde(default = "default_reveal")]
    pub reveal_seconds: i64,
}

fn default_reveal() -> i64 {
    8
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

#[derive(Clone, Copy)]
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
    int(
        "reveal_seconds",
        "Answer shows for",
        3,
        30,
        1,
        Some("seconds"),
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
            "reveal_seconds" => self.reveal_seconds.into(),
            _ => serde_json::Value::Null,
        }
    }

    fn set(&mut self, key: &str, value: i64) {
        let flag = value != 0;
        match key {
            "rounds" => self.rounds = value,
            "collect_seconds" => self.collect_seconds = value,
            "vote_seconds" => self.vote_seconds = value,
            "play_seconds" => self.play_seconds = value,
            "clip_start_seconds" => self.clip_start_seconds = value,
            "votes_per_player" => self.votes_per_player = value,
            "songs_per_player" => self.songs_per_player = value,
            "self_vote" => self.self_vote = flag,
            "overlap_next_round" => self.overlap_next_round = flag,
            "reveal_seconds" => self.reveal_seconds = value,
            _ => unreachable!("unknown setting {key}"),
        }
    }
}

/// The settings a rule set exposes, in display order, with labels and limits for
/// that rule set. Unexposed keys keep their defaults.
fn specs(rules: &Rules) -> Vec<Spec> {
    let guessing = rules.challenge == ChallengeRule::GuessTitle;
    SPECS
        .iter()
        .copied()
        .filter(|spec| match spec.key {
            "votes_per_player" => rules.ballot == BallotRule::All,
            "vote_seconds" | "self_vote" | "overlap_next_round" => !guessing,
            "reveal_seconds" => guessing,
            _ => true,
        })
        .map(|spec| match (guessing, spec.key) {
            (false, "rounds") if rules.prompt != PromptRule::None => Spec {
                label: "Rounds (0 = one per prompt)",
                ..spec
            },
            (true, "rounds") => Spec {
                label: "Rounds (0 = every song)",
                ..spec
            },
            (true, "collect_seconds") => Spec {
                label: "Time to propose songs",
                ..spec
            },
            (true, "songs_per_player") => Spec {
                label: "Songs each player proposes",
                ..spec
            },
            // Name That Tune plays clips: the clip is the guessing window.
            (true, "play_seconds") => Spec {
                label: "Guess time",
                min: 10,
                max: 120,
                ..spec
            },
            _ => spec,
        })
        .collect()
}

/// Why customization was rejected; the message is safe to show.
#[derive(Debug, PartialEq, Eq)]
pub enum ModeError {
    Rules(&'static str),
    Setting(String),
}

/// Applies `overrides` on top of `definition`, enforcing the served schema: only
/// exposed keys, integers for integer settings, booleans for boolean ones, and
/// `min <= value <= max` on the step grid.
pub fn customize(
    mut definition: Definition,
    overrides: &serde_json::Map<String, serde_json::Value>,
) -> Result<Definition, ModeError> {
    definition.rules.validate().map_err(ModeError::Rules)?;
    let exposed = specs(&definition.rules);
    for (key, value) in overrides {
        let spec = exposed
            .iter()
            .find(|spec| spec.key == key)
            .ok_or_else(|| ModeError::Setting(format!("unknown setting {key}")))?;
        let value = match spec.kind {
            SettingType::Boolean => value.as_bool().map(i64::from),
            SettingType::Integer => value.as_i64().filter(|_| !value.is_f64()),
        }
        .ok_or_else(|| ModeError::Setting(format!("{key} has the wrong type")))?;
        if !(spec.min..=spec.max).contains(&value) || (value - spec.min) % spec.step != 0 {
            return Err(ModeError::Setting(format!(
                "{key} must be between {} and {} in steps of {}",
                spec.min, spec.max, spec.step
            )));
        }
        definition.settings.set(key, value);
    }
    Ok(definition)
}

/// Starting settings for a custom rule set: the preset that shares its collect rule.
pub fn default_settings(rules: &Rules) -> Settings {
    match rules.collect {
        CollectRule::Pick => party().settings,
        CollectRule::Propose => name_that_tune().settings,
    }
}

pub fn schema(definition: &Definition) -> Vec<SettingSchema> {
    specs(&definition.rules)
        .into_iter()
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
            reveal_seconds: default_reveal(),
        },
    }
}

fn picture_this() -> Definition {
    let mut definition = party();
    definition.name = "Picture This".into();
    definition.rules.prompt = PromptRule::Image;
    definition.settings.rounds = 0;
    definition.settings.collect_seconds = 60;
    definition.settings.play_seconds = 60;
    definition
}

fn name_that_tune() -> Definition {
    Definition {
        name: "Name That Tune".into(),
        rules: Rules {
            prompt: PromptRule::None,
            collect: CollectRule::Propose,
            ballot: BallotRule::None,
            winner: WinnerRule::None,
            challenge: ChallengeRule::GuessTitle,
            scoring: ScoringRule::GuessSpeed,
        },
        settings: Settings {
            rounds: 0,
            collect_seconds: 90,
            vote_seconds: 30,
            play_seconds: 30,
            clip_start_seconds: 30,
            votes_per_player: 1,
            songs_per_player: 2,
            self_vote: false,
            overlap_next_round: false,
            reveal_seconds: default_reveal(),
        },
    }
}

fn speed_round() -> Definition {
    let mut definition = party();
    definition.name = "Speed Round".into();
    definition.settings.collect_seconds = 20;
    definition.settings.vote_seconds = 15;
    definition.settings.play_seconds = 60;
    definition.settings.rounds = 8;
    definition
}

fn duel() -> Definition {
    let mut definition = party();
    definition.name = "Duel".into();
    definition.rules.ballot = BallotRule::Pairs;
    definition.rules.winner = WinnerRule::PairWinners;
    definition.rules.scoring = ScoringRule::WinnerBonus;
    definition.settings.rounds = 3;
    definition.settings.play_seconds = 60;
    definition
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "party",
        description: "Everyone picks a song, everyone votes, the winner plays.",
        definition: party,
    },
    Preset {
        id: "speed_round",
        description: "Party on fast timers with one-minute clips.",
        definition: speed_round,
    },
    Preset {
        id: "duel",
        description: "Songs face off in pairs; every pair winner plays.",
        definition: duel,
    },
    Preset {
        id: "picture_this",
        description: "The host shows a picture; everyone picks the song that fits it best.",
        definition: picture_this,
    },
    Preset {
        id: "name_that_tune",
        description: "Everyone secretly proposes songs; race to type each title.",
        definition: name_that_tune,
    },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn overrides(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn customize_enforces_keys_types_ranges_and_steps() {
        let custom = customize(
            party(),
            &overrides(json!({"rounds": 0, "play_seconds": 30, "self_vote": true})),
        )
        .unwrap();
        assert_eq!(custom.settings.rounds, 0);
        assert_eq!(custom.settings.play_seconds, 30);
        assert!(custom.settings.self_vote);
        for bad in [
            json!({"nope": 1}),
            json!({"rounds": 51}),
            json!({"rounds": -1}),
            json!({"collect_seconds": 12}),
            json!({"collect_seconds": 5}),
            json!({"rounds": 2.0}),
            json!({"rounds": "2"}),
            json!({"self_vote": 1}),
            json!({"rounds": true}),
        ] {
            assert!(
                matches!(
                    customize(party(), &overrides(bad.clone())),
                    Err(ModeError::Setting(_))
                ),
                "{bad}"
            );
        }
        // Pairs always take one vote per pair, so the setting is not exposed.
        assert!(customize(duel(), &overrides(json!({"votes_per_player": 2}))).is_err());
        assert!(!schema(&duel()).iter().any(|s| s.key == "votes_per_player"));
        let tune = schema(&name_that_tune());
        let guess = tune.iter().find(|s| s.key == "play_seconds").unwrap();
        assert_eq!((guess.min, guess.label), (Some(10), "Guess time"));
        assert!(
            !tune
                .iter()
                .any(|s| s.key == "vote_seconds" || s.key == "overlap_next_round")
        );
        assert!(customize(name_that_tune(), &overrides(json!({"play_seconds": 0}))).is_err());
        assert!(customize(name_that_tune(), &overrides(json!({"reveal_seconds": 5}))).is_ok());
        assert!(customize(party(), &overrides(json!({"reveal_seconds": 5}))).is_err());
        let mut broken = party();
        broken.rules.winner = WinnerRule::PairWinners;
        assert!(matches!(
            customize(broken, &Default::default()),
            Err(ModeError::Rules(_))
        ));
    }

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
