//! practice 域命令：模拟面试训练的题单生成/保存/列表/删除与训练会话推进。
//! 题单生成复用 livestream 的离线 LLM 通道模式（active route → secret → complete → JSON 校验）；
//! 训练会话沿用现有会话启动路径，题单 overlay 注入本地 config 副本中的角色 style_instructions。
use super::*;

fn practice_input_error<T: ts_rs::TS>() -> CommandResult<T> {
    service_error("PRACTICE_PLAN_INVALID", "题单参数无效")
}

fn validate_plan_generate_input(input: &PracticePlanGenerateInput) -> bool {
    let position = input.position.trim().chars().count();
    let style = input.interviewer_style.trim().chars().count();
    let difficulty = input.difficulty.trim().chars().count();
    let material_ok = |id: &Option<String>| {
        id.as_deref()
            .is_none_or(|id| !id.is_empty() && id.len() <= 128)
    };
    (1..=80).contains(&position)
        && (1..=40).contains(&style)
        && (1..=20).contains(&difficulty)
        && (1..=12).contains(&input.question_count)
        && material_ok(&input.jd_material_id)
        && material_ok(&input.resume_material_id)
}

/// 读取一条资料的导出文本片段（与 livestream 相同的 48KB 上限与就绪条件）。
fn material_excerpt(
    database: &crate::database::Database,
    id: &str,
    label: &str,
) -> Result<String, ()> {
    let row = database.with_connection(|connection| {
        connection.query_row(
            "SELECT m.file_name, d.extracted_text
             FROM materials m JOIN material_documents d ON d.material_id = m.id
             WHERE m.id = ?1 AND m.retrieval_blocked = 0 AND m.status = 'text_ready'",
            rusqlite::params![id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
    });
    match row {
        Ok((name, text)) => {
            let excerpt: String = text.chars().take(48 * 1024).collect();
            Ok(format!("资料（{label}）：{name}\n{excerpt}"))
        }
        Err(_) => Err(()),
    }
}

pub(in crate::commands) fn practice_plan_generate_cmd(
    state: &AppState,
    input: PracticePlanGenerateInput,
) -> CommandResult<PracticePlan> {
    if !validate_plan_generate_input(&input) {
        return practice_input_error();
    }
    let documents = {
        let database_slot = match state.database.lock() {
            Ok(slot) => slot,
            Err(_) => return service_error("DATABASE_OPERATION_FAILED", "资料库暂时不可用"),
        };
        let Some(database) = database_slot.as_ref() else {
            return service_error("DATABASE_OPERATION_FAILED", "资料库尚未就绪");
        };
        let mut documents = Vec::new();
        if let Some(id) = input.jd_material_id.as_deref() {
            match material_excerpt(database, id, "岗位 JD") {
                Ok(excerpt) => documents.push(excerpt),
                Err(()) => {
                    return service_error("PRACTICE_MATERIAL_NOT_READY", "所选 JD 资料不可用");
                }
            }
        }
        if let Some(id) = input.resume_material_id.as_deref() {
            match material_excerpt(database, id, "我的简历") {
                Ok(excerpt) => documents.push(excerpt),
                Err(()) => {
                    return service_error("PRACTICE_MATERIAL_NOT_READY", "所选简历资料不可用");
                }
            }
        }
        documents
    };
    let config = match state.config.load() {
        Ok(config) => public_view(&config),
        Err(error) => return service_error(error.code(), "模型配置不可用"),
    };
    let route = match active_voice_route(&config) {
        Some(route) if route.mode == crate::config::VoiceRouteMode::Cascaded => route,
        _ => return service_error("PRACTICE_MODEL_REQUIRED", "请选择可用的级联语音线路"),
    };
    let provider_id = match route.llm_provider_id.as_deref() {
        Some(id) => id,
        None => return service_error("PRACTICE_MODEL_REQUIRED", "训练模型尚未配置"),
    };
    let model_id = match route.llm_model_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return service_error("PRACTICE_MODEL_REQUIRED", "训练模型尚未配置"),
    };
    let provider = match config
        .models
        .providers
        .iter()
        .find(|item| item.id == provider_id)
    {
        Some(provider) => provider,
        None => return service_error("PRACTICE_MODEL_REQUIRED", "训练模型供应商不存在"),
    };
    let secret = match read_provider_secret(state, &config, Some(provider_id)) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let model = match OpenAiCompatibleCascade::new() {
        Ok(model) => model,
        Err(_) => return service_error("PRACTICE_MODEL_UNAVAILABLE", "无法初始化训练模型"),
    };
    let endpoint = ProviderEndpoint {
        provider_id: provider.id.clone(),
        base_url: provider.base_url.clone(),
    };
    let questions = match crate::practice::plan::generate_plan_with_model(
        &model,
        &endpoint,
        secret.as_deref().map(|value| value.as_str()),
        model_id,
        &input,
        &documents,
    ) {
        Ok(questions) => questions,
        Err(error) => return service_error(error.code(), "题单生成失败，可稍后重试"),
    };
    let now = chrono::Utc::now().to_rfc3339();
    CommandResult::Ok {
        data: PracticePlan {
            id: uuid::Uuid::new_v4().to_string(),
            title: format!(
                "{}·{}",
                input.position.trim(),
                input.interviewer_style.trim()
            ),
            position: input.position.trim().to_owned(),
            interviewer_style: input.interviewer_style.trim().to_owned(),
            difficulty: input.difficulty.trim().to_owned(),
            questions,
            created_at: now.clone(),
            updated_at: now,
        },
    }
}

fn validate_saved_plan(plan: &PracticePlan) -> bool {
    let id_ok = !plan.id.is_empty() && plan.id.len() <= 128;
    let title_ok = (1..=120).contains(&plan.title.trim().chars().count());
    let position_ok = plan.position.trim().chars().count() <= 80;
    let style_ok = plan.interviewer_style.trim().chars().count() <= 40;
    let difficulty_ok = plan.difficulty.trim().chars().count() <= 20;
    let questions_ok = (1..=12).contains(&plan.questions.len())
        && plan
            .questions
            .iter()
            .all(|question| !question.prompt.trim().is_empty());
    id_ok && title_ok && position_ok && style_ok && difficulty_ok && questions_ok
}

pub(in crate::commands) fn practice_plan_save_cmd(
    state: &AppState,
    input: PracticePlan,
) -> CommandResult<PracticePlan> {
    if !validate_saved_plan(&input) {
        return practice_input_error();
    }
    let database_slot = match state.database.lock() {
        Ok(slot) => slot,
        Err(_) => return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable"),
    };
    let Some(database) = database_slot.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let mut plan = input;
    plan.position = plan.position.trim().to_owned();
    plan.interviewer_style = plan.interviewer_style.trim().to_owned();
    plan.difficulty = plan.difficulty.trim().to_owned();
    plan.updated_at = chrono::Utc::now().to_rfc3339();
    match crate::practice::store::PracticePlanStore::new(database).save(&plan) {
        Ok(()) => CommandResult::Ok { data: plan },
        Err(_) => service_error("DATABASE_OPERATION_FAILED", "题单保存失败"),
    }
}

pub(in crate::commands) fn practice_plan_list_cmd(
    state: &AppState,
) -> CommandResult<Vec<PracticePlanSummary>> {
    let database_slot = match state.database.lock() {
        Ok(slot) => slot,
        Err(_) => return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable"),
    };
    let Some(database) = database_slot.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    match crate::practice::store::PracticePlanStore::new(database).list() {
        Ok(rows) => CommandResult::Ok { data: rows },
        Err(_) => service_error("DATABASE_OPERATION_FAILED", "题单列表失败"),
    }
}

pub(in crate::commands) fn practice_plan_delete_cmd(
    state: &AppState,
    plan_id: String,
) -> CommandResult<FoundationStatus> {
    let database_slot = match state.database.lock() {
        Ok(slot) => slot,
        Err(_) => return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable"),
    };
    let Some(database) = database_slot.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    match crate::practice::store::PracticePlanStore::new(database).delete(&plan_id) {
        Ok(_) => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        Err(_) => service_error("DATABASE_OPERATION_FAILED", "题单删除失败"),
    }
}

#[tauri::command]
pub fn practice_plan_generate(
    state: State<'_, AppState>,
    input: PracticePlanGenerateInput,
) -> CommandResult<PracticePlan> {
    practice_plan_generate_cmd(&state, input)
}

pub fn practice_plan_save_blocking(
    state: State<'_, AppState>,
    input: PracticePlan,
) -> CommandResult<PracticePlan> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    practice_plan_save_cmd(&state, input)
}

pub fn practice_plan_list_blocking(
    state: State<'_, AppState>,
) -> CommandResult<Vec<PracticePlanSummary>> {
    practice_plan_list_cmd(&state)
}

pub fn practice_plan_delete_blocking(
    state: State<'_, AppState>,
    plan_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    practice_plan_delete_cmd(&state, plan_id)
}

blocking_command!(practice_plan_save, practice_plan_save_blocking(input: PracticePlan) -> PracticePlan);
blocking_command!(practice_plan_list, practice_plan_list_blocking() -> Vec<PracticePlanSummary>);
blocking_command!(practice_plan_delete, practice_plan_delete_blocking(plan_id: String) -> FoundationStatus);

// ---------- 训练会话（E04）：题单注入、进度与跳过 ----------

fn load_plan(
    database: &crate::database::Database,
    plan_id: &str,
) -> Result<PracticePlan, PublicError> {
    match crate::practice::store::PracticePlanStore::new(database).get(plan_id) {
        Ok(Some(plan)) => Ok(plan),
        Ok(None) => Err(PublicError::new(
            "PRACTICE_PLAN_NOT_FOUND",
            "题单不存在或已删除",
            false,
        )),
        Err(_) => Err(PublicError::new(
            "DATABASE_OPERATION_FAILED",
            "题单读取失败",
            false,
        )),
    }
}

fn load_plan_for_state(state: &AppState, plan_id: &str) -> Result<PracticePlan, PublicError> {
    let database_slot = match state.database.lock() {
        Ok(slot) => slot,
        Err(_) => return Err(database_busy()),
    };
    let Some(database) = database_slot.as_ref() else {
        return Err(database_busy());
    };
    load_plan(database, plan_id)
}

/// 收集会话的 practice_meta 事件引用（kind, payload）。
fn practice_meta_events(
    database: &crate::database::Database,
    session_id: &str,
) -> Result<Vec<(String, String)>, PublicError> {
    let events = SessionStore::new(database)
        .list_events(session_id)
        .map_err(|_| database_busy())?;
    Ok(events
        .into_iter()
        .filter(|event| event.kind == "practice_meta")
        .map(|event| (event.kind, event.payload))
        .collect())
}

fn database_busy() -> PublicError {
    PublicError::new(
        "DATABASE_OPERATION_FAILED",
        "Database is unavailable",
        false,
    )
}

/// 读取会话对应的题单并重建推进器；会话不存在/非训练会话/题单缺失都映射为公共错误。
fn session_practice_director(
    database: &crate::database::Database,
    session_id: &str,
) -> Result<crate::practice::director::PracticeDirector, PublicError> {
    let store = SessionStore::new(database);
    let exists = store
        .get(session_id)
        .map_err(|_| database_busy())?
        .is_some();
    if !exists {
        return Err(PublicError::new(
            "SESSION_NOT_FOUND",
            "Session not found",
            false,
        ));
    }
    let events = practice_meta_events(database, session_id)?;
    let plan_id = events
        .iter()
        .rev()
        .find_map(|(_, payload)| {
            serde_json::from_str::<serde_json::Value>(payload)
                .ok()?
                .get("planId")?
                .as_str()
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            PublicError::new(
                "PRACTICE_SESSION_STATE_INVALID",
                "该会话不是模拟面试训练",
                false,
            )
        })?;
    let plan = load_plan(database, &plan_id)?;
    let refs: Vec<(&str, &str)> = events
        .iter()
        .map(|(kind, payload)| (kind.as_str(), payload.as_str()))
        .collect();
    crate::practice::director::from_events(
        &plan,
        crate::practice::director::DEFAULT_FOLLOWUP_LIMIT,
        &refs,
    )
    .map_err(|_| PublicError::new("PRACTICE_PLAN_INVALID", "训练状态无效", false))
}

fn command_of<T: ts_rs::TS>(result: Result<T, PublicError>) -> CommandResult<T> {
    match result {
        Ok(data) => CommandResult::Ok { data },
        Err(error) => CommandResult::Err { error },
    }
}

/// 持数据库锁执行一个训练状态操作（进度/跳过共用）。
/// 调用方在此锁内不得再锁 state.database（会死锁）。
fn with_session_database<T>(
    state: &AppState,
    work: impl FnOnce(&crate::database::Database) -> Result<T, PublicError>,
) -> Result<T, PublicError> {
    let database_slot = match state.database.lock() {
        Ok(slot) => slot,
        Err(_) => return Err(database_busy()),
    };
    let Some(database) = database_slot.as_ref() else {
        return Err(database_busy());
    };
    work(database)
}

pub(in crate::commands) fn practice_session_start_cmd(
    state: &AppState,
    input: PracticeSessionStartInput,
) -> CommandResult<SessionStartResult> {
    if input.plan_id.is_empty() || input.plan_id.len() > 128 {
        return practice_input_error();
    }
    let plan = match load_plan_for_state(state, &input.plan_id) {
        Ok(plan) => plan,
        Err(error) => return CommandResult::Err { error },
    };
    let director = match crate::practice::director::PracticeDirector::from_plan(
        &plan,
        crate::practice::director::DEFAULT_FOLLOWUP_LIMIT,
    ) {
        Ok(director) => director,
        Err(_) => return practice_input_error(),
    };

    let mut config = match load_public_config(state) {
        Ok(config) => config,
        Err(error) => return CommandResult::Err { error },
    };
    // 与 session_start_selected_cmd 相同的角色/线路校验与激活逻辑。
    if let Some(id) = input.role_profile_id.as_deref() {
        if !config
            .role_profiles
            .iter()
            .any(|role| role.id == id && role.config_version > 0)
        {
            return service_error("SESSION_ROLE_REQUIRED", "请选择有效的会话角色");
        }
        config.active_role_profile_id = Some(id.into());
    }
    for role in &mut config.role_profiles {
        role.active = config.active_role_profile_id.as_deref() == Some(role.id.as_str());
    }
    // 把训练规则注入**本地 config 副本**中被选角色的 style_instructions：
    // 沿用现有角色提示词机制，不写回配置持久层。
    let injected = config
        .role_profiles
        .iter_mut()
        .find(|role| config.active_role_profile_id.as_deref() == Some(role.id.as_str()));
    let Some(role) = injected else {
        return service_error("SESSION_ROLE_REQUIRED", "请选择有效的会话角色");
    };
    let overlay = director.overlay();
    if role.style_instructions.trim().is_empty() {
        role.style_instructions = overlay;
    } else {
        role.style_instructions = format!("{}\n\n{}", role.style_instructions.trim(), overlay);
    }

    if let Some(id) = input.voice_route_id.as_deref() {
        if !config
            .speech
            .voice_routes
            .iter()
            .any(|route| route.id == id && route.config_version > 0)
        {
            return service_error("SESSION_ROUTE_REQUIRED", "请选择有效的语音线路");
        }
        config.speech.active_voice_route_id = Some(id.into());
        for route in &mut config.speech.voice_routes {
            route.active = route.id == id;
        }
    }
    let secrets_ready = secrets_backend_ready(state);
    let database_slot = match state.database.lock() {
        Ok(slot) => slot,
        Err(_) => return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable"),
    };
    let Some(database) = database_slot.as_ref() else {
        return CommandResult::Ok {
            data: SessionStartResult::Blocked {
                issues: preflight(&config, secrets_ready, false),
            },
        };
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
    let outcome = sessions.start(database, &config, secrets_ready, false);
    match outcome {
        Ok(SessionStartOutcome::Started { session }) => {
            // 训练元数据：记录题单 id 与起始题号（practice_meta 事件）。
            if SessionStore::new(database)
                .append_event(
                    &session.id,
                    "practice_meta",
                    &director.event_payload("start"),
                )
                .is_err()
            {
                return service_error("DATABASE_OPERATION_FAILED", "训练状态记录失败");
            }
            CommandResult::Ok {
                data: SessionStartResult::Started {
                    session: session.into(),
                },
            }
        }
        Ok(SessionStartOutcome::Blocked { issues }) => CommandResult::Ok {
            data: SessionStartResult::Blocked { issues },
        },
        Err(error) => session_service_error(error),
    }
}

/// 训练进度：practice_meta 事件优先，辅以转写中的【第X题】标注（取更大者）。
pub(in crate::commands) fn practice_session_progress_cmd(
    state: &AppState,
    session_id: String,
) -> CommandResult<PracticeProgress> {
    let outcome = with_session_database(state, |database| {
        let director = session_practice_director(database, &session_id)?;
        let mut progress = director.progress();
        // 转写标注兜底：事件落后时以面试官最新【第X题】标注为准（只前进不回退）。
        let markers = SessionStore::new(database)
            .list_turns(&session_id)
            .map_err(|_| database_busy())?
            .into_iter()
            .filter_map(|turn| {
                crate::practice::director::question_number_from_transcript(&[turn.assistant_text])
            })
            .max();
        if let Some(marker) = markers {
            let marker_index = marker.saturating_sub(1).min(progress.total_questions);
            if marker_index > progress.question_index {
                progress.question_index = marker_index;
                progress.followups_used = 0;
                progress.finished = marker_index >= progress.total_questions;
            }
        }
        Ok(progress)
    });
    command_of(outcome)
}

/// 跳过当前题：推进状态机、写 practice_meta 事件，
/// 并把跳过提示追加到会话滚动摘要，让面试官在下一轮立即进入下一题。
pub(in crate::commands) fn practice_session_skip_cmd(
    state: &AppState,
    session_id: String,
) -> CommandResult<PracticeProgress> {
    let outcome = with_session_database(state, |database| {
        let store = SessionStore::new(database);
        let mut director = session_practice_director(database, &session_id)?;
        if director.skip() {
            store
                .append_event(
                    &session_id,
                    "practice_meta",
                    &director.event_payload("skip"),
                )
                .map_err(|_| {
                    PublicError::new("DATABASE_OPERATION_FAILED", "训练状态记录失败", false)
                })?;
            // 提示注入滚动摘要（追加而非覆盖），下一轮 build_messages 即可见。
            let note = format!(
                "[模拟面试训练] 候选人请求跳过，请立即进入下一题（当前第{}题/共{}题）。",
                director.question_number(),
                director.progress().total_questions
            );
            let merged = match store
                .context_summary(&session_id)
                .map_err(|_| database_busy())?
            {
                Some((existing, upto)) => {
                    let combined = if existing.contains(&note) {
                        existing
                    } else {
                        format!("{existing}\n{note}")
                    };
                    (combined, upto)
                }
                None => (note, 0),
            };
            store
                .set_context_summary(&session_id, &merged.0, merged.1)
                .map_err(|_| {
                    PublicError::new("DATABASE_OPERATION_FAILED", "训练提示写入失败", false)
                })?;
        }
        Ok(director.progress())
    });
    command_of(outcome)
}

#[tauri::command]
pub fn practice_session_start(
    state: State<'_, AppState>,
    input: PracticeSessionStartInput,
) -> CommandResult<SessionStartResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    practice_session_start_cmd(&state, input)
}

pub fn practice_session_progress_blocking(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<PracticeProgress> {
    practice_session_progress_cmd(&state, session_id)
}

pub fn practice_session_skip_blocking(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<PracticeProgress> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    practice_session_skip_cmd(&state, session_id)
}

blocking_command!(practice_session_progress, practice_session_progress_blocking(session_id: String) -> PracticeProgress);
blocking_command!(practice_session_skip, practice_session_skip_blocking(session_id: String) -> PracticeProgress);
