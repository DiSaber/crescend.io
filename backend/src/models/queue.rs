//! Stable queue occurrence identities and public representations.
use chrono::{DateTime, Datelike, Utc};
use rand::TryRng;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Queue and retry identities are "128 bits represented by 32 lowercase hexadecimal characters".
/// An occurrence identity is independent of its canonical YouTube video identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::Type, ToSchema)]
#[serde(transparent)]
#[sqlx(transparent)]
#[schema(value_type = String, pattern = "^[0-9a-f]{32}$")]
pub struct EntryId(String);

pub fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl EntryId {
    pub fn parse(value: String) -> Result<Self, ()> {
        valid_id(&value).then_some(Self(value)).ok_or(())
    }

    pub fn random() -> Result<Self, ()> {
        let mut bytes = [0; 16];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ())?;
        Ok(Self(base16ct::lower::encode_string(&bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// "positive checked i64, serialized as a decimal string". Overflow is temporary unavailability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToSchema)]
#[schema(value_type = String, pattern = "^[1-9][0-9]*$")]
pub struct Revision(i64);

impl Revision {
    pub fn new(value: i64) -> Result<Self, ()> {
        (value > 0).then_some(Self(value)).ok_or(())
    }
    pub fn parse(value: &str) -> Result<Self, ()> {
        if value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(());
        }
        Self::new(value.parse().map_err(|_| ())?)
    }
    pub fn next(self) -> Result<Self, ()> {
        Self::new(self.0.checked_add(1).ok_or(())?)
    }
    pub fn value(self) -> i64 {
        self.0
    }
}

impl Serialize for Revision {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?)
            .map_err(|_| serde::de::Error::custom("invalid revision"))
    }
}

/// "Timestamps use existing UTC second precision"; SQLx keeps the existing canonical +00:00 encoding.
/// "Account IDs remain decimal strings publicly" in queue response DTOs.
pub fn timestamp(seconds: i64) -> Result<DateTime<Utc>, ()> {
    DateTime::from_timestamp(seconds, 0)
        .filter(|t| (0..=9999).contains(&t.year()))
        .ok_or(())
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitRequest {
    pub url: String,
    pub request_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct QueueEntry {
    pub id: EntryId,
    pub video: crate::models::youtube::VideoMetadata,
    pub submitted_by_user_id: String,
    #[schema(value_type = String, format = DateTime)]
    pub submitted_at: DateTime<Utc>,
    pub active_vote_count: i64,
    pub has_voted: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VotingView {
    pub mode: String,
    pub fixed_count: Option<i64>,
    pub active_guest_count: i64,
    pub effective_threshold: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct QueueView {
    pub membership_id: String,
    pub lobby_id: String,
    pub revision: Revision,
    pub current: Option<QueueEntry>,
    pub upcoming: Vec<QueueEntry>,
    pub voting: VotingView,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MutationResponse {
    pub changed: bool,
    pub queue: QueueView,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SubmissionResponse {
    pub changed: bool,
    pub entry_id: EntryId,
    pub active: bool,
    pub queue: QueueView,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdvanceReason {
    Ended,
    Skip,
    PlaybackErrorSkip,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AdvanceRequest {
    pub expected_entry_id: String,
    pub reason: AdvanceReason,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum PolicyRequest {
    Majority {},
    Fixed {
        #[schema(minimum = 1, maximum = 2147483647)]
        count: i32,
    },
}

impl PolicyRequest {
    pub fn parts(&self) -> Result<(&'static str, Option<i64>), ()> {
        match self {
            Self::Majority {} => Ok(("majority", None)),
            Self::Fixed { count } if *count > 0 => Ok(("fixed", Some(i64::from(*count)))),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    pub expected_revision: Revision,
    #[schema(minimum = 0)]
    pub to_index: u64,
}
