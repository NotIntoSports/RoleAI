//! 会话延迟数据导出：CSV（每轮一行，各阶段一列）与 Chrome Trace JSON
//! （`chrome://tracing` / Perfetto ui.perfetto.dev 可直接打开）。
//! 导出内容只来自 turn_meta 的时间线与标注，不含提示词、凭据与原始 PCM。

use std::path::{Path, PathBuf};

use crate::services::realtime_pump::TurnTimeline;

use super::store::{SessionEvent, SessionStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatencyExportError {
    NotFound,
    WriteFailed,
    Database,
}

impl LatencyExportError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotFound => "SESSION_NOT_FOUND",
            Self::WriteFailed => "SESSION_EXPORT_WRITE_FAILED",
            Self::Database => "DATABASE_OPERATION_FAILED",
        }
    }
}

impl From<crate::database::DatabaseError> for LatencyExportError {
    fn from(_: crate::database::DatabaseError) -> Self {
        Self::Database
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatencyExportFormat {
    Csv,
    Trace,
}

impl LatencyExportFormat {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "csv" => Some(Self::Csv),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Csv => "latency.csv",
            Self::Trace => "latency.trace.json",
        }
    }
}

/// 读侧共用的单轮延迟视图解析：turn_meta → (时间线, 模式, 是否被打断)。
/// 旧记录没有 timeline 时返回 None（前端显示「无数据」，导出跳过该轮）。
pub(crate) fn parse_turn_latency(
    meta: &serde_json::Value,
) -> Option<(TurnTimeline, &'static str, bool)> {
    let timeline: TurnTimeline = serde_json::from_value(meta["timeline"].clone()).ok()?;
    // latencyMode 由泵/级联打点写入；F02 之前的存量记录只有泵路径有 timeline，
    // 缺省归一为 realtime。
    let mode = match meta["latencyMode"].as_str() {
        Some("cascade") => "cascade",
        _ => "realtime",
    };
    let interrupted = meta["playbackStatus"].as_str() == Some("interrupted");
    Some((timeline, mode, interrupted))
}

/// 单轮总延迟：优先 latencyMsFirstAudio（泵：speech_stopped → 首包；
/// 级联：轮次起点 → TTS 完成），缺省退回时间线首末锚点跨度。
pub(crate) fn turn_total_latency_ms(meta: &serde_json::Value) -> Option<f64> {
    if let Some(total) = meta.get("latencyMsFirstAudio").and_then(|value| value.as_f64()) {
        return Some(total);
    }
    let timeline = meta.get("timeline")?;
    let values: Vec<f64> = timeline
        .as_object()?
        .values()
        .filter_map(|value| value.as_f64())
        .collect();
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if values.is_empty() || max < min {
        return None;
    }
    Some(max - min)
}

/// 导出文件名：`<session_id>-latency.<扩展名>`，与 session_export 同目录约定。
pub(crate) fn latency_export_path(export_root: &Path, session_id: &str, format: LatencyExportFormat) -> PathBuf {
    export_root.join(format!("{session_id}-{}", format.extension()))
}

/// 导出入口：写 `<export_root>/<session_id>-latency.<ext>`。
pub fn export_latency(
    store: &SessionStore<'_>,
    session_id: &str,
    format: LatencyExportFormat,
    export_root: &Path,
) -> Result<PathBuf, LatencyExportError> {
    let session = store.get(session_id)?.ok_or(LatencyExportError::NotFound)?;
    let turns = store.list_turns(session_id)?;
    let events = store.list_events(session_id)?;
    let metas: Vec<(i64, serde_json::Value)> = events
        .iter()
        .filter(|event| event.kind == "turn_meta")
        .filter_map(|event| {
            serde_json::from_str::<serde_json::Value>(&event.payload)
                .ok()
                .map(|meta| (event.seq, meta))
        })
        .collect();
    let body = match format {
        LatencyExportFormat::Csv => build_csv(&session.id, &session.voice_route_id, &turns, &metas),
        LatencyExportFormat::Trace => build_trace(&session.id, &session.voice_route_id, &turns, &metas),
    };
    std::fs::create_dir_all(export_root).map_err(|_| LatencyExportError::WriteFailed)?;
    let path = latency_export_path(export_root, session_id, format);
    std::fs::write(&path, body).map_err(|_| LatencyExportError::WriteFailed)?;
    Ok(path)
}

/// CSV 列（固定序）：轮次元数据 + 全部时间线字段。
const CSV_COLUMNS: [&str; 20] = [
    "turnIndex",
    "createdAt",
    "mode",
    "interrupted",
    "totalMs",
    "speechStartedMs",
    "speechStoppedMs",
    "transcriptDoneMs",
    "responseCreatedMs",
    "asrDoneMs",
    "retrievalDoneMs",
    "llmFirstTokenMs",
    "llmDoneMs",
    "firstAudioMs",
    "ttsDoneMs",
    "responseDoneMs",
    "playbackStartedMs",
    "playbackDoneMs",
    "userText",
    "assistantText",
];

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn build_csv(
    _session_id: &str,
    route_id: &str,
    turns: &[crate::sessions::store::SessionTurn],
    metas: &[(i64, serde_json::Value)],
) -> String {
    // meta 无独立 turnIndex 字段：按 turnId 对应轮次（与 session_detail 读侧一致）。
    let meta_for = |turn_id: &str| {
        metas
            .iter()
            .rev()
            .map(|(_, meta)| meta)
            .find(|meta: &&serde_json::Value| meta["turnId"] == serde_json::json!(turn_id))
    };
    let mut body = String::from("# session_id,route_id\n");
    body.push_str(&format!("# {},{}\n", csv_escape(_session_id), csv_escape(route_id)));
    body.push_str(&CSV_COLUMNS.join(","));
    body.push('\n');
    for turn in turns {
        let meta = meta_for(&turn.id);
        let (timeline, mode, interrupted) = match meta.and_then(parse_turn_latency) {
            Some(parsed) => parsed,
            None => continue, // 旧记录无时间线：跳过（CSV 只收可解释的延迟轮）。
        };
        let total = meta.and_then(|meta| turn_total_latency_ms(meta));
        let mut row: Vec<String> = vec![
            turn.turn_index.to_string(),
            csv_escape(&turn.created_at),
            mode.to_owned(),
            if interrupted { "true".into() } else { "false".into() },
            total.map(|value| value.to_string()).unwrap_or_default(),
        ];
        for field in [
            timeline.speech_started_ms,
            timeline.speech_stopped_ms,
            timeline.transcript_done_ms,
            timeline.response_created_ms,
            timeline.asr_done_ms,
            timeline.retrieval_done_ms,
            timeline.llm_first_token_ms,
            timeline.llm_done_ms,
            timeline.first_audio_ms,
            timeline.tts_done_ms,
            timeline.response_done_ms,
            timeline.playback_started_ms,
            timeline.playback_done_ms,
        ] {
            row.push(field.map(|value| value.to_string()).unwrap_or_default());
        }
        row.push(csv_escape(&turn.user_text));
        row.push(csv_escape(&turn.assistant_text));
        body.push_str(&row.join(","));
        body.push('\n');
    }
    body
}

/// Chrome Trace 的锚点顺序与语义（与前端瀑布条一致；按模式选择）。
const TRACE_REALTIME_ANCHORS: [(&str, &str); 6] = [
    ("speechStartedMs", "听到说话"),
    ("speechStoppedMs", "说完（断句）"),
    ("transcriptDoneMs", "转写完成"),
    ("responseCreatedMs", "请求发出"),
    ("firstAudioMs", "首包音频"),
    ("responseDoneMs", "回答完成"),
];

const TRACE_CASCADE_ANCHORS: [(&str, &str); 5] = [
    ("asrDoneMs", "转写完成"),
    ("retrievalDoneMs", "资料检索"),
    ("llmFirstTokenMs", "首词生成"),
    ("llmDoneMs", "回答生成"),
    ("ttsDoneMs", "语音合成"),
];

fn anchor_value(timeline: &TurnTimeline, field: &str) -> Option<u64> {
    let value = match field {
        "speechStartedMs" => timeline.speech_started_ms,
        "speechStoppedMs" => timeline.speech_stopped_ms,
        "transcriptDoneMs" => timeline.transcript_done_ms,
        "responseCreatedMs" => timeline.response_created_ms,
        "asrDoneMs" => timeline.asr_done_ms,
        "retrievalDoneMs" => timeline.retrieval_done_ms,
        "llmFirstTokenMs" => timeline.llm_first_token_ms,
        "llmDoneMs" => timeline.llm_done_ms,
        "firstAudioMs" => timeline.first_audio_ms,
        "ttsDoneMs" => timeline.tts_done_ms,
        "responseDoneMs" => timeline.response_done_ms,
        "playbackStartedMs" => timeline.playback_started_ms,
        "playbackDoneMs" => timeline.playback_done_ms,
        _ => None,
    };
    value
}

/// Chrome Trace JSON：每轮一个进程轨道（pid = turnIndex+1），相邻锚点之间
/// 为一个 Complete 片段（ts/dur 微秒）。`chrome://tracing` 与 Perfetto 可打开。
fn build_trace(
    session_id: &str,
    route_id: &str,
    turns: &[crate::sessions::store::SessionTurn],
    metas: &[(i64, serde_json::Value)],
) -> String {
    let meta_for = |turn_id: &str| {
        metas
            .iter()
            .rev()
            .map(|(_, meta)| meta)
            .find(|meta: &&serde_json::Value| meta["turnId"] == serde_json::json!(turn_id))
    };
    let mut events: Vec<serde_json::Value> = Vec::new();
    for turn in turns {
        let Some(meta) = meta_for(&turn.id) else {
            continue;
        };
        let Some((timeline, mode, interrupted)) = parse_turn_latency(meta) else {
            continue;
        };
        let pid = turn.turn_index + 1;
        let mode_label = if mode == "cascade" { "级联" } else { "实时" };
        events.push(serde_json::json!({
            "ph": "M", "name": "process_name", "pid": pid, "tid": 0,
            "args": {"name": format!("第 {} 轮 · {}{}", turn.turn_index + 1, mode_label, if interrupted { " · 被打断" } else { "" })},
        }));
        let anchors: &[(&str, &str)] = if mode == "cascade" {
            &TRACE_CASCADE_ANCHORS
        } else {
            &TRACE_REALTIME_ANCHORS
        };
        let present: Vec<(u64, &str, &str)> = anchors
            .iter()
            .filter_map(|(field, label)| {
                anchor_value(&timeline, field).map(|value| (value, *field, *label))
            })
            .collect();
        for window in present.windows(2) {
            let (start_ms, _, start_label) = (window[0].0, window[0].1, window[0].2);
            let (end_ms, end_field, _) = (window[1].0, window[1].1, window[1].2);
            if end_ms <= start_ms {
                continue;
            }
            events.push(serde_json::json!({
                "ph": "X", "cat": "latency",
                "name": format!("{} → {}", start_label, window[1].2),
                "pid": pid, "tid": 0,
                "ts": start_ms * 1_000,
                "dur": (end_ms - start_ms) * 1_000,
                "args": {"endField": end_field},
            }));
        }
    }
    let trace = serde_json::json!({
        "displayTimeUnit": "ms",
        "metadata": {"sessionId": session_id, "routeId": route_id},
        "traceEvents": events,
    });
    serde_json::to_string(&trace).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::{LatencyExportError, LatencyExportFormat, build_csv, build_trace, export_latency, latency_export_path};
    use crate::database::Database;
    use crate::sessions::store::{NewSession, NewTurn, SessionStore};

    fn opened() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        (directory, database)
    }

    fn seed(store: &SessionStore<'_>) -> (String, String) {
        store
            .insert_session(NewSession {
                id: "sess-lat",
                status: "completed",
                role_profile_id: "role",
                voice_route_id: "route-a",
                transport_mode: "direct",
            })
            .unwrap();
        store
            .insert_turn(NewTurn {
                id: "turn-1",
                session_id: "sess-lat",
                turn_index: 0,
                user_text: "问题",
                assistant_text: "回答",
                materials_used: false,
            })
            .unwrap();
        ("sess-lat".into(), "turn-1".into())
    }

    #[test]
    fn csv_has_fixed_header_and_stage_columns() {
        let body = build_csv(
            "sess",
            "route-a",
            &[],
            &[(1, serde_json::json!({"turnId": "turn-1", "timeline": {}}))],
        );
        let mut lines = body.lines();
        assert_eq!(lines.next().unwrap(), "# session_id,route_id");
        assert_eq!(lines.next().unwrap(), "# sess,route-a");
        let header = lines.next().unwrap();
        for column in [
            "turnIndex", "createdAt", "mode", "interrupted", "totalMs",
            "asrDoneMs", "ttsDoneMs", "responseDoneMs", "userText", "assistantText",
        ] {
            assert!(header.contains(&format!(",{column},")) || header.starts_with(&format!("{column},")) || header.ends_with(&format!(",{column}")), "missing {column} in {header}");
        }
    }

    #[test]
    fn csv_rows_cover_timed_turns_and_skip_untimed() {
        let turns = vec![
            crate::sessions::store::SessionTurn {
                id: "turn-1".into(),
                session_id: "s".into(),
                turn_index: 0,
                user_text: "带,逗号".into(),
                assistant_text: "回答".into(),
                materials_used: false,
                created_at: "2026-09-30T10:00:00Z".into(),
            },
            crate::sessions::store::SessionTurn {
                id: "turn-2".into(),
                session_id: "s".into(),
                turn_index: 1,
                user_text: "旧轮".into(),
                assistant_text: "无时间线".into(),
                materials_used: false,
                created_at: "2026-09-30T10:01:00Z".into(),
            },
        ];
        let metas = vec![
            (
                1,
                serde_json::json!({
                    "turnId": "turn-1", "latencyMode": "cascade", "playbackStatus": "played",
                    "latencyMsFirstAudio": 700,
                    "timeline": {"asrDoneMs": 120, "ttsDoneMs": 700}
                }),
            ),
            (2, serde_json::json!({"turnId": "turn-2", "playbackStatus": "played"})),
        ];
        let body = build_csv("s", "route-a", &turns, &metas);
        let data_rows: Vec<&str> = body
            .lines()
            .skip(3)
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(data_rows.len(), 1, "只有带时间线的轮次入 CSV：{body}");
        assert!(data_rows[0].contains("\"带,逗号\""), "含逗号文本需引号转义");
        assert!(data_rows[0].contains("cascade"));
    }

    #[test]
    fn trace_builds_per_turn_process_and_stage_slices() {
        let turns = vec![crate::sessions::store::SessionTurn {
            id: "turn-1".into(),
            session_id: "s".into(),
            turn_index: 0,
            user_text: "问".into(),
            assistant_text: "答".into(),
            materials_used: false,
            created_at: "2026-09-30T10:00:00Z".into(),
        }];
        let metas = vec![(
            1,
            serde_json::json!({
                "turnId": "turn-1", "latencyMode": "cascade", "playbackStatus": "played",
                "timeline": {"asrDoneMs": 100, "llmFirstTokenMs": 300, "llmDoneMs": 500, "ttsDoneMs": 700}
            }),
        )];
        let trace: serde_json::Value =
            serde_json::from_str(&build_trace("sess", "route-a", &turns, &metas)).unwrap();
        assert_eq!(trace["displayTimeUnit"], "ms");
        assert_eq!(trace["metadata"]["routeId"], "route-a");
        let events = trace["traceEvents"].as_array().unwrap();
        let process = events
            .iter()
            .find(|event| event["name"] == "process_name")
            .unwrap();
        assert_eq!(process["pid"], 1);
        assert!(process["args"]["name"]
            .as_str()
            .unwrap()
            .contains("级联"));
        // 4 个锚点 → 3 个片段；ts/dur 为微秒。
        let slices: Vec<&serde_json::Value> = events
            .iter()
            .filter(|event| event["ph"] == "X")
            .collect();
        assert_eq!(slices.len(), 3);
        assert_eq!(slices[0]["ts"], 100_000_u64);
        assert_eq!(slices[0]["dur"], 200_000_u64);
        assert_eq!(slices[2]["ts"], 500_000_u64);
        assert_eq!(slices[2]["dur"], 200_000_u64);
    }

    #[test]
    fn export_writes_file_inside_export_root_and_reports_missing_session() {
        let (directory, database) = opened();
        let store = SessionStore::new(&database);
        let (session_id, _turn_id) = seed(&store);
        store
            .append_event("sess-lat", "turn_meta", r#"{"turnId":"turn-1","latencyMode":"cascade","timeline":{"asrDoneMs":80,"ttsDoneMs":640}}"#)
            .unwrap();
        let export_root = directory.path().join("exports");
        let path = export_latency(&store, &session_id, LatencyExportFormat::Csv, &export_root).unwrap();
        assert_eq!(
            path,
            latency_export_path(&export_root, &session_id, LatencyExportFormat::Csv)
        );
        assert!(path.starts_with(&export_root));
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("asrDoneMs"));
        assert!(body.contains("80"));
        let missing = export_latency(&store, "nope", LatencyExportFormat::Csv, &export_root)
            .unwrap_err();
        assert_eq!(missing, LatencyExportError::NotFound);
    }
}
