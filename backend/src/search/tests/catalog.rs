use super::*;

#[tokio::test]
async fn seeds_normalization_ranking_restart_and_literal_matching() {
    let f = Fixture::new().await;
    f.database
        .import_starters(
            r#"[
        {"video_id":"aaaaaa","title":"Café́ μάιος","artist":"Beyoncé","thumbnail_url":null},
        {"video_id":"bbbbbb","title":"Café́ μάιος","artist":"Beyoncé","thumbnail_url":null},
        {"video_id":"bad!","title":"bad","artist":"bad"},
        {"video_id":"aaaaaa","title":"Café́ μάιος","artist":"Beyoncé","thumbnail_url":null}
    ]"#,
            NOW,
        )
        .await
        .unwrap();
    for q in ["cafe", "bey", "μαι", "  CAFÉ  ", "cafe beyon"] {
        let results = f
            .database
            .search_catalog(&super::super::normalize::normalize(q), NOW)
            .await
            .unwrap();
        assert_eq!(
            results
                .iter()
                .map(|s| s.video_id.as_str())
                .collect::<Vec<_>>(),
            ["aaaaaa", "bbbbbb"]
        );
    }
    sqlx::query("UPDATE catalog_songs SET submission_count=3 WHERE video_id='bbbbbb'")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(
        f.database.search_catalog("cafe", NOW).await.unwrap()[0].video_id,
        "bbbbbb"
    );
    for q in ["***", "OR NOT NEAR", "title:missing"] {
        assert!(f.database.search_catalog(q, NOW).await.unwrap().is_empty());
    }
    let reopened = Fixture::connect(&f.directory.path().join("search.db")).await;
    reopened.create_tables().await.unwrap();
    assert_eq!(reopened.search_catalog("cafe", NOW).await.unwrap().len(), 2);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_songs")
        .fetch_one(&reopened.db_pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn fts_triggers_and_source_expiry_preserve_identity() {
    let f = Fixture::new().await;
    f.database
        .import_starters(
            r#"[{"video_id":"aaaaaa","title":"starter","artist":"artist"}]"#,
            NOW,
        )
        .await
        .unwrap();
    f.database
        .save_search("provider", &[song("aaaaaa", "provider", "channel")], NOW)
        .await
        .unwrap();
    assert!(
        f.database
            .search_catalog("starter", NOW)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.database
            .search_catalog("provider", NOW)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        f.database
            .search_catalog("provider", NOW + 29 * 86400)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.database
            .search_catalog("starter", NOW + 29 * 86400)
            .await
            .unwrap()
            .len(),
        1
    );
    f.database.cleanup_catalog(NOW + 29 * 86400).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_songs")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    sqlx::query("UPDATE catalog_metadata SET normalized_title='updated' WHERE source='starter'")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert!(
        f.database
            .search_catalog("starter", NOW)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.database
            .search_catalog("updated", NOW)
            .await
            .unwrap()
            .len(),
        1
    );
    sqlx::query("DELETE FROM catalog_metadata")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert!(
        f.database
            .search_catalog("updated", NOW)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn stale_writes_relevance_and_seed_reimports_preserve_counts() {
    let f = Fixture::new().await;
    f.database.import_starters(r#"[null,3,"bad",{"video_id":"aaaaaa","title":"echo","artist":"one"},{"video_id":"bbbbbb","title":"echo long title full words","artist":"two"}]"#,NOW).await.unwrap();
    sqlx::query("UPDATE catalog_songs SET submission_count=1000 WHERE video_id='bbbbbb'")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(
        f.database.search_catalog("echo", NOW).await.unwrap()[0].video_id,
        "aaaaaa"
    );
    f.database
        .save_search(
            "fresh",
            &[song("aaaaaa", "New Café &amp;", "channel")],
            NOW + 10,
        )
        .await
        .unwrap();
    f.database
        .save_search("older", &[song("aaaaaa", "Old title", "channel")], NOW)
        .await
        .unwrap();
    f.database
        .import_starters(
            r#"[{"video_id":"aaaaaa","title":"seed replacement","artist":"seed"}]"#,
            NOW + 20,
        )
        .await
        .unwrap();
    assert_eq!(
        f.database.search_catalog("new", NOW + 20).await.unwrap()[0].title,
        "New Café &amp;"
    );
    assert!(
        f.database
            .search_catalog("old", NOW + 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        f.database
            .search_catalog("replacement", NOW + 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.database
            .search_catalog("replacement", NOW + 10 + 29 * 86400)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT submission_count FROM catalog_songs WHERE video_id='aaaaaa'"
        )
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap(),
        0
    );
    assert!(
        f.database
            .save_search(
                "overflow",
                &[song("aaaaaa", "overflow", "artist")],
                i64::MAX
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn invalid_seed_shape_and_storage_rollback() {
    let f = Fixture::new().await;
    f.database.import_starters("[]", NOW).await.unwrap();
    assert!(f.database.import_starters("{}", NOW).await.is_err());
    sqlx::query("CREATE TRIGGER fail_cache BEFORE INSERT ON song_search_cache BEGIN SELECT RAISE(FAIL,'injected'); END").execute(&f.database.db_pool).await.unwrap();
    assert!(
        f.database
            .save_search("bad", &[song("aaaaaa", "Must roll back", "artist")], NOW)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_songs")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn shipped_starter_songs_are_all_valid_and_searchable() {
    // Invalid records are skipped at startup with only a log line, so check the file here.
    let input = include_str!("../../../resources/starter-songs.json");
    let entries: Vec<serde_json::Value> = serde_json::from_str(input).unwrap();
    let mut ids = std::collections::HashSet::new();
    for mut value in entries.clone() {
        let id = value["video_id"].as_str().unwrap().to_owned();
        assert!(ids.insert(id.clone()), "duplicate {id}");
        value["watch_url"] = format!("https://www.youtube.com/watch?v={id}").into();
        let snapshot: crate::models::search::Snapshot = serde_json::from_value(value).unwrap();
        assert!(snapshot.valid(), "invalid starter song {id}");
    }
    let f = Fixture::new().await;
    f.database.import_starters(input, NOW).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_songs")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(count, entries.len() as i64);
    for (q, id) in [
        ("crab rave", "LDU_Txk06tM"),
        ("beyonce", "ViwtNLUqkMY"),
        ("senorita", "Pkh8UtuejGw"),
    ] {
        let results = f
            .database
            .search_catalog(&super::super::normalize::normalize(q), NOW)
            .await
            .unwrap();
        assert_eq!(
            results.first().map(|s| s.video_id.as_str()),
            Some(id),
            "{q}"
        );
    }
}
