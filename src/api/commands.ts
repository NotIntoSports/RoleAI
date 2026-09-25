import { invoke } from "@tauri-apps/api/core";

import type { AgentCommandInput, AgentCommandResult, CommandResult, DiagnosticsExportResult, EmbeddingConfig, EmbeddingConfigSaveInput, EmbeddingTestResult, FoundationStatus, LegacyMigrationStatus, LegacySessionImport, LiveKitConfig, LiveKitJoinToken, LiveKitSettingsSaveInput, LiveKitTestResult, LivestreamDraftInput, LivestreamGenerateInput, LivestreamRuntime, MaterialIndexResult, MaterialSearchHit, MaterialSummary, MicPcmAcceptance, ModelDiscoveryResult, ObsRuntimeStatus, ProviderConfig, ProviderSaveInput, ProviderTestResult, PublicConfig, RoleProfileConfig, RoleProfileCopyInput, RoleProfileSaveInput, RuntimeStatus, SecretStatus, SessionDetail, SessionExportResult, SessionStartResult, SessionSummary, SessionTurnView, StartupState, VoiceReferenceAudioSaveInput, VoiceReferenceCloneResult, VoiceReferenceSaveInput, VoiceReferenceSummary, VoiceReferenceUpdateInput, VoiceRouteConfig, VoiceRouteSaveInput, VoiceRouteTestResult } from "../generated/bindings";

export function getFoundationStatus() {
  return invoke<CommandResult<FoundationStatus>>("foundation_get_status");
}

export function exportDiagnostics(destination: string) {
  return invoke<CommandResult<DiagnosticsExportResult>>("diagnostics_export", { destination });
}

export function getStartupState() {
  return invoke<CommandResult<StartupState>>("config_get_startup_state");
}

export function getLegacyMigrationStatus() {
  return invoke<CommandResult<LegacyMigrationStatus>>("legacy_migration_status");
}

export function importLegacySource(path: string) {
  return invoke<CommandResult<LegacySessionImport>>("legacy_import_source", { path });
}

export function getConfigPublic() {
  return invoke<CommandResult<PublicConfig>>("config_get_public");
}

export function saveModelProvider(input: ProviderSaveInput) {
  return invoke<CommandResult<ProviderConfig>>("model_provider_save", { input });
}

export function testModelProvider(providerId: string) {
  return invoke<CommandResult<ProviderTestResult>>("model_provider_test", { providerId });
}

export function discoverModelProvider(providerId: string) {
  return invoke<CommandResult<ModelDiscoveryResult>>("model_provider_discover", { providerId });
}

export function activateModelProvider(providerId: string) {
  return invoke<CommandResult<ProviderConfig>>("model_provider_activate", { providerId });
}

export function deleteModelProvider(providerId: string) {
  return invoke<CommandResult<FoundationStatus>>("model_provider_delete", { providerId });
}

export function getModelProviderDependencies(providerId: string) {
  return invoke<CommandResult<import("../generated/bindings").ProviderDependency[]>>("model_provider_dependencies", { providerId });
}

export function saveSpeechRoute(input: VoiceRouteSaveInput) {
  return invoke<CommandResult<VoiceRouteConfig>>("speech_route_save", { input });
}

export function testSpeechRoute(routeId: string) {
  return invoke<CommandResult<VoiceRouteTestResult>>("speech_route_test", { routeId });
}

export function activateSpeechRoute(routeId: string) {
  return invoke<CommandResult<VoiceRouteConfig>>("speech_route_activate", { routeId });
}

export function deleteSpeechRoute(routeId: string) {
  return invoke<CommandResult<FoundationStatus>>("speech_route_delete", { routeId });
}

export function saveRoleProfile(input: RoleProfileSaveInput) {
  return invoke<CommandResult<RoleProfileConfig>>("role_profile_save", { input });
}

export function copyRoleProfile(input: RoleProfileCopyInput) {
  return invoke<CommandResult<RoleProfileConfig>>("role_profile_copy", { input });
}

export function activateRoleProfile(roleId: string) {
  return invoke<CommandResult<RoleProfileConfig>>("role_profile_activate", { roleId });
}

export function deleteRoleProfile(roleId: string) {
  return invoke<CommandResult<FoundationStatus>>("role_profile_delete", { roleId });
}

export function saveEmbeddingConfig(input: EmbeddingConfigSaveInput) {
  return invoke<CommandResult<EmbeddingConfig>>("embedding_config_save", { input });
}

export function testEmbeddingConfig(embeddingId: string) {
  return invoke<CommandResult<EmbeddingTestResult>>("embedding_config_test", { embeddingId });
}

export function activateEmbeddingConfig(embeddingId: string) {
  return invoke<CommandResult<EmbeddingConfig>>("embedding_config_activate", { embeddingId });
}

export function deleteEmbeddingConfig(embeddingId: string) {
  return invoke<CommandResult<FoundationStatus>>("embedding_config_delete", { embeddingId });
}

export function saveLiveKitSettings(input: LiveKitSettingsSaveInput) {
  return invoke<CommandResult<LiveKitConfig>>("livekit_settings_save", { input });
}

export function testLiveKitSettings() {
  return invoke<CommandResult<LiveKitTestResult>>("livekit_settings_test");
}

export function enableLiveKitSettings(enabled: boolean) {
  return invoke<CommandResult<LiveKitConfig>>("livekit_settings_enable", { enabled });
}

export function issueLiveKitJoinToken(room: string, identity: string) {
  return invoke<CommandResult<LiveKitJoinToken>>("livekit_issue_join_token", { room, identity });
}

export function restoreLastGoodConfig() {
  return invoke<CommandResult<StartupState>>("config_restore_last_good");
}

export function restoreDefaultConfig() {
  return invoke<CommandResult<StartupState>>("config_restore_defaults");
}

export function openAppDirectory(kind: "config" | "data") {
  return invoke<CommandResult<FoundationStatus>>("open_app_directory", { kind });
}

export function listMaterials() {
  return invoke<CommandResult<MaterialSummary[]>>("material_list");
}

export function importMaterial(path: string) {
  return invoke<CommandResult<MaterialSummary>>("material_import", { path });
}

export function searchMaterials(query: string, topK?: number) {
  return invoke<CommandResult<MaterialSearchHit[]>>("material_search", { query, topK });
}

export function deleteMaterial(id: string) {
  return invoke<CommandResult<FoundationStatus>>("material_delete", { id });
}

export function indexMaterials() {
  return invoke<CommandResult<MaterialIndexResult>>("material_index");
}

export function listMeetingProcesses() {
  return invoke<CommandResult<import("../generated/bindings").MeetingProcess[]>>("meeting_process_list");
}

export function listAudioOutputs() {
  return invoke<CommandResult<import("../generated/bindings").AudioOutputDevice[]>>("audio_output_list");
}

export function getVirtualAudioStatus() {
  return invoke<CommandResult<import("../generated/bindings").VirtualAudioPreparation>>("virtual_audio_status");
}

export function installVirtualAudio() {
  return invoke<CommandResult<import("../generated/bindings").VirtualAudioPreparation>>("virtual_audio_install");
}

export function startSession(transportMode?: "direct" | "livekit", selection?: { roleProfileId: string; voiceRouteId: string; allowWebSearch?: boolean; meetingPid?: number; outputDeviceId?: string }) {
  return invoke<CommandResult<SessionStartResult>>("session_start", { transportMode, ...selection });
}

export function stopSession() {
  return invoke<CommandResult<SessionSummary>>("session_stop");
}

export function openWebSource(url: string) {
  return invoke<CommandResult<FoundationStatus>>("open_web_source", { url });
}

export function setSessionMode(mode: "ai_active" | "operator_speaking" | "paused" | "muted") {
  return invoke<CommandResult<RuntimeStatus>>("session_set_mode", { mode });
}

export function exportSession(sessionId: string, format: "markdown" | "json" | "text") {
  return invoke<CommandResult<SessionExportResult>>("session_export", { sessionId, format });
}

export function listSessions() {
  return invoke<CommandResult<SessionSummary[]>>("session_list");
}

export function getSession(sessionId: string) {
  return invoke<CommandResult<SessionDetail>>("session_get", { sessionId });
}

export function deleteSession(sessionId: string) {
  return invoke<CommandResult<FoundationStatus>>("session_delete", { sessionId });
}

export function finalizeSessionUtterance(text: string) {
  return invoke<CommandResult<SessionTurnView>>("session_finalize_utterance", { text });
}

export function triggerMeetingAssistant() {
  return invoke<CommandResult<SessionTurnView>>("session_trigger_assistant");
}

export function sessionAgentCommand(input: AgentCommandInput) {
  return invoke<CommandResult<AgentCommandResult>>("session_agent_command", { input });
}

export function getRuntimeStatus() {
  return invoke<CommandResult<RuntimeStatus>>("runtime_get_status");
}

export function isSessionAudioReady() {
  return invoke<CommandResult<FoundationStatus>>("session_audio_ready");
}

export function pushMicPcm(pcm: string, sampleRate: number) {
  return invoke<CommandResult<MicPcmAcceptance>>("session_push_mic_pcm", { pcm, sampleRate });
}

export function createLivestreamDraft(input: LivestreamDraftInput) {
  return invoke<CommandResult<LivestreamRuntime>>("livestream_create_draft", { input });
}

export function generateLivestream(input: LivestreamGenerateInput) {
  return invoke<CommandResult<LivestreamRuntime>>("livestream_generate", { input });
}

export function getLivestream() {
  return invoke<CommandResult<LivestreamRuntime>>("livestream_get");
}

export function controlLivestream(action: "confirm" | "start" | "pause" | "takeover" | "resume" | "previous" | "next" | "replay" | "complete") {
  return invoke<CommandResult<LivestreamRuntime>>("livestream_control", { action });
}

export function insertLivestreamQuestion(question: string) {
  return invoke<CommandResult<LivestreamRuntime>>("livestream_insert_question", { question });
}

export function getObsRuntimeStatus() {
  return invoke<CommandResult<ObsRuntimeStatus>>("obs_runtime_status");
}

export function startObsVirtualCamera() {
  return invoke<CommandResult<ObsRuntimeStatus>>("obs_virtual_camera_start");
}

export function stopObsVirtualCamera() {
  return invoke<CommandResult<ObsRuntimeStatus>>("obs_virtual_camera_stop");
}

export function getObsPasswordStatus() {
  return invoke<CommandResult<SecretStatus>>("obs_password_status");
}

export function saveObsPassword(password: string) {
  return invoke<CommandResult<SecretStatus>>("obs_password_save", { password });
}

export function listVoiceReferences() {
  return invoke<CommandResult<VoiceReferenceSummary[]>>("voice_reference_list");
}

export function saveVoiceReference(input: VoiceReferenceSaveInput) {
  return invoke<CommandResult<VoiceReferenceSummary>>("voice_reference_save", { input });
}

export function cloneVoiceReference(id: string) {
  return invoke<CommandResult<VoiceReferenceCloneResult>>("voice_reference_clone", { id });
}

export function deleteVoiceReference(id: string) {
  return invoke<CommandResult<FoundationStatus>>("voice_reference_delete", { id });
}

export function saveVoiceReferenceAudio(input: VoiceReferenceAudioSaveInput) {
  return invoke<CommandResult<VoiceReferenceSummary>>("voice_reference_save_audio", { input });
}

export function updateVoiceReference(input: VoiceReferenceUpdateInput) {
  return invoke<CommandResult<VoiceReferenceSummary>>("voice_reference_update", { input });
}
