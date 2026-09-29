use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

use crate::{
    config::{ConfigError, ConfigStore},
    database::{Database, DatabaseError},
    materials::{
        BackupError, BackupService, CHUNKER_VERSION, MaterialStore, NewMaterial, ParseError,
        chunk_text, extract_text, hybrid,
        parse::{MEDIA_DOCX, MEDIA_MARKDOWN, MEDIA_PDF, MEDIA_PLAIN},
        parser_version,
        store::sha256_hex,
    },
    providers::{EmbeddingError, EmbeddingProbe},
    secrets::{SecretError, SecretService},
};

pub use crate::materials::{EmbeddingSpace, MaterialSearchHit};

const MAX_MATERIAL_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MaterialSummary {
    pub id: String,
    pub file_name: String,
    pub content_sha256: String,
    pub media_type: String,
    #[ts(type = "number")]
    pub byte_size: i64,
    pub status: String,
    #[ts(type = "number")]
    pub chunk_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MaterialIndexResult {
    #[ts(type = "number")]
    pub indexed_chunks: i64,
    pub status: String,
}

#[derive(Debug)]
pub enum MaterialServiceError {
    TypeUnsupported,
    TooLarge,
    NotUtf8,
    NoTextLayer,
    ParseFailed,
    NotFound,
    PathInvalid,
    ParseBudget,
    Operation,
    EmbeddingNotReady,
    EmbeddingNotFound,
    EmbeddingFieldsInvalid,
    Embedding(EmbeddingError),
    Secret(SecretError),
    Config(ConfigError),
}

impl MaterialServiceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::TypeUnsupported => "MATERIAL_TYPE_UNSUPPORTED",
            Self::TooLarge => "MATERIAL_TOO_LARGE",
            Self::NotUtf8 => "MATERIAL_NOT_UTF8",
            Self::NoTextLayer => "MATERIAL_NO_TEXT_LAYER",
            Self::ParseFailed => "MATERIAL_PARSE_FAILED",
            Self::NotFound => "MATERIAL_NOT_FOUND",
            Self::PathInvalid => "MATERIAL_PATH_INVALID",
            Self::ParseBudget => "MATERIAL_PARSE_BUDGET",
            Self::Operation => "MATERIAL_OPERATION_FAILED",
            Self::EmbeddingNotReady => "EMBEDDING_NOT_READY",
            Self::EmbeddingNotFound => "EMBEDDING_NOT_FOUND",
            Self::EmbeddingFieldsInvalid => "EMBEDDING_FIELDS_INVALID",
            Self::Embedding(error) => error.code(),
            Self::Secret(error) => error.code(),
            Self::Config(error) => error.code(),
        }
    }
}

impl From<DatabaseError> for MaterialServiceError {
    fn from(_: DatabaseError) -> Self {
        Self::Operation
    }
}

impl From<ParseError> for MaterialServiceError {
    fn from(error: ParseError) -> Self {
        match error {
            ParseError::NotUtf8 => Self::NotUtf8,
            ParseError::NoTextLayer => Self::NoTextLayer,
            ParseError::ParseFailed => Self::ParseFailed,
            ParseError::BudgetExceeded => Self::ParseBudget,
        }
    }
}

pub struct MaterialService<'a> {
    database: &'a Database,
    data_directory: &'a Path,
}

impl<'a> MaterialService<'a> {
    pub fn new(database: &'a Database, data_directory: &'a Path) -> Self {
        Self {
            database,
            data_directory,
        }
    }

    pub fn import_file(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<MaterialSummary, MaterialServiceError> {
        let source = source.as_ref();
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let media_type = match extension.as_str() {
            "txt" => MEDIA_PLAIN,
            "md" => MEDIA_MARKDOWN,
            "pdf" => MEDIA_PDF,
            "docx" => MEDIA_DOCX,
            _ => return Err(MaterialServiceError::TypeUnsupported),
        };
        let metadata = std::fs::metadata(source).map_err(|_| MaterialServiceError::Operation)?;
        if metadata.len() > MAX_MATERIAL_BYTES {
            return Err(MaterialServiceError::TooLarge);
        }
        let bytes = std::fs::read(source).map_err(|_| MaterialServiceError::Operation)?;
        if bytes.len() as u64 > MAX_MATERIAL_BYTES {
            return Err(MaterialServiceError::TooLarge);
        }
        let content_sha256 = sha256_hex(&bytes);
        let store = MaterialStore::new(self.database);
        if let Some(existing) = store.find_by_hash(&content_sha256)? {
            return Ok(summary(existing));
        }
        let text = extract_text(media_type, &bytes)?;
        let file_name = source
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(MaterialServiceError::Operation)?;
        let label = source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(file_name);
        let chunks = chunk_text(label, &text);
        let id = uuid::Uuid::new_v4().to_string();
        let stored_path = format!("materials/{id}.{extension}");
        let destination = self.data_directory.join(&stored_path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|_| MaterialServiceError::Operation)?;
        }
        std::fs::write(&destination, &bytes).map_err(|_| MaterialServiceError::Operation)?;
        let insert = store.insert_text_ready(NewMaterial {
            id: &id,
            file_name,
            stored_path: &stored_path,
            content_sha256: &content_sha256,
            media_type,
            byte_size: bytes.len() as i64,
            parser_version: parser_version(media_type),
            chunker_version: CHUNKER_VERSION,
            extracted_text: &text,
            chunks: &chunks,
        });
        if insert.is_err() {
            let _ = std::fs::remove_file(&destination);
            insert?;
        }
        store
            .get(&id)?
            .map(summary)
            .ok_or(MaterialServiceError::Operation)
    }

    pub fn delete(&self, id: &str) -> Result<(), MaterialServiceError> {
        let store = MaterialStore::new(self.database);
        let stored_path = store
            .block_retrieval(id)?
            .ok_or(MaterialServiceError::NotFound)?;
        store.delete_indexed_rows(id)?;
        let destination = self.data_directory.join(&stored_path);
        if destination.exists() && std::fs::remove_file(&destination).is_err() {
            store.enqueue_cleanup(&stored_path, "MATERIAL_FILE_DELETE_FAILED")?;
        }
        Ok(())
    }

    /// 清理队列重试：目标已不存在（此前已被他处删除）或本次删除成功的条目
    /// 移出队列；仍删除失败的留在队列等下次。
    pub fn retry_pending_cleanups(&self) -> Result<(), MaterialServiceError> {
        let store = MaterialStore::new(self.database);
        for stored_path in store.cleanup_paths()? {
            let destination = self.data_directory.join(&stored_path);
            if destination.exists() && std::fs::remove_file(&destination).is_err() {
                continue;
            }
            store.remove_cleanup_path(&stored_path)?;
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<MaterialSummary>, MaterialServiceError> {
        Ok(MaterialStore::new(self.database)
            .list()?
            .into_iter()
            .map(summary)
            .collect())
    }

    pub fn search_text(
        &self,
        query: &str,
        top_k: Option<u32>,
    ) -> Result<Vec<MaterialSearchHit>, MaterialServiceError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        Ok(MaterialStore::new(self.database).search_text(query, top_k.unwrap_or(20))?)
    }

    pub fn index_chunks(
        &self,
        space: &EmbeddingSpace,
        probe: &dyn EmbeddingProbe,
    ) -> Result<(), MaterialServiceError> {
        hybrid::index_chunks(self.database, space, probe)?;
        Ok(())
    }

    pub fn index_library(
        &self,
        config: &ConfigStore,
        secrets: &SecretService,
        probe: &dyn EmbeddingProbe,
    ) -> Result<MaterialIndexResult, MaterialServiceError> {
        let loaded = config.load().map_err(MaterialServiceError::Config)?;
        let embedding_id = loaded
            .knowledge
            .active_embedding_config_id
            .as_deref()
            .ok_or(MaterialServiceError::EmbeddingNotReady)?;
        let embedding = loaded
            .knowledge
            .embedding_configs
            .iter()
            .find(|item| item.id == embedding_id)
            .ok_or(MaterialServiceError::EmbeddingNotFound)?;
        if !embedding.active {
            return Err(MaterialServiceError::EmbeddingNotReady);
        }
        let endpoint = super::embedding_endpoint(&loaded.models, embedding)
            .ok_or(MaterialServiceError::EmbeddingFieldsInvalid)?;
        if endpoint.base_url.trim().is_empty() {
            return Err(MaterialServiceError::EmbeddingFieldsInvalid);
        }
        let slot = super::embedding_credential_slot(&loaded.models, embedding);
        let credential = slot
            .map(|slot| secrets.read(&slot.reference))
            .transpose()
            .map_err(MaterialServiceError::Secret)?
            .flatten();
        if slot.is_some() && credential.is_none() {
            return Err(MaterialServiceError::Secret(SecretError::Backend));
        }
        let space = EmbeddingSpace {
            provider_id: super::embedding_space_provider_id(embedding),
            model_id: embedding.model_id.clone(),
            dimensions: embedding.dimensions,
            normalized: embedding.normalized,
        };
        hybrid::index_chunks_at(
            self.database,
            &space,
            probe,
            &endpoint,
            credential.as_ref().map(|value| value.as_str()),
        )?;
        let indexed_chunks = self.database.with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM material_chunks WHERE embedding_status = 'ready'",
                [],
                |row| row.get(0),
            )
        })?;
        let status = if indexed_chunks > 0 {
            "vector_ready"
        } else {
            "text_ready"
        };
        Ok(MaterialIndexResult {
            indexed_chunks,
            status: status.into(),
        })
    }

    pub fn search_hybrid(
        &self,
        query: &str,
        query_vector: Option<&[f32]>,
        top_k: Option<u32>,
    ) -> Result<Vec<MaterialSearchHit>, MaterialServiceError> {
        Ok(hybrid::search_hybrid(
            self.database,
            query,
            query_vector,
            top_k,
        )?)
    }

    pub fn backup_library(
        &self,
        config_store: &ConfigStore,
        archive_directory: impl AsRef<Path>,
    ) -> Result<(), BackupError> {
        BackupService::new(self.database, self.data_directory, config_store)
            .create(archive_directory)
    }

    pub fn restore_library(
        &self,
        config_store: &ConfigStore,
        archive_directory: impl AsRef<Path>,
    ) -> Result<(), BackupError> {
        BackupService::new(self.database, self.data_directory, config_store)
            .restore(archive_directory)
    }
}

fn summary(record: crate::materials::store::MaterialRecord) -> MaterialSummary {
    MaterialSummary {
        id: record.id,
        file_name: record.file_name,
        content_sha256: record.content_sha256,
        media_type: record.media_type,
        byte_size: record.byte_size,
        status: record.status,
        chunk_count: record.chunk_count,
    }
}

#[cfg(test)]
mod tests;
