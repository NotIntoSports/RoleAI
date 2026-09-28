//! voice 域命令：音色参考的保存 / 音频上传 / 更新 / 列表 / 删除 / 克隆。
//! 纯搬移自 commands.rs，不含行为变更。
use super::*;

fn voice_reference_service_error<T: ts_rs::TS>(
    error: VoiceReferenceServiceError,
) -> CommandResult<T> {
    let code = error.code();
    let message = match code {
        "VOICE_REFERENCE_ID_INVALID" => "Voice reference id is invalid",
        "VOICE_REFERENCE_FIELDS_INVALID" => "Voice reference name must not be empty",
        "VOICE_REFERENCE_AUDIO_INVALID" => {
            "Reference audio must be an existing local .wav or .mp3 file"
        }
        "VOICE_REFERENCE_AUDIO_TOO_LARGE" => "Reference audio must be 10 MB or smaller",
        "VOICE_REFERENCE_AUDIO_TOO_SHORT" => "Recorded audio must be at least 3 seconds",
        "VOICE_REFERENCE_NOT_FOUND" => "Voice reference not found",
        "VOICE_REFERENCE_PROVIDER_MISSING" => {
            "Voice reference needs a model provider before cloning"
        }
        "PROVIDER_UNAUTHORIZED" => "Provider rejected the credentials",
        "PROVIDER_TIMEOUT" => "Provider request timed out",
        "PROVIDER_REQUEST_FAILED" => "Provider request failed",
        "PROVIDER_RESPONSE_INVALID" => "Provider response could not be parsed",
        "PROVIDER_RESPONSE_TOO_LARGE" => "Provider response is too large",
        "PROVIDER_ENDPOINT_INVALID" => "Provider endpoint is invalid",
        _ => "Voice reference operation failed",
    };
    service_error(code, message)
}

fn voice_reference_cmd<T: serde::Serialize + ts_rs::TS>(
    state: &AppState,
    work: impl FnOnce(&VoiceReferenceService<'_>) -> Result<T, VoiceReferenceServiceError>,
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
    let gateway = match VoiceCloneProbe::new() {
        Ok(gateway) => gateway,
        Err(error) => {
            return service_error(error.code(), "Provider client is unavailable");
        }
    };
    work(&VoiceReferenceService::new(
        database,
        &state.config,
        &state.secrets,
        &gateway,
    ))
    .map_or_else(voice_reference_service_error, |data| CommandResult::Ok {
        data,
    })
}

pub fn voice_reference_save_blocking(
    state: State<'_, AppState>,
    input: VoiceReferenceSaveInput,
) -> CommandResult<VoiceReferenceSummary> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    voice_reference_cmd(&state, |service| service.save(input))
}

pub fn voice_reference_save_audio_blocking(
    state: State<'_, AppState>,
    input: VoiceReferenceAudioSaveInput,
) -> CommandResult<VoiceReferenceSummary> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    voice_reference_cmd(&state, |service| service.save_audio(input))
}

pub fn voice_reference_update_blocking(
    state: State<'_, AppState>,
    input: VoiceReferenceUpdateInput,
) -> CommandResult<VoiceReferenceSummary> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    voice_reference_cmd(&state, |service| service.update_metadata(input))
}

pub fn voice_reference_list_blocking(
    state: State<'_, AppState>,
) -> CommandResult<Vec<VoiceReferenceSummary>> {
    voice_reference_cmd(&state, |service| service.list())
}

pub fn voice_reference_delete_blocking(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let result = voice_reference_cmd(&state, |service| service.delete(&id));
    match result {
        CommandResult::Ok { .. } => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        CommandResult::Err { error } => CommandResult::Err { error },
    }
}

pub fn voice_reference_clone_blocking(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<VoiceReferenceCloneResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    voice_reference_cmd(&state, |service| service.clone_voice(&id))
}
blocking_command!(voice_reference_save, voice_reference_save_blocking(input: VoiceReferenceSaveInput) -> VoiceReferenceSummary);
blocking_command!(voice_reference_save_audio, voice_reference_save_audio_blocking(input: VoiceReferenceAudioSaveInput) -> VoiceReferenceSummary);
blocking_command!(voice_reference_update, voice_reference_update_blocking(input: VoiceReferenceUpdateInput) -> VoiceReferenceSummary);
blocking_command!(voice_reference_list, voice_reference_list_blocking() -> Vec<VoiceReferenceSummary>);
blocking_command!(voice_reference_delete, voice_reference_delete_blocking(id: String) -> FoundationStatus);
blocking_command!(voice_reference_clone, voice_reference_clone_blocking(id: String) -> VoiceReferenceCloneResult);
