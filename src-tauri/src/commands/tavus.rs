//! Tavus 视频通话命令。密钥只进凭据管理器，返回值里没有密钥。

use super::*;
use crate::providers::{TavusConversationStarted, TavusFace, TavusHttpClient};
use crate::services::{TavusCallError, TavusCallService};

fn call_error<T: ts_rs::TS>(error: TavusCallError) -> CommandResult<T> {
    let code = error.code();
    service_error(code, &error.message())
}

pub fn tavus_credential_status_blocking(
    state: State<'_, AppState>,
) -> CommandResult<crate::contracts::SecretStatus> {
    let client = match TavusHttpClient::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "连不上 Tavus"),
    };
    TavusCallService::new(&state.config, &state.secrets, &client)
        .credential_status()
        .map_or_else(call_error, |data| CommandResult::Ok { data })
}

pub fn tavus_credential_save_blocking(
    state: State<'_, AppState>,
    api_key: String,
) -> CommandResult<crate::contracts::SecretStatus> {
    if api_key.len() > 128 || api_key.chars().any(|ch| ch.is_control()) {
        return service_error("TAVUS_KEY_INVALID", "Tavus 密钥无效");
    }
    let client = match TavusHttpClient::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "连不上 Tavus"),
    };
    TavusCallService::new(&state.config, &state.secrets, &client)
        .save_credential(&api_key)
        .map_or_else(call_error, |data| CommandResult::Ok { data })
}

pub fn tavus_list_faces_blocking(state: State<'_, AppState>) -> CommandResult<Vec<TavusFace>> {
    let client = match TavusHttpClient::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "连不上 Tavus"),
    };
    TavusCallService::new(&state.config, &state.secrets, &client)
        .list_faces()
        .map_or_else(call_error, |data| CommandResult::Ok { data })
}

pub fn tavus_conversation_start_blocking(
    state: State<'_, AppState>,
    role_profile_id: String,
) -> CommandResult<TavusConversationStarted> {
    let client = match TavusHttpClient::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "连不上 Tavus"),
    };
    TavusCallService::new(&state.config, &state.secrets, &client)
        .start(role_profile_id.trim())
        .map_or_else(call_error, |data| CommandResult::Ok { data })
}

pub fn tavus_conversation_end_blocking(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CommandResult<FoundationStatus> {
    let client = match TavusHttpClient::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "连不上 Tavus"),
    };
    TavusCallService::new(&state.config, &state.secrets, &client)
        .end(conversation_id.trim())
        .map_or_else(call_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

blocking_command!(tavus_credential_status, tavus_credential_status_blocking() -> crate::contracts::SecretStatus);
blocking_command!(tavus_credential_save, tavus_credential_save_blocking(api_key: String) -> crate::contracts::SecretStatus);
blocking_command!(tavus_list_faces, tavus_list_faces_blocking() -> Vec<TavusFace>);
blocking_command!(tavus_conversation_start, tavus_conversation_start_blocking(role_profile_id: String) -> TavusConversationStarted);
blocking_command!(tavus_conversation_end, tavus_conversation_end_blocking(conversation_id: String) -> FoundationStatus);
