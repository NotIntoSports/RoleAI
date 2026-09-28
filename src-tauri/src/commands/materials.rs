//! materials 域命令：知识库材料的列出 / 导入 / 检索 / 删除 / 索引。
//! 纯搬移自 commands.rs，不含行为变更。
use super::*;

fn material_service_error<T: ts_rs::TS>(error: MaterialServiceError) -> CommandResult<T> {
    let code = error.code();
    let message = match code {
        "MATERIAL_TYPE_UNSUPPORTED" => "Unsupported material type",
        "MATERIAL_TOO_LARGE" => "Material is too large",
        "MATERIAL_NOT_UTF8" => "Material is not valid UTF-8",
        "MATERIAL_NO_TEXT_LAYER" => "Material has no extractable text",
        "MATERIAL_PARSE_FAILED" => "Material could not be parsed",
        "MATERIAL_NOT_FOUND" => "Material not found",
        "MATERIAL_PATH_INVALID" => "Material path is invalid",
        "MATERIAL_PARSE_BUDGET" => "Material parse budget was exceeded",
        "EMBEDDING_NOT_READY" => "Embedding configuration is not ready",
        "EMBEDDING_NOT_FOUND" => "Embedding configuration was not found",
        "EMBEDDING_FIELDS_INVALID" => "Embedding configuration is invalid",
        _ => "Material operation failed",
    };
    let mut public = PublicError::new(code, message, false);
    if let Some(field) = match code {
        "MATERIAL_PATH_INVALID" => Some("path"),
        "MATERIAL_NOT_FOUND" => Some("id"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

fn resolve_import_path(path: &str) -> Result<std::path::PathBuf, MaterialServiceError> {
    let path = std::path::Path::new(path);
    if path.is_relative() {
        return Err(MaterialServiceError::PathInvalid);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| MaterialServiceError::PathInvalid)?;
    if !canonical.is_file() {
        return Err(MaterialServiceError::PathInvalid);
    }
    Ok(canonical)
}

fn with_materials<T: serde::Serialize + ts_rs::TS>(
    state: &AppState,
    work: impl FnOnce(&MaterialService<'_>) -> Result<T, MaterialServiceError>,
) -> CommandResult<T> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    work(&MaterialService::new(database, &state.paths.data_directory))
        .map_or_else(material_service_error, |data| CommandResult::Ok { data })
}

pub(super) fn material_list_cmd(state: &AppState) -> CommandResult<Vec<MaterialSummary>> {
    with_materials(state, |service| service.list())
}

pub(super) fn material_import_cmd(state: &AppState, path: String) -> CommandResult<MaterialSummary> {
    let path = match resolve_import_path(&path) {
        Ok(path) => path,
        Err(error) => return material_service_error(error),
    };
    with_materials(state, |service| service.import_file(path))
}

pub(super) fn material_search_cmd(
    state: &AppState,
    query: String,
    top_k: Option<u32>,
) -> CommandResult<Vec<MaterialSearchHit>> {
    with_materials(state, |service| service.search_text(&query, top_k))
}

pub(super) fn material_delete_cmd(state: &AppState, id: String) -> CommandResult<FoundationStatus> {
    with_materials(state, |service| {
        service.delete(&id)?;
        Ok(FoundationStatus { ready: true })
    })
}

pub(super) fn material_index_cmd(state: &AppState) -> CommandResult<MaterialIndexResult> {
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    with_materials(state, |service| {
        service.index_library(&state.config, &state.secrets, &probe)
    })
}

pub fn material_list_blocking(state: State<'_, AppState>) -> CommandResult<Vec<MaterialSummary>> {
    material_list_cmd(&state)
}

pub fn material_import_blocking(
    state: State<'_, AppState>,
    path: String,
) -> CommandResult<MaterialSummary> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    material_import_cmd(&state, path)
}

pub fn material_search_blocking(
    state: State<'_, AppState>,
    query: String,
    top_k: Option<u32>,
) -> CommandResult<Vec<MaterialSearchHit>> {
    material_search_cmd(&state, query, top_k)
}

pub fn material_delete_blocking(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    material_delete_cmd(&state, id)
}

pub fn material_index_blocking(state: State<'_, AppState>) -> CommandResult<MaterialIndexResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    material_index_cmd(&state)
}

blocking_command!(material_list, material_list_blocking() -> Vec<MaterialSummary>);
blocking_command!(material_import, material_import_blocking(path: String) -> MaterialSummary);
blocking_command!(material_search, material_search_blocking(query: String, top_k: Option<u32>) -> Vec<MaterialSearchHit>);
blocking_command!(material_delete, material_delete_blocking(id: String) -> FoundationStatus);
blocking_command!(material_index, material_index_blocking() -> MaterialIndexResult);
