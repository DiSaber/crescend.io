use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_with::{DeserializeFromStr, SerializeDisplay};
use sqlx::{
    Database, Decode, Encode, Sqlite, Type,
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteTypeInfo, SqliteValueRef},
};

/// Identifier for a lobby.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, SerializeDisplay, DeserializeFromStr, utoipa::ToSchema,
)]
#[schema(value_type = String, pattern = "^[0-9a-f]{6}$", example = "a1b2c3")]
pub struct LobbyId([u8; 3]);

impl LobbyId {
    pub fn new(bytes: [u8; 3]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for LobbyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}", base16ct::HexDisplay(&self.0))
    }
}

impl FromStr for LobbyId {
    type Err = base16ct::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut buf = [0; 3];
        base16ct::mixed::decode(s, &mut buf)?;
        Ok(LobbyId::new(buf))
    }
}

impl Type<Sqlite> for LobbyId {
    fn type_info() -> SqliteTypeInfo {
        <Vec<u8> as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <Vec<u8> as Type<Sqlite>>::compatible(ty)
    }
}

impl Encode<'_, Sqlite> for LobbyId {
    fn encode_by_ref(
        &self,
        buffer: &mut <Sqlite as Database>::ArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <&[u8] as Encode<Sqlite>>::encode(&self.0, buffer)
    }
}

impl<'r> Decode<'r, Sqlite> for LobbyId {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let bytes = <Vec<u8> as Decode<Sqlite>>::decode(value)?;
        let bytes: [u8; 3] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            format!("expected a 3 byte lobby id, got {} bytes", bytes.len())
        })?;

        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Lobby {
    pub id: LobbyId,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientLobby {
    pub id: LobbyId,
    pub created_at: DateTime<Utc>,
}

impl From<Lobby> for ClientLobby {
    fn from(lobby: Lobby) -> Self {
        Self {
            id: lobby.id,
            created_at: lobby.created_at,
        }
    }
}
