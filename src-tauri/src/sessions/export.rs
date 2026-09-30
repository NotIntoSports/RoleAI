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

/// 训练报告导出（Markdown）。报告不存在返回 NotFound；
/// 与 `export_session` 一样把产物写到 `export_root`。
pub fn export_practice_report(
    database: &crate::database::Database,
    session_id: &str,
    format: SessionExportFormat,
    export_root: &Path,
) -> Result<PathBuf, SessionExportError> {
    if format != SessionExportFormat::Markdown {
        return Err(SessionExportError::FormatInvalid);
    }
    let report = crate::practice::store::PracticeReportStore::new(database)
        .get(session_id)?
        .ok_or(SessionExportError::NotFound)?;
    let body = render_practice_markdown(&report);
    std::fs::create_dir_all(export_root).map_err(|_| SessionExportError::WriteFailed)?;
    let path = export_root.join(format!(
        "practice_report_{session_id}.{}",
        format.extension()
    ));
    std::fs::write(&path, body).map_err(|_| SessionExportError::WriteFailed)?;
    Ok(path)
}

fn format_score(value: f64) -> String {
    if value <= 0.0 {
        "未评（AI 点评不可用）".to_owned()
    } else {
        format!("{value}/5")
    }
}

fn render_practice_markdown(report: &crate::practice::PracticeReport) -> String {
    const DISCLAIMER: &str = "评分由 AI 生成，仅供练习参考";
    let mut body = String::from("# 模拟面试训练报告\n\n");
    body.push_str(&format!(
        "- 岗位：{}\n- 面试官风格：{}\n- 总分：{}\n- 内容深度：{}；结构清晰：{}；表达流畅：{}；岗位匹配：{}\n- 生成时间：{}\n\n",
        if report.position.is_empty() { "未记录" } else { &report.position },
        if report.interviewer_style.is_empty() { "未记录" } else { &report.interviewer_style },
        format_score(report.total_score),
        format_score(report.dimensions.content_depth),
        format_score(report.dimensions.structure_clarity),
        format_score(report.dimensions.fluency),
        format_score(report.dimensions.job_fit),
        report.created_at,
    ));

    body.push_str("## 客观指标（本地计算）\n\n");
    let objective = &report.objective;
    match (
        objective.total_duration_seconds,
        objective.average_answer_seconds,
    ) {
        (Some(total), Some(average)) => {
            body.push_str(&format!(
                "- 总回答时长：{total:.1} 秒；平均每次 {average:.1} 秒\n"
            ));
        }
        _ => body.push_str("- 时长信息不可用（缺少轮次时间戳）\n"),
    }
    match objective.long_pauses {
        Some(pauses) => body.push_str(&format!("- 长停顿（>3 秒）：{pauses} 次\n")),
        None => body.push_str("- 长停顿：不可用（缺少时间信息）\n"),
    }
    if objective.top_fillers.is_empty() {
        body.push_str("- 口头禅：未检出\n");
    } else {
        let fillers = objective
            .top_fillers
            .iter()
            .take(5)
            .map(|hit| format!("{}×{}", hit.word, hit.count))
            .collect::<Vec<_>>()
            .join("、");
        body.push_str(&format!("- 口头禅 Top5：{fillers}\n"));
    }
    body.push('\n');

    body.push_str("## 逐题回顾\n\n");
    for review in &report.per_question {
        body.push_str(&format!(
            "### 第{}题：{}\n\n- 我的回答：{}\n- 评分：{}\n",
            review.index,
            review.question,
            review.answer,
            format_score(review.score),
        ));
        body.push_str(&format!(
            "- 优点：{}\n",
            join_or(review.strengths.as_slice(), "未记录")
        ));
        body.push_str(&format!(
            "- 问题：{}\n",
            join_or(review.issues.as_slice(), "未记录")
        ));
        body.push_str(&format!(
            "- 改进示范：{}\n\n",
            join_or(std::slice::from_ref(&review.model_answer), "未记录")
        ));
    }

    body.push_str("## 最重要的三条建议\n\n");
    if report.top_suggestions.is_empty() {
        body.push_str("- 定性点评生成失败，可重试。\n");
    } else {
        for (index, suggestion) in report.top_suggestions.iter().enumerate() {
            body.push_str(&format!("{}. {suggestion}\n", index + 1));
        }
    }
    body.push_str(&format!(
        "\n> {DISCLAIMER}；结构/STAR 提示为启发式，仅供参考。\n"
    ));
    body
}

fn join_or(values: &[String], fallback: &str) -> String {
    let joined = values.join("；");
    if joined.is_empty() {
        fallback.to_owned()
    } else {
        joined
    }
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

#[cfg(test)]
mod practice_report_tests {
    use super::*;
    use crate::database::Database;
    use crate::practice::{PracticeDimensions, PracticeQuestionReview, PracticeReport};

    fn sample_report(llm_available: bool) -> PracticeReport {
        PracticeReport {
            session_id: "sess-report".into(),
            plan_id: "plan-1".into(),
            position: "后端工程师".into(),
            interviewer_style: "严苛面试官".into(),
            llm_available,
            total_score: if llm_available { 4.0 } else { 0.0 },
            dimensions: PracticeDimensions {
                content_depth: 4.0,
                structure_clarity: 3.5,
                fluency: 4.0,
                job_fit: 3.0,
            },
            per_question: vec![PracticeQuestionReview {
                index: 1,
                question: "介绍一个你负责的项目".into(),
                answer: "我负责支付网关，用令牌桶限流。".into(),
                score: 4.0,
                strengths: vec!["结构清晰".into()],
                issues: vec!["缺少量化结果".into()],
                model_answer: "补充 p99 数字与业务影响。".into(),
            }],
            top_suggestions: if llm_available {
                vec![
                    "补充量化结果".into(),
                    "先讲结论再讲过程".into(),
                    "控制语速".into(),
                ]
            } else {
                Vec::new()
            },
            objective: crate::practice::metrics::compute_answers(
                &[crate::practice::metrics::AnswerInput::new("我负责支付网关")
                    .with_timing(0, 60_000)],
                &Default::default(),
            ),
            created_at: "2026-10-01T08:00:00Z".into(),
        }
    }

    #[test]
    fn practice_report_markdown_snapshot_contains_all_sections_and_disclaimer() {
        let report = sample_report(true);
        let body = super::render_practice_markdown(&report);
        let expected = "\
# 模拟面试训练报告

- 岗位：后端工程师
- 面试官风格：严苛面试官
- 总分：4/5
- 内容深度：4/5；结构清晰：3.5/5；表达流畅：4/5；岗位匹配：3/5
- 生成时间：2026-10-01T08:00:00Z

## 客观指标（本地计算）

- 总回答时长：60.0 秒；平均每次 60.0 秒
- 长停顿（>3 秒）：0 次
- 口头禅：未检出

## 逐题回顾

### 第1题：介绍一个你负责的项目

- 我的回答：我负责支付网关，用令牌桶限流。
- 评分：4/5
- 优点：结构清晰
- 问题：缺少量化结果
- 改进示范：补充 p99 数字与业务影响。

## 最重要的三条建议

1. 补充量化结果
2. 先讲结论再讲过程
3. 控制语速

> 评分由 AI 生成，仅供练习参考；结构/STAR 提示为启发式，仅供参考。
";
        assert_eq!(body, expected);
    }

    #[test]
    fn practice_report_markdown_marks_llm_fallback_as_unrated() {
        let body = super::render_practice_markdown(&sample_report(false));
        assert!(body.contains("总分：未评（AI 点评不可用）"));
        assert!(body.contains("定性点评生成失败，可重试"));
        assert!(body.contains("总回答时长：60.0 秒"));
        assert!(body.contains("评分由 AI 生成，仅供练习参考"));
    }

    fn opened() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        (directory, database)
    }

    #[test]
    fn practice_report_export_writes_markdown_file_and_rejects_other_formats() {
        let (directory, database) = opened();
        database.migrate().unwrap();
        // practice_reports 外键指向 sessions：先落一条会话满足级联约束。
        database
            .with_connection(|connection| {
                connection.execute_batch(
                    "INSERT INTO sessions(id, status, role_profile_id, voice_route_id, transport_mode, updated_at)
                     VALUES ('sess-report', 'completed', 'role-1', 'route-1', 'direct', '2026-10-01T08:00:00Z');",
                )
            })
            .unwrap();
        crate::practice::store::PracticeReportStore::new(&database)
            .save(&sample_report(true))
            .unwrap();
        let export_root = directory.path().join("exports");

        let path = super::export_practice_report(
            &database,
            "sess-report",
            SessionExportFormat::Markdown,
            &export_root,
        )
        .unwrap();
        assert!(path.ends_with("practice_report_sess-report.md"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# 模拟面试训练报告"));
        assert!(!content.contains("sk-") && !content.to_lowercase().contains("pcm"));

        let rejected = super::export_practice_report(
            &database,
            "sess-report",
            SessionExportFormat::Json,
            &export_root,
        )
        .unwrap_err();
        assert_eq!(rejected, SessionExportError::FormatInvalid);

        let missing = super::export_practice_report(
            &database,
            "sess-missing",
            SessionExportFormat::Markdown,
            &export_root,
        )
        .unwrap_err();
        assert_eq!(missing, SessionExportError::NotFound);
    }
}
