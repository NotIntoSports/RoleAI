//! practice 域命令：模拟面试训练的题单生成/保存/列表/删除。
//! 题单生成复用 livestream 的离线 LLM 通道模式（active route → secret → complete → JSON 校验）；
//! 训练会话与报告命令在后续卡片扩展本文件。
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
