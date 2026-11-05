//! Swappable game rules. A mode stores one variant of each enum; each variant
//! selects a trait implementation, so new behavior is one new implementation.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// What a round asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptRule {
    None,
}

/// What players submit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollectRule {
    /// One or more songs per round, visible to everyone once collect closes.
    Pick,
}

/// How submissions are grouped for voting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BallotRule {
    /// One ballot with every submission of the round.
    All,
}

/// How a round's result is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WinnerRule {
    /// Most votes per group; ties go to the earliest submission.
    MostVotes,
}

/// What players do while songs play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeRule {
    Listen,
}

/// Who earns points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScoringRule {
    /// Every vote a song receives scores for the player who picked it.
    VotesToPicker,
}

/// One rule choice per decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub prompt: PromptRule,
    pub collect: CollectRule,
    pub ballot: BallotRule,
    pub winner: WinnerRule,
    pub challenge: ChallengeRule,
    pub scoring: ScoringRule,
}

impl Rules {
    /// Rejects combinations the engine cannot run.
    pub fn validate(&self) -> Result<(), &'static str> {
        Ok(())
    }
}

/// A submission as the rules see it: an identity, its owner and its order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub user_id: i64,
    pub seq: i64,
}

/// Groups a round's submissions into ballots.
pub trait Ballot {
    fn groups(&self, entries: &[Entry]) -> Vec<Vec<Entry>>;
}

/// Chooses winners from the voted groups. `votes` counts per submission ID.
pub trait Winner {
    fn winners(&self, groups: &[Vec<Entry>], votes: &HashMap<String, i64>) -> Vec<Entry>;
}

/// Points awarded when a round's vote closes, per account.
pub trait Scoring {
    fn round_points(
        &self,
        entries: &[Entry],
        winners: &[Entry],
        votes: &HashMap<String, i64>,
    ) -> HashMap<i64, i64>;
}

/// Points for each vote a song receives under `VotesToPicker`.
pub const POINTS_PER_VOTE: i64 = 10;

struct AllBallot;
impl Ballot for AllBallot {
    fn groups(&self, entries: &[Entry]) -> Vec<Vec<Entry>> {
        if entries.is_empty() {
            Vec::new()
        } else {
            vec![entries.to_vec()]
        }
    }
}

struct MostVotes;
impl Winner for MostVotes {
    fn winners(&self, groups: &[Vec<Entry>], votes: &HashMap<String, i64>) -> Vec<Entry> {
        groups
            .iter()
            .filter_map(|group| most_votes(group, votes))
            .collect()
    }
}

/// Highest count wins; equal counts go to the lowest submission sequence.
pub fn most_votes(group: &[Entry], votes: &HashMap<String, i64>) -> Option<Entry> {
    group
        .iter()
        .max_by(|a, b| {
            let (va, vb) = (
                votes.get(&a.id).unwrap_or(&0),
                votes.get(&b.id).unwrap_or(&0),
            );
            va.cmp(vb).then(b.seq.cmp(&a.seq))
        })
        .cloned()
}

struct VotesToPicker;
impl Scoring for VotesToPicker {
    fn round_points(
        &self,
        entries: &[Entry],
        _winners: &[Entry],
        votes: &HashMap<String, i64>,
    ) -> HashMap<i64, i64> {
        let mut points = HashMap::new();
        for entry in entries {
            let count = votes.get(&entry.id).copied().unwrap_or(0);
            if count > 0 {
                *points.entry(entry.user_id).or_insert(0) += count * POINTS_PER_VOTE;
            }
        }
        points
    }
}

impl BallotRule {
    pub fn imp(self) -> &'static dyn Ballot {
        match self {
            Self::All => &AllBallot,
        }
    }
}

impl WinnerRule {
    pub fn imp(self) -> &'static dyn Winner {
        match self {
            Self::MostVotes => &MostVotes,
        }
    }
}

impl ScoringRule {
    pub fn imp(self) -> &'static dyn Scoring {
        match self {
            Self::VotesToPicker => &VotesToPicker,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, user_id: i64, seq: i64) -> Entry {
        Entry {
            id: id.into(),
            user_id,
            seq,
        }
    }

    #[test]
    fn all_ballot_is_one_group_in_submission_order() {
        let entries = vec![entry("a", 1, 1), entry("b", 2, 2)];
        assert_eq!(BallotRule::All.imp().groups(&entries), vec![entries]);
        assert!(BallotRule::All.imp().groups(&[]).is_empty());
    }

    #[test]
    fn most_votes_breaks_ties_by_earliest_submission() {
        let group = vec![entry("a", 1, 1), entry("b", 2, 2), entry("c", 3, 3)];
        let votes = HashMap::from([("b".to_string(), 2), ("c".to_string(), 2)]);
        let winners = WinnerRule::MostVotes
            .imp()
            .winners(&[group.clone()], &votes);
        assert_eq!(winners, vec![entry("b", 2, 2)]);
        let none = WinnerRule::MostVotes
            .imp()
            .winners(&[group], &HashMap::new());
        assert_eq!(none, vec![entry("a", 1, 1)]);
    }

    #[test]
    fn votes_to_picker_scores_every_vote_for_its_picker() {
        let entries = vec![entry("a", 1, 1), entry("b", 2, 2), entry("c", 1, 3)];
        let votes = HashMap::from([("a".to_string(), 2), ("c".to_string(), 1)]);
        let points = ScoringRule::VotesToPicker
            .imp()
            .round_points(&entries, &[], &votes);
        assert_eq!(points, HashMap::from([(1, 3 * POINTS_PER_VOTE)]));
    }
}
