use chrono::{DateTime, Utc};
use rand::TryRng;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Stable lobby identity, independent of the short invitation code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::Type, ToSchema)]
#[serde(transparent)]
#[sqlx(transparent)]
#[schema(value_type = String, pattern = "^[0-9a-f]{32}$")]
pub struct LobbyId(String);

impl LobbyId {
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(base16ct::lower::encode_string(&bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifies one membership generation, not a user or invitation code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::Type, ToSchema)]
#[serde(transparent)]
#[sqlx(transparent)]
#[schema(value_type = String, pattern = "^[0-9a-f]{32}$")]
pub struct MembershipId(String);

impl MembershipId {
    pub fn random() -> Result<Self, ()> {
        let mut bytes = [0; 16];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ())?;
        Ok(Self::from_bytes(bytes))
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(base16ct::lower::encode_string(&bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Candidate values are generated inside the transaction's bounded retry loop.
#[derive(Debug, Clone)]
pub struct LobbyAllocation {
    pub lobby_id: LobbyId,
    pub membership_id: MembershipId,
    pub join_code: String,
}

impl LobbyAllocation {
    pub fn random() -> Result<Self, ()> {
        let mut bytes = [0u8; 64];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ())?;
        const ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        // 252 is divisible by 36: rejecting the last four byte values avoids modulo bias.
        // A bounded batch also fails closed instead of spinning if entropy is unusable.
        let join_code: String = bytes[32..]
            .iter()
            .copied()
            .filter(|b| *b < 252)
            .take(6)
            .map(|b| ALPHABET[usize::from(b % 36)] as char)
            .collect();
        if join_code.len() != 6 {
            return Err(());
        }
        Ok(Self {
            lobby_id: LobbyId::from_bytes(bytes[..16].try_into().unwrap()),
            membership_id: MembershipId::from_bytes(bytes[16..32].try_into().unwrap()),
            join_code,
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemberRole {
    Owner,
    Member,
}

impl MemberRole {
    pub fn for_user(user_id: i64, owner_user_id: i64) -> Self {
        if user_id == owner_user_id {
            Self::Owner
        } else {
            Self::Member
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemberView {
    #[schema(pattern = "^[0-9]+$")]
    pub user_id: String,
    pub role: MemberRole,
    #[schema(value_type = String, format = DateTime)]
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LobbyView {
    pub id: LobbyId,
    #[schema(pattern = "^[A-Z0-9]{6}$")]
    pub join_code: String,
    #[schema(pattern = "^[0-9]+$")]
    pub owner_user_id: String,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<Utc>,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,
    #[schema(pattern = "^[1-9][0-9]*$")]
    pub revision: String,
    pub members: Vec<MemberView>,
}

/// Both values are present for an active membership, or both absent for empty state.
#[derive(Debug, Serialize, ToSchema)]
pub struct CurrentLobby {
    pub membership_id: Option<MembershipId>,
    pub lobby: Option<LobbyView>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JoinLobbyRequest {
    /// Six ASCII letters/digits, ignoring surrounding ASCII whitespace and case.
    pub join_code: String,
}

impl JoinLobbyRequest {
    pub fn normalized_code(&self) -> Option<String> {
        let code = self.join_code.trim_matches(|c: char| {
            matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{000b}' | '\u{000c}')
        });
        (code.len() == 6 && code.bytes().all(|b| b.is_ascii_alphanumeric()))
            .then(|| code.to_ascii_uppercase())
    }
}
