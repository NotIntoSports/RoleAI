//! 会话历史记录命令：列表/详情/删除/导出（纯搬移自 sessions.rs）。

use super::*;

/// 从 turn_meta 组装单轮延迟视图；旧记录没有 timeline 时返回 None（前端显示「无数据」）。
fn turn_latency_view(
    meta: Option<&serde_json::Value>,
    route_id: &str,
) -> Option<crate::contracts::TurnLatencyView> {
    let meta = meta?;
    let (timeline, mode, interrupted) =
        crate::sessions::latency_export::parse_turn_latency(meta)?;
    Some(crate::contracts::TurnLatencyView {
        route_id: route_id.to_owned(),
        mode: mode.to_owned(),
        interrupted,
        timeline,
    })
}

pub(in crate::commands) fn session_detail(
    store: &SessionStore<'_>,
    id: &str,
) -> Result<SessionDetail, SessionServiceError> {
    let session = store.get(id)?.ok_or(SessionServiceError::NotFound)?;
    let mut turns = Vec::new();
    let events = store.list_events(id)?;
    for turn in store.list_turns(id)? {
        let web = events
            .iter()
            .rev()
            .filter(|event| event.kind == "web_sources")
            .filter_map(|event| serde_json::from_str::<serde_json::Value>(&event.payload).ok())
            .find(|event| event["turnId"] == turn.id);
        let meta = events
            .iter()
            .rev()
            .filter(|event| event.kind == "turn_meta")
            .filter_map(|event| serde_json::from_str::<serde_json::Value>(&event.payload).ok())
            .find(|event| event["turnId"] == turn.id);
        let citations = store
            .list_citations(&turn.id)?
            .into_iter()
            .map(|citation| SessionCitationView {
                material_id: citation.material_id,
                chunk_id: citation.chunk_id,
                snippet: clip_snippet(&citation.snippet),
            })
            .collect();
        let latency = turn_latency_view(meta.as_ref(), &session.voice_route_id);
        turns.push(SessionTurnView {
            trigger_source: meta
                .as_ref()
                .and_then(|value| value["triggerSource"].as_str())
                .map(str::to_owned),
            user_confirmed: meta
                .as_ref()
                .and_then(|value| value["userConfirmed"].as_bool()),
            playback_status: meta
                .as_ref()
                .and_then(|value| value["playbackStatus"].as_str())
                .map(str::to_owned),
            web_sources: web
                .as_ref()
                .and_then(|event| serde_json::from_value(event["sources"].clone()).ok()),
            web_degraded: web.as_ref().and_then(|event| event["degraded"].as_bool()),
            latency,
            id: turn.id,
            turn_index: turn.turn_index,
            user_text: turn.user_text,
            assistant_text: turn.assistant_text,
            materials_used: turn.materials_used,
            citations,
            created_at: turn.created_at,
        });
    }
    Ok(SessionDetail {
        session: session.into(),
        turns,
    })
}

pub(in crate::commands) fn session_list_cmd(
    state: &AppState,
) -> CommandResult<Vec<SessionSummary>> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    match SessionStore::new(database).list() {
        Ok(rows) => CommandResult::Ok {
            data: rows.into_iter().map(SessionSummary::from).collect(),
        },
        Err(error) => service_error(error.code(), "Session list failed"),
    }
}

pub(in crate::commands) fn session_get_cmd(
    state: &AppState,
    session_id: String,
) -> CommandResult<SessionDetail> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    match session_detail(&SessionStore::new(database), &session_id) {
        Ok(detail) => CommandResult::Ok { data: detail },
        Err(error) => session_service_error(error),
    }
}

pub(in crate::commands) fn session_delete_cmd(
    state: &AppState,
    session_id: String,
) -> CommandResult<FoundationStatus> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let mut sessions = match state.sessions.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error(
                "SERVICE_BUSY",
                "Service configuration is temporarily unavailable",
            );
        }
    };
    if sessions.session_id() == Some(session_id.as_str()) {
        sessions.reset();
    }
    match SessionStore::new(database).delete_session(&session_id) {
        Ok(()) => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        Err(error) => service_error(error.code(), "Session delete failed"),
    }
}

pub(in crate::commands) fn session_export_cmd(
    state: &AppState,
    session_id: String,
    format: String,
) -> CommandResult<SessionExportResult> {
    let Some(format) = SessionExportFormat::from_name(&format) else {
        return CommandResult::Err {
            error: PublicError::new(
                SessionExportError::FormatInvalid.code(),
                "Unsupported export format",
                false,
            )
            .with_field("format"),
        };
    };
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let export_root = state.paths.data_directory.join("exports");
    match export_session(
        &SessionStore::new(database),
        &session_id,
        format,
        &export_root,
    ) {
        Ok(path) => CommandResult::Ok {
            data: SessionExportResult {
                path: path.to_string_lossy().into_owned(),
            },
        },
        Err(error) => {
            let mut public = PublicError::new(error.code(), "Session export failed", false);
            if error == SessionExportError::NotFound {
                public = public.with_field("sessionId");
            }
            CommandResult::Err { error: public }
        }
    }
}

pub fn session_export_blocking(
    state: State<'_, AppState>,
    session_id: String,
    format: String,
) -> CommandResult<SessionExportResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    session_export_cmd(&state, session_id, format)
}

pub fn session_list_blocking(state: State<'_, AppState>) -> CommandResult<Vec<SessionSummary>> {
    session_list_cmd(&state)
}

pub fn session_get_blocking(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<SessionDetail> {
    session_get_cmd(&state, session_id)
}

pub fn session_delete_blocking(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    session_delete_cmd(&state, session_id)
}

pub(in crate::commands) fn session_latency_export_cmd(
    state: &AppState,
    session_id: String,
    format: String,
) -> CommandResult<SessionExportResult> {
    let Some(format) = crate::sessions::LatencyExportFormat::from_name(&format) else {
        return CommandResult::Err {
            error: PublicError::new(
                SessionExportError::FormatInvalid.code(),
                "Unsupported export format",
                false,
            )
            .with_field("format"),
        };
    };
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    // 与 session_export 同目录（数据目录 exports/ 下），渲染进程不可指定任意路径。
    let export_root = state.paths.data_directory.join("exports");
    match crate::sessions::export_latency(
        &SessionStore::new(database),
        &session_id,
        format,
        &export_root,
    ) {
        Ok(path) => CommandResult::Ok {
            data: SessionExportResult {
                path: path.to_string_lossy().into_owned(),
            },
        },
        Err(error) => {
            let mut public = PublicError::new(error.code(), "Session export failed", false);
            if error == crate::sessions::LatencyExportError::NotFound {
                public = public.with_field("sessionId");
            }
            CommandResult::Err { error: public }
        }
    }
}

pub fn session_latency_export_blocking(
    state: State<'_, AppState>,
    session_id: String,
    format: String,
) -> CommandResult<SessionExportResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    session_latency_export_cmd(&state, session_id, format)
}

blocking_command!(session_export, session_export_blocking(session_id: String, format: String) -> SessionExportResult);
blocking_command!(session_latency_export, session_latency_export_blocking(session_id: String, format: String) -> SessionExportResult);
blocking_command!(session_list, session_list_blocking() -> Vec<SessionSummary>);
blocking_command!(session_get, session_get_blocking(session_id: String) -> SessionDetail);
blocking_command!(session_delete, session_delete_blocking(session_id: String) -> FoundationStatus);

#[cfg(test)]
mod turn_latency_view_tests {
    use super::turn_latency_view;

    #[test]
    fn cascade_meta_maps_to_latency_view() {
        let meta = serde_json::json!({
            "latencyMode": "cascade",
            "playbackStatus": "played",
            "timeline": {"asrDoneMs": 12, "llmFirstTokenMs": 40},
        });
        let view = turn_latency_view(Some(&meta), "route-1").unwrap();
        assert_eq!(view.route_id, "route-1");
        assert_eq!(view.mode, "cascade");
        assert!(!view.interrupted);
        assert_eq!(view.timeline.asr_done_ms, Some(12));
        assert_eq!(view.timeline.llm_first_token_ms, Some(40));
        assert_eq!(view.timeline.first_audio_ms, None);
    }

    #[test]
    fn legacy_pump_meta_defaults_to_realtime_mode() {
        let meta = serde_json::json!({
            "playbackStatus": "interrupted",
            "timeline": {"speechStartedMs": 5, "firstAudioMs": 300},
        });
        let view = turn_latency_view(Some(&meta), "route-2").unwrap();
        assert_eq!(view.mode, "realtime");
        assert!(view.interrupted);
        assert_eq!(view.timeline.speech_started_ms, Some(5));
    }

    #[test]
    fn meta_without_timeline_yields_none() {
        let meta = serde_json::json!({"playbackStatus": "played"});
        assert!(turn_latency_view(Some(&meta), "route-3").is_none());
        assert!(turn_latency_view(None, "route-3").is_none());
    }
}
