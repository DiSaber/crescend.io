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

    /// An ID that came from the database.
    pub(crate) fn from_trusted(id: String) -> Self {
        Self(id)
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

/// Characters allowed in a generated join code. This is the Crockford-style
/// ambiguity-reduced set: it keeps the digits `0` and `1` but drops the letters
/// `O`, `I` and `L`, which are easily confused with them (or each other) when a
/// code is read off a projector. 32 characters divide 256 exactly, so every
/// byte maps uniformly with no rejection sampling required.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Fold a visually ambiguous character onto the canonical character that is
/// generated in its place. `O` -> `0`, `I`/`L` -> `1`. The canonical set above
/// never emits `O`, `I` or `L`, so this folding cannot make two distinct codes
/// collide: it only maps look-alikes that could not both have been generated
/// onto the single code that was. Legacy lobbies created before this change may
/// still contain `O`/`I`/`L` because the old alphabet emitted them; folding both
/// the stored code and the typed code at lookup time keeps those lobbies joinable.
fn canonicalize_char(b: u8) -> u8 {
    match b {
        b'O' => b'0',
        b'I' | b'L' => b'1',
        other => other,
    }
}

impl LobbyAllocation {
    pub fn random() -> Result<Self, ()> {
        let mut bytes = [0u8; 64];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ())?;
        // 256 is divisible by 32: every byte value maps to exactly one alphabet
        // character, so there is no modulo bias and no rejected bytes.
        let join_code: String = bytes[32..]
            .iter()
            .copied()
            .take(6)
            .map(|b| ALPHABET[usize::from(b % 32)] as char)
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
    /// This member's name in this lobby: 1-24 characters, unique in the lobby.
    /// Plain text chosen by the member; render it as text, never as HTML.
    pub nickname: String,
    #[schema(value_type = String, format = DateTime)]
    pub joined_at: DateTime<Utc>,
    /// True when the member has had no open event stream for over a minute, so the
    /// game and the skip-vote majority stop waiting for them. Never true for the host.
    pub away: bool,
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
    /// Six letters or digits, ignoring case and any spaces or dashes.
    pub join_code: String,
    /// Optional name for this lobby: 1-24 characters after trimming, no control
    /// characters. A clash in the lobby gets " 2", " 3", ... appended. When absent the
    /// member keeps their name in this lobby, or gets their last nickname.
    #[serde(default)]
    pub nickname: Option<String>,
}

/// Optional body of lobby creation; an empty body is still accepted.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLobbyRequest {
    /// The host's name in the new lobby, with the same rules as joining.
    pub nickname: String,
}

impl JoinLobbyRequest {
    pub fn normalized_code(&self) -> Option<String> {
        normalize_code(&self.join_code)
    }
}

/// Six letters or digits, ignoring case and any spaces or dashes ("abc 123", "ABC-123").
/// Look-alike letters are folded to their canonical digits (`O`->`0`, `I`/`L`->`1`)
/// so a code typed from an old or ambiguous code still matches the stored lobby.
pub fn normalize_code(raw: &str) -> Option<String> {
    let code: String = raw
        .chars()
        .filter(|c| !(c.is_whitespace() || matches!(c, '-' | '\u{2010}'..='\u{2015}')))
        .collect();
    (code.len() == 6 && code.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| {
        code.bytes()
            .map(|b| canonicalize_char(b.to_ascii_uppercase()) as char)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generated code must never contain a look-alike character.
    #[test]
    fn generated_codes_avoid_look_alike_characters() {
        const FORBIDDEN: &[u8] = b"OIL";
        for _ in 0..2048 {
            let allocation = LobbyAllocation::random().expect("entropy available");
            assert_eq!(allocation.join_code.len(), 6);
            assert!(
                allocation
                    .join_code
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            );
            assert!(
                allocation
                    .join_code
                    .bytes()
                    .all(|b| !FORBIDDEN.contains(&b)),
                "code contained a look-alike character: {}",
                allocation.join_code
            );
        }
    }

    /// Typing a look-alike of a character must resolve to the same canonical code,
    /// so codes from lobbies created with the old alphabet still join.
    #[test]
    fn typed_look_alikes_normalize_to_canonical_code() {
        // O <-> 0 and I/L <-> 1 folds.
        assert_eq!(normalize_code("A0B1C0"), Some("A0B1C0".into()));
        assert_eq!(normalize_code("AOBICL"), Some("A0B1C1".into()));
        assert_eq!(normalize_code("ao00l1"), Some("A00011".into()));
        // Spaces and dashes are ignored; the remaining six characters are folded.
        assert_eq!(normalize_code("AO-BI CL"), Some("A0B1C1".into()));
        // A legacy code "A0OI1L" folds deterministically to "A00111".
        let legacy = "A0OI1L";
        let canonical = normalize_code(legacy).unwrap();
        assert_eq!(canonical, "A00111");
        assert_eq!(Some(canonical.clone()), normalize_code(&canonical));
    }
}
