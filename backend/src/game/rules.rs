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
    /// Each round shows the host's next text prompt.
    Text,
    /// Each round shows the host's next uploaded image, with optional text.
    Image,
}

/// What players submit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollectRule {
    /// One or more songs per round, visible to everyone once collect closes.
    Pick,
    /// Songs proposed once, up front, and kept secret; each round plays one.
    Propose,
}

/// How submissions are grouped for voting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BallotRule {
    /// One ballot with every submission of the round.
    All,
    /// Head-to-head pairs in submission order; an odd song out gets a bye.
    Pairs,
    /// No voting (Name That Tune).
    None,
}

/// How a round's result is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WinnerRule {
    /// Most votes on the single ballot; ties go to the earliest submission.
    MostVotes,
    /// The winner of every pair (same tie rule); all of them play.
    PairWinners,
    /// No vote to decide (Name That Tune).
    None,
}

/// What players do while songs play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeRule {
    Listen,
    /// Race to type the hidden song's title while a clip plays.
    GuessTitle,
}

/// Who earns points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScoringRule {
    /// Every vote a song receives scores for the player who picked it.
    VotesToPicker,
    /// A flat bonus for each contested group a player's song wins.
    WinnerBonus,
    /// Faster correct guesses score more; the proposer scores per correct guesser.
    GuessSpeed,
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
    /// Rejects combinations the engine cannot run, naming the first conflict.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.collect == CollectRule::Propose {
            if self.prompt != PromptRule::None {
                return Err("collect propose cannot use prompts");
            }
            if (self.ballot, self.winner) != (BallotRule::None, WinnerRule::None) {
                return Err("collect propose requires ballot none and winner none");
            }
            if self.challenge != ChallengeRule::GuessTitle {
                return Err("collect propose requires challenge guess_title");
            }
            if self.scoring != ScoringRule::GuessSpeed {
                return Err("challenge guess_title requires scoring guess_speed");
            }
            return Ok(());
        }
        if self.challenge != ChallengeRule::Listen {
            return Err("challenge guess_title requires collect propose");
        }
        if self.scoring == ScoringRule::GuessSpeed {
            return Err("scoring guess_speed requires challenge guess_title");
        }
        match (self.ballot, self.winner) {
            (BallotRule::All, WinnerRule::MostVotes)
            | (BallotRule::Pairs, WinnerRule::PairWinners) => {}
            (BallotRule::None, _) | (_, WinnerRule::None) => {
                return Err("collect pick requires a ballot and a winner rule");
            }
            (BallotRule::All, _) => return Err("ballot all requires winner most_votes"),
            (BallotRule::Pairs, _) => return Err("ballot pairs requires winner pair_winners"),
        }
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

/// Groups a round's submissions into ballots and accounts for votes on them.
pub trait Ballot {
    fn groups(&self, entries: &[Entry]) -> Vec<Vec<Entry>>;
    /// Whether `user` may vote for `entry` in `group` at all. Without self votes a
    /// player never votes for their own song.
    fn eligible(&self, _group: &[Entry], entry: &Entry, user: i64, self_vote: bool) -> bool {
        self_vote || entry.user_id != user
    }
    /// Votes a player must cast to be done: `votable` says which entries they may pick.
    fn needed(
        &self,
        groups: &[Vec<Entry>],
        votable: &dyn Fn(&Entry) -> bool,
        per_player: i64,
    ) -> i64;
    /// Whether one more vote for `target` fits beside the player's existing votes.
    fn allows(&self, groups: &[Vec<Entry>], mine: &[String], target: &str, per_player: i64)
    -> bool;
}

/// Chooses winners from the voted groups. `votes` counts per submission ID.
pub trait Winner {
    fn winners(&self, groups: &[Vec<Entry>], votes: &HashMap<String, i64>) -> Vec<Entry>;
}

/// Points awarded when a round's vote closes, per account, and for title guesses.
pub trait Scoring {
    fn round_points(
        &self,
        _groups: &[Vec<Entry>],
        _winners: &[Entry],
        _votes: &HashMap<String, i64>,
    ) -> HashMap<i64, i64> {
        HashMap::new()
    }
    /// Points for a correct guess: `rank` is 1 for the first correct guesser and
    /// `remaining` of `window` seconds were left.
    fn guess_points(&self, _rank: i64, _remaining: i64, _window: i64) -> i64 {
        0
    }
    /// Points for the proposer when the round closes with `correct` correct guessers.
    fn proposer_points(&self, _correct: i64) -> i64 {
        0
    }
}

/// Points for each vote a song receives under `VotesToPicker`.
pub const POINTS_PER_VOTE: i64 = 10;
/// Points for winning a contested group under `WinnerBonus`. Byes earn nothing.
pub const WIN_BONUS: i64 = 50;
/// `GuessSpeed`: every correct guess earns the base, plus up to the speed points
/// in proportion to the time left; the first correct guesser adds a bonus.
pub const GUESS_BASE: i64 = 50;
pub const GUESS_SPEED: i64 = 50;
pub const FIRST_GUESS_BONUS: i64 = 25;
/// `GuessSpeed`: the proposer's points per correct guesser.
pub const PROPOSER_PER_GUESS: i64 = 20;

struct AllBallot;
impl Ballot for AllBallot {
    fn groups(&self, entries: &[Entry]) -> Vec<Vec<Entry>> {
        if entries.is_empty() {
            Vec::new()
        } else {
            vec![entries.to_vec()]
        }
    }
    fn needed(
        &self,
        groups: &[Vec<Entry>],
        votable: &dyn Fn(&Entry) -> bool,
        per_player: i64,
    ) -> i64 {
        let available = groups.iter().flatten().filter(|e| votable(e)).count() as i64;
        available.min(per_player)
    }
    fn allows(
        &self,
        _groups: &[Vec<Entry>],
        mine: &[String],
        _target: &str,
        per_player: i64,
    ) -> bool {
        (mine.len() as i64) < per_player
    }
}

struct PairsBallot;
impl Ballot for PairsBallot {
    fn groups(&self, entries: &[Entry]) -> Vec<Vec<Entry>> {
        entries.chunks(2).map(<[Entry]>::to_vec).collect()
    }
    /// Without self votes a player sits out the pair their own song is in.
    fn eligible(&self, group: &[Entry], _entry: &Entry, user: i64, self_vote: bool) -> bool {
        self_vote || !group.iter().any(|e| e.user_id == user)
    }
    /// One vote in every pair the player may vote in; byes take no votes.
    fn needed(
        &self,
        groups: &[Vec<Entry>],
        votable: &dyn Fn(&Entry) -> bool,
        _per_player: i64,
    ) -> i64 {
        groups
            .iter()
            .filter(|g| g.len() > 1 && g.iter().any(votable))
            .count() as i64
    }
    fn allows(
        &self,
        groups: &[Vec<Entry>],
        mine: &[String],
        target: &str,
        _per_player: i64,
    ) -> bool {
        let Some(group) = groups.iter().find(|g| g.iter().any(|e| e.id == target)) else {
            return false;
        };
        group.len() > 1 && !group.iter().any(|e| mine.contains(&e.id))
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

/// Same tie rule per pair; every pair contributes a winner, in pair order.
struct PairWinners;
impl Winner for PairWinners {
    fn winners(&self, groups: &[Vec<Entry>], votes: &HashMap<String, i64>) -> Vec<Entry> {
        MostVotes.winners(groups, votes)
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
        groups: &[Vec<Entry>],
        _winners: &[Entry],
        votes: &HashMap<String, i64>,
    ) -> HashMap<i64, i64> {
        let mut points = HashMap::new();
        for entry in groups.iter().flatten() {
            let count = votes.get(&entry.id).copied().unwrap_or(0);
            if count > 0 {
                *points.entry(entry.user_id).or_insert(0) += count * POINTS_PER_VOTE;
            }
        }
        points
    }
}

struct WinnerBonus;
impl Scoring for WinnerBonus {
    fn round_points(
        &self,
        groups: &[Vec<Entry>],
        winners: &[Entry],
        _votes: &HashMap<String, i64>,
    ) -> HashMap<i64, i64> {
        let mut points = HashMap::new();
        for group in groups.iter().filter(|g| g.len() > 1) {
            if let Some(winner) = winners.iter().find(|w| group.contains(w)) {
                *points.entry(winner.user_id).or_insert(0) += WIN_BONUS;
            }
        }
        points
    }
}

struct GuessSpeed;
impl Scoring for GuessSpeed {
    fn guess_points(&self, rank: i64, remaining: i64, window: i64) -> i64 {
        let window = window.max(1);
        let remaining = remaining.clamp(0, window);
        let speed = (GUESS_SPEED * remaining + window / 2) / window;
        GUESS_BASE + speed + if rank == 1 { FIRST_GUESS_BONUS } else { 0 }
    }
    fn proposer_points(&self, correct: i64) -> i64 {
        PROPOSER_PER_GUESS * correct
    }
}

/// Rules with nothing to vote on group nothing and decide nothing.
struct NoBallot;
impl Ballot for NoBallot {
    fn groups(&self, _entries: &[Entry]) -> Vec<Vec<Entry>> {
        Vec::new()
    }
    fn needed(
        &self,
        _groups: &[Vec<Entry>],
        _votable: &dyn Fn(&Entry) -> bool,
        _per_player: i64,
    ) -> i64 {
        0
    }
    fn allows(
        &self,
        _groups: &[Vec<Entry>],
        _mine: &[String],
        _target: &str,
        _per_player: i64,
    ) -> bool {
        false
    }
}

struct NoWinner;
impl Winner for NoWinner {
    fn winners(&self, _groups: &[Vec<Entry>], _votes: &HashMap<String, i64>) -> Vec<Entry> {
        Vec::new()
    }
}

impl BallotRule {
    pub fn imp(self) -> &'static dyn Ballot {
        match self {
            Self::All => &AllBallot,
            Self::Pairs => &PairsBallot,
            Self::None => &NoBallot,
        }
    }
}

impl WinnerRule {
    pub fn imp(self) -> &'static dyn Winner {
        match self {
            Self::MostVotes => &MostVotes,
            Self::PairWinners => &PairWinners,
            Self::None => &NoWinner,
        }
    }
}

impl ScoringRule {
    pub fn imp(self) -> &'static dyn Scoring {
        match self {
            Self::VotesToPicker => &VotesToPicker,
            Self::WinnerBonus => &WinnerBonus,
            Self::GuessSpeed => &GuessSpeed,
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

    fn rules(ballot: BallotRule, winner: WinnerRule) -> Rules {
        Rules {
            prompt: PromptRule::None,
            collect: CollectRule::Pick,
            ballot,
            winner,
            challenge: ChallengeRule::Listen,
            scoring: ScoringRule::VotesToPicker,
        }
    }

    #[test]
    fn all_ballot_is_one_group_in_submission_order() {
        let entries = vec![entry("a", 1, 1), entry("b", 2, 2)];
        assert_eq!(BallotRule::All.imp().groups(&entries), vec![entries]);
        assert!(BallotRule::All.imp().groups(&[]).is_empty());
    }

    #[test]
    fn all_ballot_needs_the_smaller_of_votes_and_votable_songs() {
        let groups =
            BallotRule::All
                .imp()
                .groups(&[entry("a", 1, 1), entry("b", 2, 2), entry("c", 3, 3)]);
        let not_mine = |e: &Entry| e.user_id != 1;
        assert_eq!(BallotRule::All.imp().needed(&groups, &not_mine, 1), 1);
        assert_eq!(BallotRule::All.imp().needed(&groups, &not_mine, 5), 2);
        assert!(BallotRule::All.imp().allows(&groups, &["b".into()], "c", 2));
        assert!(!BallotRule::All.imp().allows(&groups, &["b".into()], "c", 1));
    }

    #[test]
    fn pairs_ballot_pairs_in_order_with_a_bye_and_one_vote_per_pair() {
        let entries: Vec<_> = (1..=5).map(|i| entry(&format!("s{i}"), i, i)).collect();
        let groups = BallotRule::Pairs.imp().groups(&entries);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0], entries[0..2].to_vec());
        assert_eq!(groups[2], vec![entries[4].clone()]);
        let pairs = BallotRule::Pairs.imp();
        // Player 1 owns s1, so without self votes only the second pair counts.
        let own_pair = |e: &Entry| pairs.eligible(&groups[(e.seq as usize - 1) / 2], e, 1, false);
        assert_eq!(pairs.needed(&groups, &own_pair, 1), 1);
        assert_eq!(pairs.needed(&groups, &|_: &Entry| true, 1), 2);
        assert!(pairs.allows(&groups, &[], "s3", 1));
        assert!(pairs.allows(&groups, &["s1".into()], "s3", 1));
        assert!(
            !pairs.allows(&groups, &["s3".into()], "s4", 1),
            "one vote per pair"
        );
        assert!(!pairs.allows(&groups, &[], "s5", 1), "byes take no votes");
        assert!(
            !pairs.eligible(&groups[0], &groups[0][1], 1, false),
            "own pair"
        );
        assert!(pairs.eligible(&groups[0], &groups[0][1], 1, true));
        assert!(
            BallotRule::All
                .imp()
                .eligible(&groups[0], &groups[0][1], 1, false)
        );
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
    fn pair_winners_take_every_pair_including_byes() {
        let groups = vec![
            vec![entry("a", 1, 1), entry("b", 2, 2)],
            vec![entry("c", 3, 3)],
        ];
        let votes = HashMap::from([("b".to_string(), 1)]);
        let winners = WinnerRule::PairWinners.imp().winners(&groups, &votes);
        assert_eq!(winners, vec![entry("b", 2, 2), entry("c", 3, 3)]);
        let points = ScoringRule::WinnerBonus
            .imp()
            .round_points(&groups, &winners, &votes);
        assert_eq!(
            points,
            HashMap::from([(2, WIN_BONUS)]),
            "the bye scores nothing"
        );
    }

    #[test]
    fn votes_to_picker_scores_every_vote_for_its_picker() {
        let groups = vec![vec![entry("a", 1, 1), entry("b", 2, 2), entry("c", 1, 3)]];
        let votes = HashMap::from([("a".to_string(), 2), ("c".to_string(), 1)]);
        let points = ScoringRule::VotesToPicker
            .imp()
            .round_points(&groups, &[], &votes);
        assert_eq!(points, HashMap::from([(1, 3 * POINTS_PER_VOTE)]));
    }

    #[test]
    fn guess_speed_rewards_speed_first_place_and_known_proposals() {
        let scoring = ScoringRule::GuessSpeed.imp();
        assert_eq!(
            scoring.guess_points(1, 30, 30),
            GUESS_BASE + GUESS_SPEED + FIRST_GUESS_BONUS
        );
        assert_eq!(
            scoring.guess_points(2, 15, 30),
            GUESS_BASE + GUESS_SPEED / 2
        );
        assert_eq!(scoring.guess_points(3, 0, 30), GUESS_BASE);
        assert_eq!(
            scoring.guess_points(2, 99, 30),
            GUESS_BASE + GUESS_SPEED,
            "clamped"
        );
        assert!(scoring.guess_points(2, 20, 30) > scoring.guess_points(2, 10, 30));
        assert_eq!(scoring.proposer_points(3), 3 * PROPOSER_PER_GUESS);
        assert_eq!(
            scoring.proposer_points(0),
            0,
            "an obscure pick nobody gets scores nothing"
        );
        assert_eq!(ScoringRule::VotesToPicker.imp().proposer_points(3), 0);
    }

    #[test]
    fn name_that_tune_rules_only_combine_one_way() {
        let tune = Rules {
            prompt: PromptRule::None,
            collect: CollectRule::Propose,
            ballot: BallotRule::None,
            winner: WinnerRule::None,
            challenge: ChallengeRule::GuessTitle,
            scoring: ScoringRule::GuessSpeed,
        };
        assert!(tune.validate().is_ok());
        for broken in [
            Rules {
                ballot: BallotRule::Pairs,
                ..tune
            },
            Rules {
                winner: WinnerRule::MostVotes,
                ..tune
            },
            Rules {
                challenge: ChallengeRule::Listen,
                ..tune
            },
            Rules {
                scoring: ScoringRule::VotesToPicker,
                ..tune
            },
            Rules {
                collect: CollectRule::Pick,
                ..tune
            },
        ] {
            assert!(broken.validate().is_err(), "{broken:?}");
        }
        let mut pick = rules(BallotRule::All, WinnerRule::MostVotes);
        pick.scoring = ScoringRule::GuessSpeed;
        assert!(pick.validate().is_err());
        assert!(
            rules(BallotRule::None, WinnerRule::None)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn mismatched_ballot_and_winner_rules_are_rejected() {
        assert!(
            rules(BallotRule::All, WinnerRule::MostVotes)
                .validate()
                .is_ok()
        );
        assert!(
            rules(BallotRule::Pairs, WinnerRule::PairWinners)
                .validate()
                .is_ok()
        );
        assert!(
            rules(BallotRule::Pairs, WinnerRule::MostVotes)
                .validate()
                .is_err()
        );
        assert!(
            rules(BallotRule::All, WinnerRule::PairWinners)
                .validate()
                .is_err()
        );
        let mut bonus = rules(BallotRule::All, WinnerRule::MostVotes);
        bonus.scoring = ScoringRule::WinnerBonus;
        assert!(
            bonus.validate().is_ok(),
            "a single ballot can award a winner bonus"
        );
    }
}
