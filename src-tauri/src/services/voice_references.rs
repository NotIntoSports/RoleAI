use serde::{Deserialize, Serialize};
use ts_rs::TS;

use rusqlite::OptionalExtension;

use crate::{
    config::{ConfigError, ConfigStore, ProviderConfig},
    database::{Database, DatabaseError},
    providers::{ProviderEndpoint, ProviderError},
    secrets::{SecretError, SecretService},
};

use super::ids;

const MAX_AUDIO_BYTES: u64 = 10 * 1024 * 1024;
const MIN_AUDIO_MS: i64 = 3000;

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct VoiceReferenceSaveInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub transcript: Option<String>,
    pub audio_path: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct VoiceReferenceAudioSaveInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub transcript: Option<String>,
    pub audio_base64: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct VoiceReferenceUpdateInput {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub transcript: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct VoiceReferenceSummary {
    pub id: String,
    pub name: String,
    pub provider_id: Option<String>,
    pub mime_type: String,
    pub byte_size: i64,
    pub duration_ms: Option<i64>,
    pub transcript: String,
    pub remote_file_id: Option<String>,
    pub voice_id: Option<String>,
    pub clone_status: String,
    pub clone_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct VoiceReferenceCloneResult {
    pub reference_id: String,
    pub voice_id: String,
    pub remote_file_id: String,
}

#[derive(Debug)]
pub enum VoiceReferenceServiceError {
    InvalidId,
    FieldsInvalid,
    AudioInvalid,
    AudioTooLarge,
    AudioTooShort,
    NotFound,
    ProviderMissing,
    Database(DatabaseError),
    Config(ConfigError),
    Secret(SecretError),
    Provider(ProviderError),
}

impl VoiceReferenceServiceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidId => "VOICE_REFERENCE_ID_INVALID",
            Self::FieldsInvalid => "VOICE_REFERENCE_FIELDS_INVALID",
            Self::AudioInvalid => "VOICE_REFERENCE_AUDIO_INVALID",
            Self::AudioTooLarge => "VOICE_REFERENCE_AUDIO_TOO_LARGE",
            Self::AudioTooShort => "VOICE_REFERENCE_AUDIO_TOO_SHORT",
            Self::NotFound => "VOICE_REFERENCE_NOT_FOUND",
            Self::ProviderMissing => "VOICE_REFERENCE_PROVIDER_MISSING",
            Self::Database(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Secret(error) => error.code(),
            Self::Provider(error) => error.code(),
        }
    }
}

impl From<ProviderError> for VoiceReferenceServiceError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

/// 上传参考音频并调用音色克隆的网关抽象；生产实现包装智谱 files/voice-clone 接口。
pub trait VoiceCloneGateway {
    fn upload_sample(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String, crate::providers::VoiceCloneError>;

    fn clone_voice(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        voice_name: &str,
        transcript: &str,
        file_id: &str,
    ) -> Result<String, crate::providers::VoiceCloneError>;
}

impl VoiceCloneGateway for crate::providers::VoiceCloneProbe {
    fn upload_sample(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String, crate::providers::VoiceCloneError> {
        crate::providers::VoiceCloneProbe::upload_sample(
            self,
            endpoint,
            credential,
            file_name,
            mime_type,
            bytes,
        )
    }

    fn clone_voice(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        voice_name: &str,
        transcript: &str,
        file_id: &str,
    ) -> Result<String, crate::providers::VoiceCloneError> {
        crate::providers::VoiceCloneProbe::clone_voice(
            self,
            endpoint,
            credential,
            voice_name,
            transcript,
            file_id,
        )
    }
}

pub struct VoiceReferenceService<'a> {
    database: &'a Database,
    config: &'a ConfigStore,
    secrets: &'a SecretService,
    gateway: &'a dyn VoiceCloneGateway,
}

impl<'a> VoiceReferenceService<'a> {
    pub fn new(
        database: &'a Database,
        config: &'a ConfigStore,
        secrets: &'a SecretService,
        gateway: &'a dyn VoiceCloneGateway,
    ) -> Self {
        Self {
            database,
            config,
            secrets,
            gateway,
        }
    }

    pub fn save(
        &self,
        input: VoiceReferenceSaveInput,
    ) -> Result<VoiceReferenceSummary, VoiceReferenceServiceError> {
        let id = ids::resolve_optional_id(input.id.as_deref())
            .map_err(|_| VoiceReferenceServiceError::InvalidId)?;
        let name = input.name.trim();
        if name.is_empty() {
            return Err(VoiceReferenceServiceError::FieldsInvalid);
        }
        let transcript = input.transcript.as_deref().unwrap_or("").trim().to_owned();
        let (mime_type, bytes, duration_ms) = read_audio_file(&input.audio_path)?;
        let now = chrono::Utc::now().to_rfc3339();
        let provider_id = clean(input.provider_id);
        self.upsert_summary(&id, name, provider_id.as_deref(), &mime_type, &bytes, duration_ms, &transcript, &now)?;
        self.load_summary(&id)
    }

    /// 保存应用内录音或文件选择器读入的音频（base64 传输），按内容嗅探 wav/mp3。
    pub fn save_audio(
        &self,
        input: VoiceReferenceAudioSaveInput,
    ) -> Result<VoiceReferenceSummary, VoiceReferenceServiceError> {
        let id = ids::resolve_optional_id(input.id.as_deref())
            .map_err(|_| VoiceReferenceServiceError::InvalidId)?;
        let name = input.name.trim();
        if name.is_empty() {
            return Err(VoiceReferenceServiceError::FieldsInvalid);
        }
        let transcript = input.transcript.as_deref().unwrap_or("").trim().to_owned();
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(input.audio_base64.trim().as_bytes())
            .map_err(|_| VoiceReferenceServiceError::AudioInvalid)?;
        let (mime_type, duration_ms) = validate_audio_payload(&bytes)?;
        let now = chrono::Utc::now().to_rfc3339();
        let provider_id = clean(input.provider_id);
        self.upsert_summary(&id, name, provider_id.as_deref(), mime_type, &bytes, duration_ms, &transcript, &now)?;
        self.load_summary(&id)
    }

    /// 只更新名称/供应商/文字稿，保留音频与克隆状态——用于编辑既有条目而无需重新录音。
    pub fn update_metadata(
        &self,
        input: VoiceReferenceUpdateInput,
    ) -> Result<VoiceReferenceSummary, VoiceReferenceServiceError> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err(VoiceReferenceServiceError::FieldsInvalid);
        }
        let transcript = input.transcript.as_deref().unwrap_or("").trim().to_owned();
        let provider_id = clean(input.provider_id);
        let changed = self
            .database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE voice_references
                     SET name = ?2, provider_id = ?3, transcript = ?4, updated_at = ?5
                     WHERE id = ?1",
                    rusqlite::params![
                        input.id,
                        name,
                        provider_id,
                        transcript,
                        chrono::Utc::now().to_rfc3339()
                    ],
                )
            })
            .map_err(VoiceReferenceServiceError::Database)?;
        if changed == 0 {
            return Err(VoiceReferenceServiceError::NotFound);
        }
        self.load_summary(&input.id)
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_summary(
        &self,
        id: &str,
        name: &str,
        provider_id: Option<&str>,
        mime_type: &str,
        bytes: &[u8],
        duration_ms: Option<i64>,
        transcript: &str,
        now: &str,
    ) -> Result<(), VoiceReferenceServiceError> {
        self.database
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO voice_references(
                            id, name, provider_id, mime_type, byte_size, duration_ms, transcript,
                            audio, remote_file_id, voice_id, clone_status, clone_error,
                            created_at, updated_at
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, 'pending', NULL, ?9, ?9)
                        ON CONFLICT(id) DO UPDATE SET
                            name = excluded.name,
                            provider_id = excluded.provider_id,
                            mime_type = excluded.mime_type,
                            byte_size = excluded.byte_size,
                            duration_ms = excluded.duration_ms,
                            transcript = excluded.transcript,
                            audio = excluded.audio,
                            remote_file_id = NULL,
                            voice_id = NULL,
                            clone_status = 'pending',
                            clone_error = NULL,
                            updated_at = excluded.updated_at",
                        rusqlite::params![
                            id,
                            name,
                            provider_id,
                            mime_type,
                            bytes.len() as i64,
                            duration_ms,
                            transcript,
                            bytes,
                            now,
                        ],
                    )
                    .map(|_| ())
            })
            .map_err(VoiceReferenceServiceError::Database)
    }

    fn load_summary(
        &self,
        id: &str,
    ) -> Result<VoiceReferenceSummary, VoiceReferenceServiceError> {
        self.database
            .with_connection(|connection| query_summary(connection, id))
            .map_err(VoiceReferenceServiceError::Database)?
            .ok_or(VoiceReferenceServiceError::NotFound)
    }

    pub fn list(&self) -> Result<Vec<VoiceReferenceSummary>, VoiceReferenceServiceError> {
        self.database
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT id, name, provider_id, mime_type, byte_size, duration_ms,
                            transcript, remote_file_id, voice_id, clone_status, clone_error,
                            created_at, updated_at
                     FROM voice_references ORDER BY created_at, id",
                )?;
                statement
                    .query_map([], map_summary)?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(VoiceReferenceServiceError::Database)
    }

    pub fn delete(&self, reference_id: &str) -> Result<(), VoiceReferenceServiceError> {
        let deleted = self
            .database
            .with_connection(|connection| {
                connection.execute(
                    "DELETE FROM voice_references WHERE id = ?1",
                    rusqlite::params![reference_id],
                )
            })
            .map_err(VoiceReferenceServiceError::Database)?;
        if deleted == 0 {
            return Err(VoiceReferenceServiceError::NotFound);
        }
        Ok(())
    }

    /// 上传参考音频、调用音色克隆，并把返回的音色 ID 持久化到该条参考记录。
    pub fn clone_voice(
        &self,
        reference_id: &str,
    ) -> Result<VoiceReferenceCloneResult, VoiceReferenceServiceError> {
        let row = self
            .database
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT audio, mime_type, transcript, provider_id FROM voice_references
                         WHERE id = ?1",
                        rusqlite::params![reference_id],
                        |row| {
                            Ok((
                                row.get::<_, Vec<u8>>("audio")?,
                                row.get::<_, String>("mime_type")?,
                                row.get::<_, String>("transcript")?,
                                row.get::<_, Option<String>>("provider_id")?,
                            ))
                        },
                    )
                    .optional()
            })
            .map_err(VoiceReferenceServiceError::Database)?
            .ok_or(VoiceReferenceServiceError::NotFound)?;
        let (audio, mime_type, transcript, provider_id) = row;
        let provider_id =
            provider_id.filter(|value| !value.trim().is_empty()).ok_or(
                VoiceReferenceServiceError::ProviderMissing,
            )?;
        let config = self.config.load().map_err(VoiceReferenceServiceError::Config)?;
        let provider = config
            .models
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or(VoiceReferenceServiceError::ProviderMissing)?;
        if provider.base_url.trim().is_empty() {
            return Err(VoiceReferenceServiceError::ProviderMissing);
        }
        let endpoint = ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        };
        let credential = read_credential(provider, self.secrets)?;

        let upload = self.gateway.upload_sample(
            &endpoint,
            credential.as_deref(),
            &format!("{reference_id}.{}", audio_extension(&mime_type)),
            &mime_type,
            audio,
        );
        let remote_file_id = match upload {
            Ok(remote_file_id) => remote_file_id,
            Err(error) => {
                self.mark_failed(reference_id, &format!("上传参考音频失败：{}", error.detail()))?;
                return Err(VoiceReferenceServiceError::Provider(error.kind));
            }
        };
        self.mark_uploaded(reference_id, &remote_file_id)?;

        // 智谱限制音色名 ≤30 字符：前缀 7 位 + uuid4 简写前 23 位十六进制（约 92 位随机，撞名可忽略）。
        let voice_name = format!("roleai_{}", &uuid::Uuid::new_v4().simple().to_string()[..23]);
        debug_assert_eq!(voice_name.chars().count(), 30);
        let voice = match self.gateway.clone_voice(
            &endpoint,
            credential.as_deref(),
            &voice_name,
            &transcript,
            &remote_file_id,
        ) {
            Ok(voice) => voice,
            Err(error) => {
                self.mark_failed(reference_id, &format!("调用音色克隆失败：{}", error.detail()))?;
                return Err(VoiceReferenceServiceError::Provider(error.kind));
            }
        };
        self.database
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE voice_references SET voice_id = ?2, clone_status = 'cloned',
                                clone_error = NULL, updated_at = ?3 WHERE id = ?1",
                        rusqlite::params![reference_id, voice, chrono::Utc::now().to_rfc3339()],
                    )
                    .map(|_| ())
            })
            .map_err(VoiceReferenceServiceError::Database)?;
        Ok(VoiceReferenceCloneResult {
            reference_id: reference_id.to_owned(),
            voice_id: voice,
            remote_file_id,
        })
    }

    fn mark_uploaded(
        &self,
        reference_id: &str,
        remote_file_id: &str,
    ) -> Result<(), VoiceReferenceServiceError> {
        self.database
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE voice_references SET remote_file_id = ?2,
                                clone_status = 'uploaded', updated_at = ?3 WHERE id = ?1",
                        rusqlite::params![
                            reference_id,
                            remote_file_id,
                            chrono::Utc::now().to_rfc3339()
                        ],
                    )
                    .map(|_| ())
            })
            .map_err(VoiceReferenceServiceError::Database)
    }

    fn mark_failed(
        &self,
        reference_id: &str,
        detail: &str,
    ) -> Result<(), VoiceReferenceServiceError> {
        self.database
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE voice_references SET clone_status = 'failed', clone_error = ?2,
                                updated_at = ?3 WHERE id = ?1",
                        rusqlite::params![
                            reference_id,
                            detail,
                            chrono::Utc::now().to_rfc3339()
                        ],
                    )
                    .map(|_| ())
            })
            .map_err(VoiceReferenceServiceError::Database)
    }
}

fn query_summary(
    connection: &rusqlite::Connection,
    id: &str,
) -> rusqlite::Result<Option<VoiceReferenceSummary>> {
    connection
        .query_row(
            "SELECT id, name, provider_id, mime_type, byte_size, duration_ms, transcript,
                    remote_file_id, voice_id, clone_status, clone_error, created_at, updated_at
             FROM voice_references WHERE id = ?1",
            rusqlite::params![id],
            map_summary,
        )
        .optional()
}

fn map_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<VoiceReferenceSummary> {
    Ok(VoiceReferenceSummary {
        id: row.get("id")?,
        name: row.get("name")?,
        provider_id: row.get("provider_id")?,
        mime_type: row.get("mime_type")?,
        byte_size: row.get("byte_size")?,
        duration_ms: row.get("duration_ms")?,
        transcript: row.get("transcript")?,
        remote_file_id: row.get("remote_file_id")?,
        voice_id: row.get("voice_id")?,
        clone_status: row.get("clone_status")?,
        clone_error: row.get("clone_error")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn read_audio_file(
    raw_path: &str,
) -> Result<(String, Vec<u8>, Option<i64>), VoiceReferenceServiceError> {
    let path = std::path::Path::new(raw_path.trim());
    if path.is_relative() {
        return Err(VoiceReferenceServiceError::AudioInvalid);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| VoiceReferenceServiceError::AudioInvalid)?;
    if !canonical.is_file() {
        return Err(VoiceReferenceServiceError::AudioInvalid);
    }
    let (mime_type, is_wav) = match canonical
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("wav") => ("audio/wav", true),
        Some("mp3") => ("audio/mpeg", false),
        _ => return Err(VoiceReferenceServiceError::AudioInvalid),
    };
    let metadata = std::fs::metadata(&canonical).map_err(|_| VoiceReferenceServiceError::AudioInvalid)?;
    if metadata.len() == 0 {
        return Err(VoiceReferenceServiceError::AudioInvalid);
    }
    if metadata.len() > MAX_AUDIO_BYTES {
        return Err(VoiceReferenceServiceError::AudioTooLarge);
    }
    let bytes = std::fs::read(&canonical).map_err(|_| VoiceReferenceServiceError::AudioInvalid)?;
    let duration_ms = is_wav.then(|| wav_duration_ms(&bytes)).flatten();
    Ok((mime_type.to_owned(), bytes, duration_ms))
}

/// 解析 RIFF/WAVE 头部的 byte_rate 与 data 块长度估算时长；非 WAV 或畸形头返回 None。
fn wav_duration_ms(bytes: &[u8]) -> Option<i64> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut byte_rate = None;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size =
            u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        if chunk_id == b"fmt " && chunk_size >= 16 && offset + 8 + chunk_size <= bytes.len() {
            byte_rate = Some(u32::from_le_bytes(
                bytes[offset + 16..offset + 20].try_into().ok()?,
            ));
        } else if chunk_id == b"data" {
            let data_size = chunk_size.min(bytes.len().saturating_sub(offset + 8));
            let rate = byte_rate.filter(|rate| *rate > 0)?;
            return Some((data_size as i64 * 1000) / i64::from(rate));
        }
        offset += 8 + chunk_size + (chunk_size & 1);
    }
    None
}

fn audio_extension(mime_type: &str) -> &'static str {
    if mime_type == "audio/wav" {
        "wav"
    } else {
        "mp3"
    }
}

/// 校验录音/选文件载荷：按内容嗅探 wav/mp3、≤10MB；wav 时长需 ≥3 秒，mp3 时长无法廉价解析故交给供应商校验。
fn validate_audio_payload(
    bytes: &[u8],
) -> Result<(&'static str, Option<i64>), VoiceReferenceServiceError> {
    if bytes.is_empty() {
        return Err(VoiceReferenceServiceError::AudioInvalid);
    }
    if bytes.len() as u64 > MAX_AUDIO_BYTES {
        return Err(VoiceReferenceServiceError::AudioTooLarge);
    }
    if bytes.len() >= 12 && bytes[0..4] == *b"RIFF" && bytes[8..12] == *b"WAVE" {
        let duration_ms =
            wav_duration_ms(bytes).ok_or(VoiceReferenceServiceError::AudioInvalid)?;
        if duration_ms < MIN_AUDIO_MS {
            return Err(VoiceReferenceServiceError::AudioTooShort);
        }
        return Ok(("audio/wav", Some(duration_ms)));
    }
    let is_mp3 = (bytes.len() >= 3 && bytes[0..3] == *b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0);
    if is_mp3 {
        return Ok(("audio/mpeg", None));
    }
    Err(VoiceReferenceServiceError::AudioInvalid)
}

fn clean(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    })
}

fn read_credential(
    provider: &ProviderConfig,
    secrets: &SecretService,
) -> Result<Option<String>, VoiceReferenceServiceError> {
    let credential = provider
        .credential
        .as_ref()
        .filter(|slot| slot.configured)
        .map(|slot| secrets.read(&slot.reference))
        .transpose()
        .map_err(VoiceReferenceServiceError::Secret)?
        .flatten();
    if provider
        .credential
        .as_ref()
        .is_some_and(|slot| slot.configured)
        && credential.is_none()
    {
        return Err(VoiceReferenceServiceError::Secret(SecretError::Backend));
    }
    Ok(credential.map(|value| value.to_string()))
}
