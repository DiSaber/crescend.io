//! Ghost members (closed the browser without leaving) and the open phase: the host can
//! remove them, and after a minute away the game stops waiting for them.
use super::*;

impl Fixture {
    async fn lobby_id(&self, lobby: &Lobby) -> String {
        sqlx::query_scalar("SELECT lobby_id FROM lobby_memberships WHERE id = ?")
            .bind(&lobby.host_membership)
            .fetch_one(&self.database.db_pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn removing_a_ghost_mid_pick_closes_the_phase_early_and_keeps_their_points() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (ann, am) = &lobby.guests[0];
    let (ghost, _) = &lobby.guests[1];
    f.start(&lobby, "party").await;
    f.submit(&lobby.host, &lobby.host_membership, "dQw4w9WgXcQ")
        .await
        .unwrap();
    f.submit(ann, am, "fJ9rUzIMcZQ").await.unwrap();
    assert_eq!(f.view(ann, am).await.phase, Phase::Collect);
    // Only the host can remove.
    assert!(matches!(
        f.database
            .remove_member(ann.id, am, ghost.id, || f.now())
            .await,
        Err(crate::database::lobbies::LobbyError::HostRequired)
    ));
    let removal = f
        .database
        .remove_member(lobby.host.id, &lobby.host_membership, ghost.id, || f.now())
        .await
        .unwrap();
    assert!(matches!(
        removal,
        crate::database::lobbies::Removal::Removed(_, _)
    ));
    let view = f.view(ann, am).await;
    assert_eq!(
        view.phase,
        Phase::Vote,
        "the phase was only waiting on the ghost"
    );
    // The ghost stays on the scoreboard, marked as gone.
    let entry = view
        .scoreboard
        .iter()
        .find(|e| e.user_id == ghost.id.to_string());
    if let Some(entry) = entry {
        assert!(!entry.active);
    }
}

#[tokio::test]
async fn a_member_away_for_a_minute_is_not_waited_for_but_the_host_always_is() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let id = f.lobby_id(&lobby).await;
    let (ann, am) = &lobby.guests[0];
    let (ghost, _) = &lobby.guests[1];
    f.start(&lobby, "party").await;
    f.submit(ann, am, "fJ9rUzIMcZQ").await.unwrap();
    // The host is away too, but the game never skips the host.
    crate::lobbies::presence::mark_away(&id, lobby.host.id, 600);
    crate::lobbies::presence::mark_away(&id, ghost.id, 600);
    f.database.settle_presence(&id, || f.now()).await.unwrap();
    assert_eq!(f.view(ann, am).await.phase, Phase::Collect);
    crate::lobbies::presence::forget(&id, lobby.host.id);
    f.submit(&lobby.host, &lobby.host_membership, "dQw4w9WgXcQ")
        .await
        .unwrap();
    assert_eq!(f.view(ann, am).await.phase, Phase::Vote);
}

#[tokio::test]
async fn a_member_away_for_less_than_a_minute_is_still_waited_for() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let id = f.lobby_id(&lobby).await;
    let (ann, am) = &lobby.guests[0];
    let (ghost, _) = &lobby.guests[1];
    f.start(&lobby, "party").await;
    f.submit(&lobby.host, &lobby.host_membership, "dQw4w9WgXcQ")
        .await
        .unwrap();
    f.submit(ann, am, "fJ9rUzIMcZQ").await.unwrap();
    crate::lobbies::presence::mark_away(&id, ghost.id, 45);
    let revision = f.revision(&lobby).await;
    f.database.settle_presence(&id, || f.now()).await.unwrap();
    assert_eq!(f.view(ann, am).await.phase, Phase::Collect);
    crate::lobbies::presence::mark_away(&id, ghost.id, 61);
    let settled = f
        .database
        .settle_presence(&id, || f.now())
        .await
        .unwrap()
        .unwrap();
    assert!(settled > revision);
    assert_eq!(f.view(ann, am).await.phase, Phase::Vote);
}
