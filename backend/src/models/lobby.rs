use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};

/// Identifier for a lobby.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct LobbyId(i64);

impl LobbyId {
    pub fn new(id: i64) -> Self {
        Self(id)
    }
}

/// Join code bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, sqlx::Type)]
#[sqlx(transparent)]
pub struct JoinCode(Vec<u8>);

impl JoinCode {
    pub fn new(bytes: [u8; 3]) -> Self {
        Self(bytes.to_vec())
    }
}

impl fmt::Display for JoinCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}", base16ct::HexDisplay(&self.0))
    }
}

impl FromStr for JoinCode {
    type Err = base16ct::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut buf = [0; 3];
        base16ct::mixed::decode(s, &mut buf)?;
        Ok(JoinCode::new(buf))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Lobby {
    pub id: LobbyId,
    pub join_code: JoinCode,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientLobby {
    pub id: LobbyId,
    #[serde_as(as = "DisplayFromStr")]
    pub join_code: JoinCode,
    pub created_at: DateTime<Utc>,
}

impl From<Lobby> for ClientLobby {
    fn from(lobby: Lobby) -> Self {
        Self {
            id: lobby.id,
            join_code: lobby.join_code,
            created_at: lobby.created_at,
        }
    }
}
