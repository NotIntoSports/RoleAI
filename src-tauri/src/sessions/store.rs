use rusqlite::{OptionalExtension, params};

use crate::database::{Database, DatabaseError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub status: String,
    pub role_profile_id: String,
    pub voice_route_id: String,
    pub transport_mode: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurn {
    pub id: String,
    pub session_id: String,
    pub turn_index: i64,
    pub user_text: String,
    pub assistant_text: String,
    pub materials_used: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCitation {
    pub id: i64,
    pub turn_id: String,
    pub material_id: String,
    pub chunk_id: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    pub id: i64,
    pub session_id: String,
    pub seq: i64,
    pub kind: String,
    pub payload: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub id: String,
    pub session_id: String,
    pub app_version: String,
    pub config_revision: String,
    pub provider_ids: String,
    pub model_ids: String,
    pub voice_route_id: String,
    pub transport_mode: String,
    pub role_hash: String,
    pub knowledge_fingerprint: String,
    pub created_at: String,
}

pub struct NewSession<'a> {
    pub id: &'a str,
    pub status: &'a str,
    pub role_profile_id: &'a str,
    pub voice_route_id: &'a str,
    pub transport_mode: &'a str,
}

pub struct NewTurn<'a> {
    pub id: &'a str,
    pub session_id: &'a str,
    pub turn_index: i64,
    pub user_text: &'a str,
    pub assistant_text: &'a str,
    pub materials_used: bool,
}

pub struct NewCitation<'a> {
    pub turn_id: &'a str,
    pub material_id: &'a str,
    pub chunk_id: &'a str,
    pub snippet: &'a str,
}

pub struct NewSnapshot<'a> {
    pub id: &'a str,
    pub session_id: &'a str,
    pub app_version: &'a str,
    pub config_revision: &'a str,
    pub provider_ids: &'a str,
    pub model_ids: &'a str,
    pub voice_route_id: &'a str,
    pub transport_mode: &'a str,
    pub role_hash: &'a str,
    pub knowledge_fingerprint: &'a str,
}

pub struct SessionStore<'a> {
    database: &'a Database,
}

impl<'a> SessionStore<'a> {
    pub fn new(database: &'a Database) -> Self {
        Self { database }
    }

    pub fn insert_session(&self, session: NewSession<'_>) -> Result<(), DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sessions(
                    id, status, role_profile_id, voice_route_id, transport_mode,
                    started_at, finished_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?6)",
                params![
                    session.id,
                    session.status,
                    session.role_profile_id,
                    session.voice_route_id,
                    session.transport_mode,
                    now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<SessionRecord>, DatabaseError> {
        self.database.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, status, role_profile_id, voice_route_id, transport_mode,
                            started_at, finished_at, updated_at
                     FROM sessions
                     WHERE id = ?1",
                    params![id],
                    map_session,
                )
                .optional()
        })
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, status, role_profile_id, voice_route_id, transport_mode,
                        started_at, finished_at, updated_at
                 FROM sessions
                 ORDER BY updated_at DESC, rowid DESC",
            )?;
            let rows = statement.query_map([], map_session)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn set_status(&self, id: &str, status: &str) -> Result<(), DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE sessions SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, status, now],
            )?;
            Ok(())
        })
    }

    pub fn finish(&self, id: &str, status: &str) -> Result<(), DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE sessions SET status = ?2, finished_at = ?3, updated_at = ?3 WHERE id = ?1",
                params![id, status, now],
            )?;
            Ok(())
        })
    }

    pub fn mark_interrupted_open(&self) -> Result<usize, DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE sessions
                 SET status = 'interrupted',
                     finished_at = COALESCE(finished_at, ?1),
                     updated_at = ?1
                 WHERE status NOT IN ('completed', 'failed', 'interrupted')",
                params![now],
            )
        })
    }

    pub fn context_summary(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, i64)>, DatabaseError> {
        self.database.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT context_summary, context_summary_turn_index
                     FROM sessions WHERE id = ?1",
                    params![session_id],
                    |row| {
                        let summary: Option<String> = row.get(0)?;
                        let upto: i64 = row.get(1)?;
                        Ok(summary.map(|s| (s, upto)))
                    },
                )
                .optional()
                .map(Option::flatten)
        })
    }

    pub fn set_context_summary(
        &self,
        session_id: &str,
        summary: &str,
        upto: i64,
    ) -> Result<(), DatabaseError> {
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE sessions SET context_summary = ?1, context_summary_turn_index = ?2 WHERE id = ?3",
                params![summary, upto, session_id],
            )?;
            Ok(())
        })
    }

    pub fn update_assistant_text(
        &self,
        turn_id: &str,
        assistant_text: &str,
    ) -> Result<(), DatabaseError> {
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE session_turns SET assistant_text = ?2 WHERE id = ?1",
                params![turn_id, assistant_text],
            )?;
            Ok(())
        })
    }

    pub fn insert_turn(&self, turn: NewTurn<'_>) -> Result<(), DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO session_turns(
                    id, session_id, turn_index, user_text, assistant_text,
                    materials_used, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    turn.id,
                    turn.session_id,
                    turn.turn_index,
                    turn.user_text,
                    turn.assistant_text,
                    i64::from(turn.materials_used),
                    now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn insert_citations(&self, citations: &[NewCitation<'_>]) -> Result<(), DatabaseError> {
        self.database.with_transaction(|transaction| {
            for citation in citations {
                transaction.execute(
                    "INSERT INTO session_citations(turn_id, material_id, chunk_id, snippet)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        citation.turn_id,
                        citation.material_id,
                        citation.chunk_id,
                        citation.snippet,
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn list_turns(&self, session_id: &str) -> Result<Vec<SessionTurn>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, session_id, turn_index, user_text, assistant_text,
                        materials_used, created_at
                 FROM session_turns
                 WHERE session_id = ?1
                 ORDER BY turn_index ASC",
            )?;
            let rows = statement.query_map(params![session_id], map_turn)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn list_citations(&self, turn_id: &str) -> Result<Vec<SessionCitation>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, turn_id, material_id, chunk_id, snippet
                 FROM session_citations
                 WHERE turn_id = ?1
                 ORDER BY id ASC",
            )?;
            let rows = statement.query_map(params![turn_id], map_citation)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn append_event(
        &self,
        session_id: &str,
        kind: &str,
        payload: &str,
    ) -> Result<i64, DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        self.database.with_transaction(|transaction| {
            let next_seq: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(seq) + 1, 0) FROM session_events WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO session_events(session_id, seq, kind, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session_id, next_seq, kind, payload, now],
            )?;
            Ok(next_seq)
        })
    }

    pub fn list_events(&self, session_id: &str) -> Result<Vec<SessionEvent>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, session_id, seq, kind, payload, created_at
                 FROM session_events
                 WHERE session_id = ?1
                 ORDER BY seq ASC",
            )?;
            let rows = statement.query_map(params![session_id], map_event)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn insert_snapshot(&self, snapshot: NewSnapshot<'_>) -> Result<(), DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO runtime_snapshots(
                    id, session_id, app_version, config_revision, provider_ids, model_ids,
                    voice_route_id, transport_mode, role_hash, knowledge_fingerprint, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    snapshot.id,
                    snapshot.session_id,
                    snapshot.app_version,
                    snapshot.config_revision,
                    snapshot.provider_ids,
                    snapshot.model_ids,
                    snapshot.voice_route_id,
                    snapshot.transport_mode,
                    snapshot.role_hash,
                    snapshot.knowledge_fingerprint,
                    now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_snapshots(&self, session_id: &str) -> Result<Vec<RuntimeSnapshot>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, session_id, app_version, config_revision, provider_ids, model_ids,
                        voice_route_id, transport_mode, role_hash, knowledge_fingerprint, created_at
                 FROM runtime_snapshots
                 WHERE session_id = ?1
                 ORDER BY created_at DESC, rowid DESC",
            )?;
            let rows = statement.query_map(params![session_id], map_snapshot)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn delete_session(&self, id: &str) -> Result<(), DatabaseError> {
        self.database.with_connection(|connection| {
            connection.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
            Ok(())
        })
    }
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        status: row.get(1)?,
        role_profile_id: row.get(2)?,
        voice_route_id: row.get(3)?,
        transport_mode: row.get(4)?,
        started_at: row.get(5)?,
        finished_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn map_turn(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionTurn> {
    Ok(SessionTurn {
        id: row.get(0)?,
        session_id: row.get(1)?,
        turn_index: row.get(2)?,
        user_text: row.get(3)?,
        assistant_text: row.get(4)?,
        materials_used: row.get::<_, i64>(5)? == 1,
        created_at: row.get(6)?,
    })
}

fn map_citation(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionCitation> {
    Ok(SessionCitation {
        id: row.get(0)?,
        turn_id: row.get(1)?,
        material_id: row.get(2)?,
        chunk_id: row.get(3)?,
        snippet: row.get(4)?,
    })
}

fn map_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionEvent> {
    Ok(SessionEvent {
        id: row.get(0)?,
        session_id: row.get(1)?,
        seq: row.get(2)?,
        kind: row.get(3)?,
        payload: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn map_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeSnapshot> {
    Ok(RuntimeSnapshot {
        id: row.get(0)?,
        session_id: row.get(1)?,
        app_version: row.get(2)?,
        config_revision: row.get(3)?,
        provider_ids: row.get(4)?,
        model_ids: row.get(5)?,
        voice_route_id: row.get(6)?,
        transport_mode: row.get(7)?,
        role_hash: row.get(8)?,
        knowledge_fingerprint: row.get(9)?,
        created_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{NewCitation, NewSession, NewSnapshot, NewTurn, SessionStore};
    use crate::database::Database;

    fn opened() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        (directory, database)
    }

    fn sample_session<'a>(id: &'a str, status: &'a str) -> NewSession<'a> {
        NewSession {
            id,
            status,
            role_profile_id: "role-1",
            voice_route_id: "route-1",
            transport_mode: "direct",
        }
    }

    #[test]
    fn insert_session_can_be_gotten_and_listed() {
        let (_directory, database) = opened();
        let store = SessionStore::new(&database);

        store
            .insert_session(sample_session("session-1", "listening"))
            .unwrap();
        store
            .insert_session(sample_session("session-2", "completed"))
            .unwrap();

        let got = store.get("session-1").unwrap().expect("session-1 exists");
        assert_eq!(got.id, "session-1");
        assert_eq!(got.status, "listening");
        assert_eq!(got.role_profile_id, "role-1");
        assert_eq!(got.voice_route_id, "route-1");
        assert_eq!(got.transport_mode, "direct");
        assert!(got.started_at.is_some());
        assert!(got.finished_at.is_none());
        assert!(!got.updated_at.is_empty());

        let listed: Vec<_> = store
            .list()
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(listed, vec!["session-2", "session-1"]);
        assert!(store.get("missing").unwrap().is_none());
    }

    #[test]
    fn set_status_updates_persisted_session() {
        let (_directory, database) = opened();
        let store = SessionStore::new(&database);
        store
            .insert_session(sample_session("session-1", "idle"))
            .unwrap();

        store.set_status("session-1", "listening").unwrap();

        let got = store.get("session-1").unwrap().expect("session-1 exists");
        assert_eq!(got.status, "listening");
    }

    #[test]
    fn mark_interrupted_open_marks_non_terminal_sessions() {
        let (_directory, database) = opened();
        let store = SessionStore::new(&database);
        for (id, status) in [
            ("open-listening", "listening"),
            ("open-preparing", "preparing"),
            ("done", "completed"),
            ("failed", "failed"),
            ("already", "interrupted"),
        ] {
            store.insert_session(sample_session(id, status)).unwrap();
        }

        store.mark_interrupted_open().unwrap();

        assert_eq!(
            store.get("open-listening").unwrap().unwrap().status,
            "interrupted"
        );
        assert_eq!(
            store.get("open-preparing").unwrap().unwrap().status,
            "interrupted"
        );
        assert_eq!(store.get("done").unwrap().unwrap().status, "completed");
        assert_eq!(store.get("failed").unwrap().unwrap().status, "failed");
        assert_eq!(store.get("already").unwrap().unwrap().status, "interrupted");
    }

    #[test]
    fn insert_turn_and_citations_persist() {
        let (_directory, database) = opened();
        let store = SessionStore::new(&database);
        store
            .insert_session(sample_session("session-1", "thinking"))
            .unwrap();

        store
            .insert_turn(NewTurn {
                id: "turn-1",
                session_id: "session-1",
                turn_index: 0,
                user_text: "你好",
                assistant_text: "你好，我是助手",
                materials_used: true,
            })
            .unwrap();
        store
            .insert_citations(&[NewCitation {
                turn_id: "turn-1",
                material_id: "mat-1",
                chunk_id: "mat-1:0",
                snippet: "订单服务与 Kafka",
            }])
            .unwrap();

        let turns = store.list_turns("session-1").unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].id, "turn-1");
        assert_eq!(turns[0].turn_index, 0);
        assert_eq!(turns[0].user_text, "你好");
        assert_eq!(turns[0].assistant_text, "你好，我是助手");
        assert!(turns[0].materials_used);

        let citations = store.list_citations("turn-1").unwrap();
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].material_id, "mat-1");
        assert_eq!(citations[0].chunk_id, "mat-1:0");
        assert_eq!(citations[0].snippet, "订单服务与 Kafka");
    }

    #[test]
    fn append_event_assigns_monotonic_seq_per_session() {
        let (_directory, database) = opened();
        let store = SessionStore::new(&database);
        store
            .insert_session(sample_session("session-a", "listening"))
            .unwrap();
        store
            .insert_session(sample_session("session-b", "listening"))
            .unwrap();

        let first = store
            .append_event("session-a", "status", r#"{"status":"listening"}"#)
            .unwrap();
        let second = store
            .append_event("session-a", "transcript", r#"{"text":"你好"}"#)
            .unwrap();
        let third = store
            .append_event("session-a", "reply", r#"{"text":"你好，我是助手"}"#)
            .unwrap();
        let other = store
            .append_event("session-b", "takeover", r#"{"status":"paused"}"#)
            .unwrap();

        assert_eq!([first, second, third, other], [0, 1, 2, 0]);

        let events = store.list_events("session-a").unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.seq, event.kind.as_str(), event.payload.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (0, "status", r#"{"status":"listening"}"#),
                (1, "transcript", r#"{"text":"你好"}"#),
                (2, "reply", r#"{"text":"你好，我是助手"}"#),
            ]
        );
        assert!(
            !events
                .iter()
                .any(|event| event.payload.contains("pcm") || event.payload.contains("prompt"))
        );
    }

    #[test]
    fn insert_snapshot_persists_runtime_ids_without_secrets() {
        let (_directory, database) = opened();
        let store = SessionStore::new(&database);
        store
            .insert_session(sample_session("session-1", "preparing"))
            .unwrap();

        store
            .insert_snapshot(NewSnapshot {
                id: "snap-1",
                session_id: "session-1",
                app_version: "0.1.0",
                config_revision: "rev-1",
                provider_ids: r#"["asr-1","llm-1"]"#,
                model_ids: r#"["whisper","gpt"]"#,
                voice_route_id: "route-1",
                transport_mode: "direct",
                role_hash: "abc123",
                knowledge_fingerprint: "local|bge|1024",
            })
            .unwrap();

        let snapshots = store.list_snapshots("session-1").unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, "snap-1");
        assert_eq!(snapshots[0].app_version, "0.1.0");
        assert_eq!(snapshots[0].provider_ids, r#"["asr-1","llm-1"]"#);
        assert_eq!(snapshots[0].model_ids, r#"["whisper","gpt"]"#);
        assert_eq!(snapshots[0].role_hash, "abc123");
        assert_eq!(snapshots[0].knowledge_fingerprint, "local|bge|1024");
        assert_eq!(snapshots[0].transport_mode, "direct");
        assert!(!snapshots[0].provider_ids.contains("sk-"));
    }

    #[test]
    fn delete_session_cascades_turns_events_and_snapshots() {
        let (_directory, database) = opened();
        let store = SessionStore::new(&database);
        store
            .insert_session(sample_session("session-1", "speaking"))
            .unwrap();
        store
            .insert_turn(NewTurn {
                id: "turn-1",
                session_id: "session-1",
                turn_index: 0,
                user_text: "问",
                assistant_text: "答",
                materials_used: false,
            })
            .unwrap();
        store
            .insert_citations(&[NewCitation {
                turn_id: "turn-1",
                material_id: "mat-1",
                chunk_id: "mat-1:0",
                snippet: "片段",
            }])
            .unwrap();
        store
            .append_event("session-1", "status", r#"{"status":"speaking"}"#)
            .unwrap();
        store
            .insert_snapshot(NewSnapshot {
                id: "snap-1",
                session_id: "session-1",
                app_version: "0.1.0",
                config_revision: "rev-1",
                provider_ids: "[]",
                model_ids: "[]",
                voice_route_id: "route-1",
                transport_mode: "direct",
                role_hash: "hash",
                knowledge_fingerprint: "",
            })
            .unwrap();

        assert!(store.get("session-1").unwrap().is_some());
        assert_eq!(store.list_turns("session-1").unwrap().len(), 1);
        assert_eq!(store.list_citations("turn-1").unwrap().len(), 1);
        assert_eq!(store.list_events("session-1").unwrap().len(), 1);
        assert_eq!(store.list_snapshots("session-1").unwrap().len(), 1);

        store.delete_session("session-1").unwrap();

        assert!(store.get("session-1").unwrap().is_none());
        assert!(store.list_turns("session-1").unwrap().is_empty());
        assert!(store.list_citations("turn-1").unwrap().is_empty());
        assert!(store.list_events("session-1").unwrap().is_empty());
        assert!(store.list_snapshots("session-1").unwrap().is_empty());
    }

    fn insert_test_session(store: &SessionStore<'_>) -> String {
        let id = "session-context-summary";
        store
            .insert_session(NewSession {
                id,
                status: "completed",
                role_profile_id: "",
                voice_route_id: "",
                transport_mode: "direct",
            })
            .unwrap();
        id.to_string()
    }

    #[test]
    fn context_summary_roundtrip_and_absence() {
        let (_directory, database) = opened();
        let store = SessionStore::new(&database);
        // 造一个会话（沿用既有 NewSession 助手/字面量，与相邻测试一致）
        let session_id = insert_test_session(&store);
        assert!(store.context_summary(&session_id).unwrap().is_none());
        // 不存在会话的读路径：返回 None 而非错误。
        assert!(store.context_summary("session-missing").unwrap().is_none());
        store
            .set_context_summary(&session_id, "此前讨论了时间问题", 3)
            .unwrap();
        assert_eq!(
            store.context_summary(&session_id).unwrap(),
            Some(("此前讨论了时间问题".to_string(), 3))
        );
    }
}
