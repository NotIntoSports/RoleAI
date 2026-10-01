use std::path::{Path, PathBuf};

use crate::database::DatabaseError;
use crate::sessions::{SessionCitation, SessionRecord, SessionStore, SessionTurn};

const SNIPPET_CHARS: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExportFormat {
    Markdown,
    Json,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExportError {
    NotFound,
    FormatInvalid,
    WriteFailed,
    Database,
}

impl SessionExportError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotFound => "SESSION_NOT_FOUND",
            Self::FormatInvalid => "SESSION_EXPORT_FORMAT_INVALID",
            Self::WriteFailed => "SESSION_EXPORT_FAILED",
            Self::Database => "DATABASE_OPERATION_FAILED",
        }
    }
}

impl From<DatabaseError> for SessionExportError {
    fn from(_: DatabaseError) -> Self {
        Self::Database
    }
}

impl SessionExportFormat {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "markdown" => Some(Self::Markdown),
            "json" => Some(Self::Json),
            "text" => Some(Self::Text),
            _ => None,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Json => "json",
            Self::Text => "txt",
        }
    }
}

pub fn export_session(
    store: &SessionStore<'_>,
    session_id: &str,
    format: SessionExportFormat,
    export_root: &Path,
) -> Result<PathBuf, SessionExportError> {
    let session = store.get(session_id)?.ok_or(SessionExportError::NotFound)?;
    let turns = store.list_turns(session_id)?;
    let mut exported = Vec::with_capacity(turns.len());
    for turn in turns {
        let citations = store.list_citations(&turn.id)?;
        exported.push((turn, citations));
    }
    let body = render_export(&session, &exported, format);
    std::fs::create_dir_all(export_root).map_err(|_| SessionExportError::WriteFailed)?;
    let path = export_root.join(format!("{session_id}.{}", format.extension()));
    std::fs::write(&path, body).map_err(|_| SessionExportError::WriteFailed)?;
    Ok(path)
}

fn clip_snippet(text: &str) -> String {
    text.chars().take(SNIPPET_CHARS).collect()
}

fn render_export(
    session: &SessionRecord,
    turns: &[(SessionTurn, Vec<SessionCitation>)],
    format: SessionExportFormat,
) -> String {
    match format {
        SessionExportFormat::Json => render_json(session, turns),
        SessionExportFormat::Markdown => render_markdown(session, turns),
        SessionExportFormat::Text => render_text(session, turns),
    }
}

fn render_json(session: &SessionRecord, turns: &[(SessionTurn, Vec<SessionCitation>)]) -> String {
    let turns = turns
        .iter()
        .map(|(turn, citations)| {
            serde_json::json!({
                "turnIndex": turn.turn_index,
                "userText": turn.user_text,
                "assistantText": turn.assistant_text,
                "materialsUsed": turn.materials_used,
                "citations": citations.iter().map(|citation| {
                    serde_json::json!({
                        "snippet": clip_snippet(&citation.snippet),
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "schemaVersion": 1,
        "id": session.id,
        "status": session.status,
        "phase": session.status,
        "startedAt": session.started_at,
        "finishedAt": session.finished_at,
        "updatedAt": session.updated_at,
        "turns": turns,
    })
    .to_string()
}

fn render_markdown(
    session: &SessionRecord,
    turns: &[(SessionTurn, Vec<SessionCitation>)],
) -> String {
    let mut body = format!(
        "# Session {}\n\nstatus: {}\nstarted: {}\nfinished: {}\n",
        session.id,
        session.status,
        session.started_at.as_deref().unwrap_or(""),
        session.finished_at.as_deref().unwrap_or(""),
    );
    for (turn, citations) in turns {
        body.push_str(&format!(
            "\n## Turn {}\n\n**user**: {}\n\n**assistant**: {}\n",
            turn.turn_index, turn.user_text, turn.assistant_text
        ));
        for citation in citations {
            body.push_str(&format!("\n- {}\n", clip_snippet(&citation.snippet)));
        }
    }
    body
}

fn render_text(session: &SessionRecord, turns: &[(SessionTurn, Vec<SessionCitation>)]) -> String {
    let mut body = format!(
        "Session {}\nstatus: {}\nstarted: {}\nfinished: {}\n",
        session.id,
        session.status,
        session.started_at.as_deref().unwrap_or(""),
        session.finished_at.as_deref().unwrap_or(""),
    );
    for (turn, citations) in turns {
        body.push_str(&format!(
            "\nTurn {}\nuser: {}\nassistant: {}\n",
            turn.turn_index, turn.user_text, turn.assistant_text
        ));
        for citation in citations {
            body.push_str(&format!("citation: {}\n", clip_snippet(&citation.snippet)));
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::{SessionExportError, SessionExportFormat, export_session};
    use crate::database::Database;
    use crate::sessions::{NewCitation, NewSession, NewSnapshot, NewTurn, SessionStore};

    const SECRET: &str = "sk-export-secret-value";
    const VECTOR: &str = "[0.125,-0.5,0.75]";
    const PCM: &str = "PCM_RAW_BYTES";
    const FULL_MATERIAL: &str = "FULL_MATERIAL_BODY_SHOULD_NOT_EXPORT";
    const PROVIDER_PAYLOAD: &str = r#"{"choices":[{"message":{"content":"RAW_PROVIDER_BODY"}}]}"#;

    fn opened() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        (directory, database)
    }

    fn seed_session(store: &SessionStore<'_>) {
        store
            .insert_session(NewSession {
                id: "session-export",
                status: "completed",
                role_profile_id: "role-1",
                voice_route_id: "route-1",
                transport_mode: "direct",
            })
            .unwrap();
        store.finish("session-export", "completed").unwrap();
        store
            .insert_turn(NewTurn {
                id: "turn-export",
                session_id: "session-export",
                turn_index: 0,
                user_text: "订单超时怎么处理",
                assistant_text: "先查订单服务日志",
                materials_used: true,
            })
            .unwrap();
        let long_snippet: String = "引用片段"
            .chars()
            .chain(std::iter::repeat('字'))
            .take(200)
            .collect();
        store
            .insert_citations(&[NewCitation {
                turn_id: "turn-export",
                material_id: "mat-1",
                chunk_id: "mat-1:0",
                snippet: &long_snippet,
            }])
            .unwrap();
        store
            .insert_snapshot(NewSnapshot {
                id: "snap-export",
                session_id: "session-export",
                app_version: "0.1.0",
                config_revision: "rev-1",
                provider_ids: r#"["asr-1"]"#,
                model_ids: r#"["whisper"]"#,
                voice_route_id: "route-1",
                transport_mode: "direct",
                role_hash: "rolehash",
                knowledge_fingerprint: "local|bge|1024",
            })
            .unwrap();
        store
            .append_event(
                "session-export",
                "reply",
                &format!(
                    r#"{{"secret":"{SECRET}","vector":{VECTOR},"pcm":"{PCM}","material":"{FULL_MATERIAL}","provider":{PROVIDER_PAYLOAD}}}"#
                ),
            )
            .unwrap();
    }

    fn export_body(
        store: &SessionStore<'_>,
        export_root: &std::path::Path,
        format: SessionExportFormat,
    ) -> String {
        let path = export_session(store, "session-export", format, export_root).unwrap();
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some(format.extension())
        );
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn json_export_includes_schema_timestamps_phase_and_citation_snippets() {
        let (directory, database) = opened();
        let store = SessionStore::new(&database);
        seed_session(&store);
        let session = store.get("session-export").unwrap().unwrap();

        let body = export_body(
            &store,
            &directory.path().join("exports"),
            SessionExportFormat::Json,
        );
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["id"], "session-export");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["phase"], "completed");
        assert_eq!(json["startedAt"], session.started_at.clone().unwrap());
        assert_eq!(json["finishedAt"], session.finished_at.clone().unwrap());
        assert_eq!(json["updatedAt"], session.updated_at);
        assert_eq!(json["turns"][0]["turnIndex"], 0);
        assert_eq!(json["turns"][0]["userText"], "订单超时怎么处理");
        assert_eq!(json["turns"][0]["assistantText"], "先查订单服务日志");
        assert_eq!(json["turns"][0]["materialsUsed"], true);
        let snippet = json["turns"][0]["citations"][0]["snippet"]
            .as_str()
            .expect("citation snippet");
        assert!(snippet.starts_with("引用片段"));
        assert_eq!(snippet.chars().count(), 160);
        assert!(json.get("providerIds").is_none());
        assert!(json.get("events").is_none());
        assert!(json["turns"][0].get("ttsPcm").is_none());
    }

    #[test]
    fn markdown_and_text_include_turns_and_clipped_citation_snippets() {
        let (directory, database) = opened();
        let store = SessionStore::new(&database);
        seed_session(&store);
        let session = store.get("session-export").unwrap().unwrap();
        let export_root = directory.path().join("exports");

        let markdown = export_body(&store, &export_root, SessionExportFormat::Markdown);
        assert!(markdown.contains("# Session session-export"));
        assert!(markdown.contains("status: completed"));
        assert!(markdown.contains(&format!("started: {}", session.started_at.clone().unwrap())));
        assert!(markdown.contains(&format!(
            "finished: {}",
            session.finished_at.clone().unwrap()
        )));
        assert!(markdown.contains("**user**: 订单超时怎么处理"));
        assert!(markdown.contains("**assistant**: 先查订单服务日志"));
        assert!(markdown.contains("引用片段"));
        assert!(!markdown.contains(&"字".repeat(157)));

        let text = export_body(&store, &export_root, SessionExportFormat::Text);
        assert!(text.contains("Session session-export"));
        assert!(text.contains("status: completed"));
        assert!(text.contains("user: 订单超时怎么处理"));
        assert!(text.contains("assistant: 先查订单服务日志"));
        assert!(text.contains("引用片段"));
        assert!(!text.contains(&"字".repeat(157)));
    }

    #[test]
    fn export_omits_secrets_vectors_pcm_full_materials_and_provider_payloads() {
        let (directory, database) = opened();
        let store = SessionStore::new(&database);
        seed_session(&store);
        let export_root = directory.path().join("exports");

        for format in [
            SessionExportFormat::Json,
            SessionExportFormat::Markdown,
            SessionExportFormat::Text,
        ] {
            let body = export_body(&store, &export_root, format);
            let lower = body.to_ascii_lowercase();
            assert!(!body.contains(SECRET), "{format:?} leaked secret");
            assert!(!body.contains(VECTOR), "{format:?} leaked vector");
            assert!(!body.contains(PCM), "{format:?} leaked pcm");
            assert!(
                !body.contains(FULL_MATERIAL),
                "{format:?} leaked full material"
            );
            assert!(
                !body.contains("RAW_PROVIDER_BODY"),
                "{format:?} leaked provider payload"
            );
            assert!(!lower.contains("api_key"), "{format:?} leaked api_key");
            assert!(!lower.contains("password"), "{format:?} leaked password");
        }
    }

    #[test]
    fn export_missing_session_returns_not_found() {
        let (directory, database) = opened();
        let store = SessionStore::new(&database);
        let error = export_session(
            &store,
            "missing",
            SessionExportFormat::Json,
            &directory.path().join("exports"),
        )
        .unwrap_err();
        assert_eq!(error, SessionExportError::NotFound);
        assert_eq!(error.code(), "SESSION_NOT_FOUND");
    }
}
