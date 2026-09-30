use std::time::Duration;

use rusqlite::Connection;

use super::{Database, LATEST_SCHEMA_VERSION};

fn database() -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
    (directory, database)
}

#[test]
fn empty_database_migrates_once_and_passes_integrity_check() {
    let (_directory, database) = database();
    database.migrate().unwrap();
    database.migrate().unwrap();

    assert_eq!(database.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    assert_eq!(database.integrity_check().unwrap(), "ok");
    assert_eq!(
        database.application_table_names().unwrap(),
        vec![
            "app_preferences",
            "diagnostic_events",
            "embedding_spaces",
            "material_chunks",
            "material_chunks_fts",
            "material_documents",
            "material_file_cleanup",
            "materials",
            "practice_plans",
            "practice_reports",
            "runtime_snapshots",
            "schema_migrations",
            "session_citations",
            "session_events",
            "session_turns",
            "sessions",
            "voice_references",
        ]
    );
}

#[test]
fn connection_enables_required_sqlite_safety_settings() {
    let (_directory, database) = database();
    assert_eq!(database.pragma_string("journal_mode").unwrap(), "wal");
    assert_eq!(database.pragma_i64("foreign_keys").unwrap(), 1);
    assert_eq!(database.pragma_i64("busy_timeout").unwrap(), 5_000);
}

#[test]
fn future_schema_version_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("future.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(
        "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL) STRICT;
         INSERT INTO schema_migrations(version, applied_at) VALUES (99, 'future');",
    ).unwrap();
    drop(connection);

    let database = Database::open(path).unwrap();
    assert_eq!(
        database.migrate().unwrap_err().code(),
        "DATABASE_VERSION_NEWER"
    );
}

#[test]
fn foundation_schema_cannot_store_secret_named_columns() {
    let (_directory, database) = database();
    database.migrate().unwrap();
    let forbidden = database
        .column_names()
        .unwrap()
        .into_iter()
        .filter(|name| {
            let name = name.to_ascii_lowercase();
            ["api_key", "token", "secret", "password"]
                .iter()
                .any(|part| name.contains(part))
        })
        .collect::<Vec<_>>();
    assert!(
        forbidden.is_empty(),
        "forbidden secret columns: {forbidden:?}"
    );
}

#[test]
fn configured_busy_timeout_matches_five_seconds() {
    let (_directory, database) = database();
    assert_eq!(database.busy_timeout(), Duration::from_secs(5));
}

#[test]
fn foundation_database_migrates_to_materials_schema() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v1.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("../../migrations/0001_foundation.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, '2026-09-04T00:00:00Z')",
            [],
        )
        .unwrap();
    drop(connection);

    let database = Database::open(path).unwrap();
    database.migrate().unwrap();

    assert_eq!(database.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    assert!(
        database
            .application_table_names()
            .unwrap()
            .contains(&"materials".to_owned())
    );
}

#[test]
fn materials_schema_migrates_to_cascade_session_schema() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v2.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("../../migrations/0001_foundation.sql"))
        .unwrap();
    connection
        .execute_batch(include_str!("../../migrations/0002_materials.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, '2026-09-04T00:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (2, '2026-09-04T12:00:00Z')",
            [],
        )
        .unwrap();
    drop(connection);

    let database = Database::open(path).unwrap();
    database.migrate().unwrap();

    assert_eq!(database.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    let tables = database.application_table_names().unwrap();
    for name in [
        "sessions",
        "session_turns",
        "session_citations",
        "session_events",
        "runtime_snapshots",
    ] {
        assert!(tables.contains(&name.to_owned()), "missing table {name}");
    }
}

#[test]
fn session_check_rejects_unknown_status_and_transport() {
    let (_directory, database) = database();
    database.migrate().unwrap();
    database
        .execute_batch(
            "INSERT INTO sessions(
                id, status, role_profile_id, voice_route_id, transport_mode, updated_at
             ) VALUES ('session-ok', 'idle', '', '', 'direct', '2026-09-05T00:00:00Z');",
        )
        .unwrap();

    assert!(
        database
            .execute_batch(
                "INSERT INTO sessions(
                    id, status, role_profile_id, voice_route_id, transport_mode, updated_at
                 ) VALUES ('session-bad-status', 'unknown', '', '', 'direct', '2026-09-05T00:00:00Z');"
            )
            .is_err()
    );
    assert!(
        database
            .execute_batch(
                "INSERT INTO sessions(
                    id, status, role_profile_id, voice_route_id, transport_mode, updated_at
                 ) VALUES ('session-bad-transport', 'idle', '', '', 'webrtc', '2026-09-05T00:00:00Z');"
            )
            .is_err()
    );
}

#[test]
fn session_check_allows_livekit_transport() {
    let (_directory, database) = database();
    database.migrate().unwrap();
    database
        .execute_batch(
            "INSERT INTO sessions(
                id, status, role_profile_id, voice_route_id, transport_mode, updated_at
             ) VALUES ('session-livekit', 'idle', '', '', 'livekit', '2026-09-05T00:00:00Z');
             INSERT INTO runtime_snapshots(
                id, session_id, app_version, config_revision, provider_ids, model_ids,
                voice_route_id, transport_mode, role_hash, knowledge_fingerprint, created_at
             ) VALUES (
                'snap-livekit', 'session-livekit', '0.1.0', '1', '[]', '[]',
                '', 'livekit', '', '', '2026-09-05T00:00:00Z'
             );",
        )
        .unwrap();
    assert_eq!(
        database
            .query_string("SELECT transport_mode FROM sessions WHERE id='session-livekit'")
            .unwrap(),
        "livekit"
    );
    assert_eq!(
        database
            .query_string("SELECT transport_mode FROM runtime_snapshots WHERE id='snap-livekit'")
            .unwrap(),
        "livekit"
    );
}

#[test]
fn session_schema_accepts_only_the_application_structured_event_kinds() {
    let (_directory, database) = database();
    database.migrate().unwrap();
    database.execute_batch(
        "INSERT INTO sessions(id,status,role_profile_id,voice_route_id,transport_mode,updated_at)
         VALUES ('event-session','listening','','','direct','2026-09-05T00:00:00Z');"
    ).unwrap();
    for (seq, kind) in ["web_sources", "turn_meta", "scenario"]
        .into_iter()
        .enumerate()
    {
        database
            .execute_batch(&format!(
                "INSERT INTO session_events(session_id,seq,kind,payload,created_at)
             VALUES ('event-session',{seq},'{kind}','{{}}','2026-09-05T00:00:00Z');"
            ))
            .unwrap_or_else(|_| panic!("structured event kind must be accepted: {kind}"));
    }
    assert!(
        database
            .execute_batch(
                "INSERT INTO session_events(session_id,seq,kind,payload,created_at)
         VALUES ('event-session',99,'arbitrary_plugin_event','{}','2026-09-05T00:00:00Z');"
            )
            .is_err()
    );
}

#[test]
fn cascade_session_schema_migrates_to_livekit_transport() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v3.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("../../migrations/0001_foundation.sql"))
        .unwrap();
    connection
        .execute_batch(include_str!("../../migrations/0002_materials.sql"))
        .unwrap();
    connection
        .execute_batch(include_str!("../../migrations/0003_sessions.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, '2026-09-04T00:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (2, '2026-09-04T12:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (3, '2026-09-05T00:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .execute_batch(
            "INSERT INTO sessions(
                id, status, role_profile_id, voice_route_id, transport_mode, updated_at
             ) VALUES ('session-direct', 'listening', 'role-1', 'route-1', 'direct', '2026-09-05T00:00:00Z');
             INSERT INTO session_turns(
                id, session_id, turn_index, user_text, assistant_text, materials_used, created_at
             ) VALUES ('turn-1', 'session-direct', 0, '问', '答', 0, '2026-09-05T00:00:00Z');
             INSERT INTO runtime_snapshots(
                id, session_id, app_version, config_revision, provider_ids, model_ids,
                voice_route_id, transport_mode, role_hash, knowledge_fingerprint, created_at
             ) VALUES (
                'snap-direct', 'session-direct', '0.1.0', '1', '[]', '[]',
                'route-1', 'direct', 'hash', '', '2026-09-05T00:00:00Z'
             );",
        )
        .unwrap();
    drop(connection);

    let database = Database::open(path).unwrap();
    database.migrate().unwrap();

    assert_eq!(database.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    assert_eq!(database.integrity_check().unwrap(), "ok");
    assert_eq!(
        database
            .query_string("SELECT transport_mode FROM sessions WHERE id='session-direct'")
            .unwrap(),
        "direct"
    );
    assert_eq!(
        database
            .query_string("SELECT id FROM session_turns WHERE session_id='session-direct'")
            .unwrap(),
        "turn-1"
    );
    assert_eq!(
        database
            .query_string("SELECT transport_mode FROM runtime_snapshots WHERE id='snap-direct'")
            .unwrap(),
        "direct"
    );
    database
        .execute_batch(
            "INSERT INTO sessions(
                id, status, role_profile_id, voice_route_id, transport_mode, updated_at
             ) VALUES ('session-livekit', 'idle', '', '', 'livekit', '2026-09-06T00:00:00Z');",
        )
        .unwrap();
    assert!(
        database
            .execute_batch(
                "INSERT INTO sessions(
                    id, status, role_profile_id, voice_route_id, transport_mode, updated_at
                 ) VALUES ('session-bad-transport', 'idle', '', '', 'webrtc', '2026-09-06T00:00:00Z');"
            )
            .is_err()
    );
    database
        .execute_batch("DELETE FROM sessions WHERE id='session-direct';")
        .unwrap();
    assert!(
        database
            .query_string("SELECT id FROM session_turns WHERE session_id='session-direct'")
            .is_err()
    );
    assert!(
        database
            .query_string("SELECT id FROM runtime_snapshots WHERE session_id='session-direct'")
            .is_err()
    );
}

#[test]
fn migrate_exposes_sqlite_vec_version_0_1_9() {
    let (_directory, database) = database();
    database.migrate().unwrap();
    let version = database.query_string("SELECT vec_version()").unwrap();
    assert!(
        version.starts_with("v0.1.9"),
        "expected sqlite-vec 0.1.9, got {version:?}"
    );
}

#[test]
fn materials_fts5_matches_chinese_trigrams() {
    let (_directory, database) = database();
    database.migrate().unwrap();
    database
        .execute_batch(
            "INSERT INTO material_chunks_fts(content, material_id, chunk_id)
             VALUES ('负责订单服务与 Kafka 链路', 'material-1', 'chunk-1');",
        )
        .unwrap();

    assert_eq!(
        database
            .query_string(
                "SELECT chunk_id FROM material_chunks_fts WHERE material_chunks_fts MATCH '订单服务'"
            )
            .unwrap(),
        "chunk-1"
    );
}

#[test]
fn session_schema_includes_context_summary_columns() {
    let (_directory, database) = database();
    database.migrate().unwrap();

    let columns = database
        .query_strings("SELECT name FROM pragma_table_info('sessions')")
        .unwrap();
    assert!(columns.contains(&"context_summary".to_owned()));
    assert!(columns.contains(&"context_summary_turn_index".to_owned()));

    database
        .execute_batch(
            "INSERT INTO sessions(
                id, status, role_profile_id, voice_route_id, transport_mode, updated_at
             ) VALUES ('session-summary', 'idle', '', '', 'direct', '2026-09-26T00:00:00Z');",
        )
        .unwrap();
    assert_eq!(
        database
            .query_string(
                "SELECT COALESCE(context_summary, '<null>')
                 FROM sessions WHERE id = 'session-summary'"
            )
            .unwrap(),
        "<null>"
    );
    assert_eq!(
        database
            .query_string(
                "SELECT CAST(context_summary_turn_index AS TEXT)
                 FROM sessions WHERE id = 'session-summary'"
            )
            .unwrap(),
        "-1"
    );
}

#[test]
fn voice_reference_schema_includes_target_model_column() {
    let (_directory, database) = database();
    database.migrate().unwrap();

    let columns = database
        .query_strings("SELECT name FROM pragma_table_info('voice_references')")
        .unwrap();
    assert!(columns.contains(&"target_model".to_owned()));

    database
        .execute_batch(
            "INSERT INTO voice_references(
                id, name, mime_type, byte_size, audio, created_at, updated_at
             ) VALUES ('ref-target', '示例', 'audio/wav', 4, x'00000000', '2026-09-27T00:00:00Z', '2026-09-27T00:00:00Z');",
        )
        .unwrap();
    assert_eq!(
        database
            .query_string(
                "SELECT COALESCE(target_model, '<null>')
                 FROM voice_references WHERE id = 'ref-target'"
            )
            .unwrap(),
        "<null>"
    );
}

#[test]
fn practice_schema_migrates_from_voice_reference_target_model() {
    let (_directory, database) = database();
    database.migrate().unwrap();

    assert_eq!(database.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    let plan_columns = database
        .query_strings("SELECT name FROM pragma_table_info('practice_plans')")
        .unwrap();
    for column in [
        "id",
        "title",
        "position",
        "interviewer_style",
        "difficulty",
        "question_count",
        "questions_json",
        "created_at",
        "updated_at",
    ] {
        assert!(
            plan_columns.contains(&column.to_owned()),
            "missing {column}"
        );
    }
    let report_columns = database
        .query_strings("SELECT name FROM pragma_table_info('practice_reports')")
        .unwrap();
    for column in [
        "id",
        "plan_id",
        "position",
        "interviewer_style",
        "total_score",
        "dimensions_json",
        "objective_json",
        "report_json",
        "created_at",
    ] {
        assert!(
            report_columns.contains(&column.to_owned()),
            "missing {column}"
        );
    }

    // practice_reports 的主键级联自 sessions：删会话删报告。
    database
        .execute_batch(
            "INSERT INTO sessions(id, status, role_profile_id, voice_route_id, transport_mode, updated_at)
             VALUES ('sess-p', 'completed', 'role-1', 'route-1', 'direct', '2026-10-01T00:00:00Z');
             INSERT INTO practice_reports(id, plan_id, position, total_score, report_json, created_at)
             VALUES ('sess-p', 'plan-1', '后端工程师', 4.0, '{}', '2026-10-01T00:00:00Z');",
        )
        .unwrap();
    database
        .execute_batch("DELETE FROM sessions WHERE id = 'sess-p';")
        .unwrap();
    let orphans: i64 = database
        .with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM practice_reports WHERE id = 'sess-p'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(orphans, 0);

    // 事件 kind 白名单纳入 practice_meta。
    database
        .execute_batch(
            "INSERT INTO sessions(id, status, role_profile_id, voice_route_id, transport_mode, updated_at)
             VALUES ('sess-q', 'completed', 'role-1', 'route-1', 'direct', '2026-10-01T00:00:00Z');
             INSERT INTO session_events(session_id, seq, kind, payload, created_at)
             VALUES ('sess-q', 0, 'practice_meta', '{}', '2026-10-01T00:00:00Z');",
        )
        .unwrap();
    assert!(
        database
            .execute_batch(
                "INSERT INTO session_events(session_id, seq, kind, payload, created_at)
             VALUES ('sess-q', 1, 'unknown_kind', '{}', '2026-10-01T00:00:00Z');"
            )
            .is_err()
    );
}
