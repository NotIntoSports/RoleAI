use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, backup::Backup};
use serde::{Deserialize, Serialize};

use crate::{
    config::{AppConfigV1, ConfigStore},
    database::Database,
    materials::store::sha256_hex,
};

const ARCHIVE_MANIFEST: &str = "manifest.json";
const ARCHIVE_CONFIG: &str = "config.json";
const ARCHIVE_SQLITE: &str = "app.sqlite";
const ARCHIVE_MATERIALS: &str = "materials";
const RESTORE_ROLLBACK: &str = ".restore-rollback";
const RESTORE_JOURNAL: &str = ".restore-journal.json";
const BACKUP_PAGES: i32 = 100;
const BACKUP_PAUSE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreFault {
    AfterDatabase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupError {
    Integrity,
    HashMismatch,
    SecretForbidden,
    MaterialMissing,
    Operation,
}

impl BackupError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Integrity => "BACKUP_INTEGRITY_FAILED",
            Self::HashMismatch => "BACKUP_HASH_MISMATCH",
            Self::SecretForbidden => "BACKUP_SECRET_FORBIDDEN",
            Self::MaterialMissing => "BACKUP_MATERIAL_MISSING",
            Self::Operation => "BACKUP_OPERATION_FAILED",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    schema_version: i64,
    app_version: String,
    hashes: BTreeMap<String, String>,
}

pub struct BackupService<'a> {
    database: &'a Database,
    data_directory: &'a Path,
    config_store: &'a ConfigStore,
}

impl<'a> BackupService<'a> {
    pub fn new(
        database: &'a Database,
        data_directory: &'a Path,
        config_store: &'a ConfigStore,
    ) -> Self {
        Self {
            database,
            data_directory,
            config_store,
        }
    }

    pub fn create(&self, archive_directory: impl AsRef<Path>) -> Result<(), BackupError> {
        let archive = archive_directory.as_ref();
        require_integrity(self.database)?;
        fs::create_dir_all(archive).map_err(|_| BackupError::Operation)?;
        online_backup_to_path(self.database, &archive.join(ARCHIVE_SQLITE))?;
        copy_directory(
            &self.data_directory.join(ARCHIVE_MATERIALS),
            &archive.join(ARCHIVE_MATERIALS),
        )?;
        write_scrubbed_config(self.config_store, &archive.join(ARCHIVE_CONFIG))?;
        let hashes = hash_archive_files(archive)?;
        let manifest = BackupManifest {
            schema_version: schema_version(self.database)?,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            hashes,
        };
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).map_err(|_| BackupError::Operation)?;
        reject_secret_bytes(&manifest_bytes)?;
        fs::write(archive.join(ARCHIVE_MANIFEST), manifest_bytes)
            .map_err(|_| BackupError::Operation)?;
        Ok(())
    }

    pub fn restore(&self, archive_directory: impl AsRef<Path>) -> Result<(), BackupError> {
        self.restore_inner(archive_directory.as_ref(), None)
    }

    #[cfg(test)]
    fn restore_failing_after_database(
        &self,
        archive_directory: impl AsRef<Path>,
    ) -> Result<(), BackupError> {
        self.restore_inner(
            archive_directory.as_ref(),
            Some(RestoreFault::AfterDatabase),
        )
    }

    pub fn recover_interrupted_restore(&self) -> Result<(), BackupError> {
        if !self.data_directory.join(RESTORE_JOURNAL).exists() {
            return Ok(());
        }
        self.rollback_live_from_snapshot()
    }

    fn restore_inner(
        &self,
        archive_directory: &Path,
        fault: Option<RestoreFault>,
    ) -> Result<(), BackupError> {
        let staged = stage_archive(archive_directory)?;
        let staged_root = staged.as_path();
        let manifest = read_manifest(&staged_root.join(ARCHIVE_MANIFEST))?;
        verify_hashes(staged_root, &manifest)?;
        let archive_db = open_archive_sqlite(&staged_root.join(ARCHIVE_SQLITE))?;
        require_integrity(&archive_db)?;
        archive_db.migrate().map_err(|_| BackupError::Operation)?;
        confirm_material_files(&archive_db, staged_root)?;
        let restored = read_scrubbed_config(&staged_root.join(ARCHIVE_CONFIG))?;
        drop(archive_db);

        let incoming_materials = self.data_directory.join(".materials.restoring");
        if incoming_materials.exists() {
            fs::remove_dir_all(&incoming_materials).map_err(|_| BackupError::Operation)?;
        }
        copy_directory(&staged_root.join(ARCHIVE_MATERIALS), &incoming_materials)?;
        self.snapshot_live_for_restore()?;
        write_restore_journal(self.data_directory, "database")?;
        let committed = (|| {
            online_backup_into(self.database, &staged_root.join(ARCHIVE_SQLITE))?;
            if fault == Some(RestoreFault::AfterDatabase) {
                return Err(BackupError::Operation);
            }
            write_restore_journal(self.data_directory, "materials")?;
            replace_directory(
                &self.data_directory.join(ARCHIVE_MATERIALS),
                &incoming_materials,
            )?;
            write_restore_journal(self.data_directory, "config")?;
            self.config_store
                .update(|config| {
                    *config = restored;
                    Ok(())
                })
                .map_err(|_| BackupError::Operation)?;
            write_restore_journal(self.data_directory, "done")?;
            Ok(())
        })();
        if committed.is_err() {
            let _ = fs::remove_dir_all(&incoming_materials);
            self.rollback_live_from_snapshot()?;
        } else {
            clear_restore_journal(self.data_directory);
        }
        committed
    }

    fn snapshot_live_for_restore(&self) -> Result<(), BackupError> {
        let rollback = self.data_directory.join(RESTORE_ROLLBACK);
        if rollback.exists() {
            fs::remove_dir_all(&rollback).map_err(|_| BackupError::Operation)?;
        }
        fs::create_dir_all(&rollback).map_err(|_| BackupError::Operation)?;
        online_backup_to_path(self.database, &rollback.join("app.sqlite3"))?;
        copy_directory(
            &self.data_directory.join(ARCHIVE_MATERIALS),
            &rollback.join(ARCHIVE_MATERIALS),
        )?;
        write_scrubbed_config(self.config_store, &rollback.join(ARCHIVE_CONFIG))?;
        write_restore_journal(self.data_directory, "snapshotted")?;
        Ok(())
    }

    fn rollback_live_from_snapshot(&self) -> Result<(), BackupError> {
        let rollback = self.data_directory.join(RESTORE_ROLLBACK);
        if !rollback.exists() {
            clear_restore_journal(self.data_directory);
            return Ok(());
        }
        online_backup_into(self.database, &rollback.join("app.sqlite3"))?;
        let rollback_materials = rollback.join(ARCHIVE_MATERIALS);
        let staged_materials = self.data_directory.join(".materials.restoring-rollback");
        if staged_materials.exists() {
            fs::remove_dir_all(&staged_materials).map_err(|_| BackupError::Operation)?;
        }
        copy_directory(&rollback_materials, &staged_materials)?;
        replace_directory(
            &self.data_directory.join(ARCHIVE_MATERIALS),
            &staged_materials,
        )?;
        let restored = read_scrubbed_config(&rollback.join(ARCHIVE_CONFIG))?;
        self.config_store
            .update(|config| {
                *config = restored;
                Ok(())
            })
            .map_err(|_| BackupError::Operation)?;
        let _ = fs::remove_dir_all(&rollback);
        clear_restore_journal(self.data_directory);
        Ok(())
    }
}

fn write_restore_journal(data_directory: &Path, phase: &str) -> Result<(), BackupError> {
    let payload = serde_json::json!({ "phase": phase });
    fs::write(
        data_directory.join(RESTORE_JOURNAL),
        serde_json::to_vec(&payload).map_err(|_| BackupError::Operation)?,
    )
    .map_err(|_| BackupError::Operation)
}

fn clear_restore_journal(data_directory: &Path) {
    let _ = fs::remove_file(data_directory.join(RESTORE_JOURNAL));
    let _ = fs::remove_dir_all(data_directory.join(RESTORE_ROLLBACK));
}

pub(crate) fn require_integrity(database: &Database) -> Result<(), BackupError> {
    let check = database
        .integrity_check()
        .map_err(|_| BackupError::Integrity)?;
    if check == "ok" {
        Ok(())
    } else {
        Err(BackupError::Integrity)
    }
}

fn schema_version(database: &Database) -> Result<i64, BackupError> {
    database
        .with_connection(|connection| {
            connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
        })
        .map_err(|_| BackupError::Operation)
}

fn online_backup_to_path(database: &Database, destination: &Path) -> Result<(), BackupError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|_| BackupError::Operation)?;
    }
    database
        .with_connection(|source| {
            let mut dest = Connection::open(destination)?;
            {
                let backup = Backup::new(source, &mut dest)?;
                backup.run_to_completion(BACKUP_PAGES, BACKUP_PAUSE, None)?;
            }
            let check: String = dest.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            if check != "ok" {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok(())
        })
        .map_err(|_| BackupError::Integrity)
}

fn online_backup_into(database: &Database, source_path: &Path) -> Result<(), BackupError> {
    database
        .with_connection_mut(|dest| {
            let source = Connection::open(source_path)?;
            let backup = Backup::new(&source, dest)?;
            backup.run_to_completion(BACKUP_PAGES, BACKUP_PAUSE, None)?;
            Ok(())
        })
        .map_err(|_| BackupError::Integrity)
}

pub(crate) fn write_scrubbed_config(store: &ConfigStore, path: &Path) -> Result<(), BackupError> {
    let config = store.load().map_err(|_| BackupError::Operation)?;
    let bytes = serde_json::to_vec_pretty(&config).map_err(|_| BackupError::Operation)?;
    reject_secret_bytes(&bytes)?;
    fs::write(path, bytes).map_err(|_| BackupError::Operation)
}

fn read_scrubbed_config(path: &Path) -> Result<AppConfigV1, BackupError> {
    let json = fs::read_to_string(path).map_err(|_| BackupError::Operation)?;
    reject_secret_text(&json)?;
    AppConfigV1::from_json(&json).map_err(|error| {
        if error.code() == "CONFIG_SECRET_INLINE_FORBIDDEN" {
            BackupError::SecretForbidden
        } else {
            BackupError::Operation
        }
    })
}

fn read_manifest(path: &Path) -> Result<BackupManifest, BackupError> {
    let json = fs::read_to_string(path).map_err(|_| BackupError::Operation)?;
    reject_secret_text(&json)?;
    serde_json::from_str(&json).map_err(|_| BackupError::Operation)
}

fn hash_archive_files(archive: &Path) -> Result<BTreeMap<String, String>, BackupError> {
    let mut hashes = BTreeMap::new();
    for name in [ARCHIVE_CONFIG, ARCHIVE_SQLITE] {
        hashes.insert(name.to_owned(), file_sha256(&archive.join(name))?);
    }
    for file in list_files(&archive.join(ARCHIVE_MATERIALS))? {
        let key = relative_key(archive, &file)?;
        hashes.insert(key, file_sha256(&file)?);
    }
    if hashes.len() < 2 {
        return Err(BackupError::Operation);
    }
    Ok(hashes)
}

fn verify_hashes(archive: &Path, manifest: &BackupManifest) -> Result<(), BackupError> {
    let current = hash_archive_files(archive)?;
    if current != manifest.hashes {
        return Err(BackupError::HashMismatch);
    }
    verify_file_hashes(archive, &manifest.hashes)
}

pub(crate) fn verify_file_hashes(
    root: &Path,
    hashes: &BTreeMap<String, String>,
) -> Result<(), BackupError> {
    for (key, expected) in hashes {
        let path = join_archive_key(root, key);
        if !path.is_file() {
            return Err(BackupError::HashMismatch);
        }
        let actual = file_sha256(&path)?;
        if actual != *expected {
            return Err(BackupError::HashMismatch);
        }
    }
    Ok(())
}

fn join_archive_key(root: &Path, key: &str) -> PathBuf {
    key.split('/')
        .filter(|part| !part.is_empty())
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
}

fn confirm_material_files(database: &Database, archive: &Path) -> Result<(), BackupError> {
    let stored_paths = database
        .with_connection(|connection| {
            let mut statement = connection.prepare("SELECT stored_path FROM materials")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| BackupError::Operation)?;
    for stored_path in stored_paths {
        if !archive.join(&stored_path).is_file() {
            return Err(BackupError::MaterialMissing);
        }
    }
    Ok(())
}

struct StagingDir(PathBuf);

impl StagingDir {
    fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stage_archive(archive: &Path) -> Result<StagingDir, BackupError> {
    let staged = std::env::temp_dir().join(format!("materials-restore-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staged).map_err(|_| BackupError::Operation)?;
    let staged = StagingDir(staged);
    for name in [ARCHIVE_MANIFEST, ARCHIVE_CONFIG, ARCHIVE_SQLITE] {
        let source = archive.join(name);
        if !source.is_file() {
            return Err(BackupError::Operation);
        }
        fs::copy(&source, staged.as_path().join(name)).map_err(|_| BackupError::Operation)?;
    }
    copy_directory(
        &archive.join(ARCHIVE_MATERIALS),
        &staged.as_path().join(ARCHIVE_MATERIALS),
    )?;
    Ok(staged)
}

fn open_archive_sqlite(path: &Path) -> Result<Database, BackupError> {
    Database::open(path).map_err(|_| BackupError::Integrity)
}

fn copy_directory(from: &Path, to: &Path) -> Result<(), BackupError> {
    fs::create_dir_all(to).map_err(|_| BackupError::Operation)?;
    if !from.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(from).map_err(|_| BackupError::Operation)? {
        let entry = entry.map_err(|_| BackupError::Operation)?;
        let destination = to.join(entry.file_name());
        let file_type = entry.file_type().map_err(|_| BackupError::Operation)?;
        if file_type.is_dir() {
            copy_directory(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination).map_err(|_| BackupError::Operation)?;
        }
    }
    Ok(())
}

fn replace_directory(live: &Path, incoming: &Path) -> Result<(), BackupError> {
    let previous = live.with_file_name(".materials.previous");
    if previous.exists() {
        fs::remove_dir_all(&previous).map_err(|_| BackupError::Operation)?;
    }
    if live.exists() {
        fs::rename(live, &previous).map_err(|_| BackupError::Operation)?;
    }
    if fs::rename(incoming, live).is_err() {
        if previous.exists() {
            let _ = fs::rename(&previous, live);
        }
        let _ = fs::remove_dir_all(incoming);
        return Err(BackupError::Operation);
    }
    if previous.exists() {
        fs::remove_dir_all(&previous).map_err(|_| BackupError::Operation)?;
    }
    Ok(())
}

fn list_files(root: &Path) -> Result<Vec<PathBuf>, BackupError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), BackupError> {
    for entry in fs::read_dir(root).map_err(|_| BackupError::Operation)? {
        let entry = entry.map_err(|_| BackupError::Operation)?;
        let file_type = entry.file_type().map_err(|_| BackupError::Operation)?;
        if file_type.is_dir() {
            collect_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn relative_key(root: &Path, file: &Path) -> Result<String, BackupError> {
    let relative = file
        .strip_prefix(root)
        .map_err(|_| BackupError::Operation)?;
    Ok(relative
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

pub(crate) fn file_sha256(path: &Path) -> Result<String, BackupError> {
    let bytes = fs::read(path).map_err(|_| BackupError::Operation)?;
    Ok(sha256_hex(&bytes))
}

pub(crate) fn reject_secret_bytes(bytes: &[u8]) -> Result<(), BackupError> {
    reject_secret_text(&String::from_utf8_lossy(bytes))
}

fn reject_secret_text(text: &str) -> Result<(), BackupError> {
    let lower = text.to_ascii_lowercase();
    if ["password", "secretvalue", "secretcontents", "\"token\""]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return Err(BackupError::SecretForbidden);
    }
    if text
        .split(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_'))
        .any(|token| token.len() >= 16 && (token.starts_with("sk-") || token.starts_with("sk_")))
    {
        return Err(BackupError::SecretForbidden);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::BackupService;
    use crate::{
        config::{ConfigStore, ProviderConfig, SecretSlot},
        database::Database,
        materials::{hybrid::EmbeddingSpace, MaterialStore},
        providers::{EmbeddingError, EmbeddingProbe, ProviderEndpoint},
        services::MaterialService,
    };

    const SECRET_DECOY: &str = "sk-live-backup-forbidden-key";
    const IMPORT_BODY: &str = "工作经历\n2019.03-2021.06 负责订单服务与 Kafka 链路。";

    fn opened(directory: &tempfile::TempDir) -> Database {
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        database
    }

    fn seeded_config(directory: &tempfile::TempDir) -> ConfigStore {
        let store = ConfigStore::new(directory.path().join("config.json"));
        store
            .update(|config| {
                config.models.providers.push(ProviderConfig {
                    web_capability: None,
                    id: "p1".into(),
                    name: Some("Example".into()),
                    base_url: "https://example.com/v1".into(),
                    credential: Some(SecretSlot {
                        reference: "providers/p1/api-key".into(),
                        configured: true,
                    }),
                });
                Ok(())
            })
            .unwrap();
        std::fs::write(directory.path().join("sidecar-secret.txt"), SECRET_DECOY).unwrap();
        store
    }

    fn write_import(directory: &tempfile::TempDir, name: &str, body: &str) -> std::path::PathBuf {
        let path = directory.path().join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn contains_secret_material(text: &str) -> bool {
        let lower = text.to_ascii_lowercase();
        [
            "password",
            "secretvalue",
            "secretcontents",
            "\"token\"",
            SECRET_DECOY,
        ]
        .iter()
        .any(|needle| lower.contains(&needle.to_ascii_lowercase()))
    }

    fn archive_entries(archive: &std::path::Path) -> [std::path::PathBuf; 4] {
        [
            archive.join("manifest.json"),
            archive.join("config.json"),
            archive.join("app.sqlite"),
            archive.join("materials"),
        ]
    }

    fn material_files(root: &std::path::Path) -> Vec<String> {
        let materials = root.join("materials");
        if !materials.is_dir() {
            return Vec::new();
        }
        let mut names = std::fs::read_dir(&materials)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn live_searchable(database: &Database, query: &str) -> i64 {
        crate::materials::MaterialStore::new(database)
            .searchable_chunk_count(query)
            .unwrap()
    }

    fn set_manifest_hash(archive: &std::path::Path, key: &str, hash: &str) {
        let path = archive.join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["hashes"][key] = serde_json::json!(hash);
        std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }

    fn rewrite_sqlite_hash(archive: &std::path::Path) {
        let bytes = std::fs::read(archive.join("app.sqlite")).unwrap();
        let digest = crate::materials::store::sha256_hex(&bytes);
        set_manifest_hash(archive, "app.sqlite", &digest);
    }

    struct Fake4dProbe;

    impl EmbeddingProbe for Fake4dProbe {
        fn embed(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            dimensions: u32,
            _: &str,
        ) -> Result<Vec<f32>, EmbeddingError> {
            let mut vector = vec![0.0; dimensions as usize];
            if dimensions > 0 {
                vector[0] = 1.0;
            }
            Ok(vector)
        }
    }

    #[test]
    fn backup_after_import_writes_spec_layout() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        materials
            .import_file(write_import(&directory, "resume.md", IMPORT_BODY))
            .unwrap();
        let archive = directory.path().join("archive");
        BackupService::new(&database, directory.path(), &config)
            .create(&archive)
            .unwrap();

        let [manifest, config_json, sqlite, materials_dir] = archive_entries(&archive);
        assert!(manifest.is_file(), "manifest.json");
        assert!(config_json.is_file(), "config.json");
        assert!(sqlite.is_file(), "app.sqlite");
        assert!(materials_dir.is_dir(), "materials/");
        assert!(!archive.join("app.sqlite3").exists());
        assert!(!archive.join("sidecar-secret.txt").exists());
        assert_eq!(material_files(&archive).len(), 1);
        assert_eq!(
            Database::open(&sqlite).unwrap().integrity_check().unwrap(),
            "ok"
        );
    }

    #[test]
    fn backup_config_and_manifest_contain_no_secret_material() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        materials
            .import_file(write_import(&directory, "resume.md", IMPORT_BODY))
            .unwrap();
        let archive = directory.path().join("archive");
        BackupService::new(&database, directory.path(), &config)
            .create(&archive)
            .unwrap();

        let config_json = std::fs::read_to_string(archive.join("config.json")).unwrap();
        let manifest_json = std::fs::read_to_string(archive.join("manifest.json")).unwrap();
        assert!(config_json.contains("providers/p1/api-key"));
        assert!(
            config_json.contains("\"configured\": true")
                || config_json.contains("\"configured\":true")
        );
        assert!(
            !contains_secret_material(&config_json),
            "config.json leaked secret material: {config_json}"
        );
        assert!(
            !contains_secret_material(&manifest_json),
            "manifest.json leaked secret material: {manifest_json}"
        );
        assert!(!config_json.contains(SECRET_DECOY));
        assert!(!manifest_json.contains(SECRET_DECOY));
    }

    #[test]
    fn restore_keeps_search_and_import_dedup() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        let source = write_import(&directory, "resume.md", IMPORT_BODY);
        let imported = materials.import_file(&source).unwrap();
        let archive = directory.path().join("archive");
        let backup = BackupService::new(&database, directory.path(), &config);
        backup.create(&archive).unwrap();

        materials.delete(&imported.id).unwrap();
        assert_eq!(live_searchable(&database, "订单服务"), 0);

        backup.restore(&archive).unwrap();
        assert_eq!(live_searchable(&database, "订单服务"), 1);
        assert_eq!(materials.import_file(&source).unwrap().id, imported.id);
        assert_eq!(
            Database::open(directory.path().join("app.sqlite3"))
                .unwrap()
                .integrity_check()
                .unwrap(),
            "ok"
        );
    }

    #[test]
    fn hash_mismatch_does_not_replace_live_data() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        materials
            .import_file(write_import(&directory, "resume.md", IMPORT_BODY))
            .unwrap();
        let archive = directory.path().join("archive");
        let backup = BackupService::new(&database, directory.path(), &config);
        backup.create(&archive).unwrap();

        let extra = materials
            .import_file(write_import(
                &directory,
                "later.txt",
                "本轮补充内容用于确认失败恢复不会回滚。",
            ))
            .unwrap();
        set_manifest_hash(
            &archive,
            "config.json",
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        let before_files = material_files(directory.path());
        let before_config = std::fs::read_to_string(directory.path().join("config.json")).unwrap();

        assert_eq!(
            backup.restore(&archive).unwrap_err().code(),
            "BACKUP_HASH_MISMATCH"
        );
        assert_eq!(live_searchable(&database, "订单服务"), 1);
        assert_eq!(live_searchable(&database, "本轮补充内容"), 1);
        assert!(
            materials
                .list()
                .unwrap()
                .iter()
                .any(|item| item.id == extra.id)
        );
        assert_eq!(material_files(directory.path()), before_files);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.json")).unwrap(),
            before_config
        );
    }

    #[test]
    fn missing_material_file_does_not_replace_live_data() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        materials
            .import_file(write_import(&directory, "resume.md", IMPORT_BODY))
            .unwrap();
        let archive = directory.path().join("archive");
        let backup = BackupService::new(&database, directory.path(), &config);
        backup.create(&archive).unwrap();

        let extra = materials
            .import_file(write_import(
                &directory,
                "later.txt",
                "本轮补充内容用于确认失败恢复不会回滚。",
            ))
            .unwrap();
        let archived = std::fs::read_dir(archive.join("materials"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let name = archived.file_name().to_string_lossy().into_owned();
        std::fs::remove_file(archived.path()).unwrap();
        let manifest_path = archive.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        manifest["hashes"]
            .as_object_mut()
            .unwrap()
            .remove(&format!("materials/{name}"));
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let before_files = material_files(directory.path());

        assert_eq!(
            backup.restore(&archive).unwrap_err().code(),
            "BACKUP_MATERIAL_MISSING"
        );
        assert_eq!(live_searchable(&database, "订单服务"), 1);
        assert_eq!(live_searchable(&database, "本轮补充内容"), 1);
        assert!(
            materials
                .list()
                .unwrap()
                .iter()
                .any(|item| item.id == extra.id)
        );
        assert_eq!(material_files(directory.path()), before_files);
    }

    #[test]
    fn corrupt_archive_sqlite_does_not_replace_live_data() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        materials
            .import_file(write_import(&directory, "resume.md", IMPORT_BODY))
            .unwrap();
        let archive = directory.path().join("archive");
        let backup = BackupService::new(&database, directory.path(), &config);
        backup.create(&archive).unwrap();

        let extra = materials
            .import_file(write_import(
                &directory,
                "later.txt",
                "本轮补充内容用于确认失败恢复不会回滚。",
            ))
            .unwrap();
        std::fs::write(archive.join("app.sqlite"), b"not-a-sqlite-database").unwrap();
        rewrite_sqlite_hash(&archive);
        let before_files = material_files(directory.path());

        assert_eq!(
            backup.restore(&archive).unwrap_err().code(),
            "BACKUP_INTEGRITY_FAILED"
        );
        assert_eq!(live_searchable(&database, "订单服务"), 1);
        assert_eq!(live_searchable(&database, "本轮补充内容"), 1);
        assert!(
            materials
                .list()
                .unwrap()
                .iter()
                .any(|item| item.id == extra.id)
        );
        assert_eq!(material_files(directory.path()), before_files);
    }

    #[test]
    fn hybrid_vec0_survives_backup_restore() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        materials
            .import_file(write_import(&directory, "resume.md", IMPORT_BODY))
            .unwrap();
        materials
            .index_chunks(
                &EmbeddingSpace {
                    provider_id: "fake".into(),
                    model_id: "embed-4".into(),
                    dimensions: 4,
                    normalized: true,
                },
                &Fake4dProbe,
            )
            .unwrap();
        let archive = directory.path().join("archive");
        let backup = BackupService::new(&database, directory.path(), &config);
        backup.create(&archive).unwrap();
        materials
            .import_file(write_import(
                &directory,
                "later.txt",
                "本轮补充内容用于确认失败恢复不会回滚。",
            ))
            .unwrap();

        backup.restore(&archive).unwrap();

        let sql: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT sql FROM sqlite_schema WHERE name = 'material_chunk_vectors'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert!(sql.contains("float[4]"), "{sql}");
        assert_eq!(live_searchable(&database, "订单服务"), 1);
        assert_eq!(live_searchable(&database, "本轮补充内容"), 0);
    }

    #[test]
    fn restore_rolls_back_when_commit_fails_after_database() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        materials
            .import_file(write_import(&directory, "resume.md", IMPORT_BODY))
            .unwrap();
        let archive = directory.path().join("archive");
        let backup = BackupService::new(&database, directory.path(), &config);
        backup.create(&archive).unwrap();

        materials
            .import_file(write_import(
                &directory,
                "later.txt",
                "本轮补充内容用于确认失败恢复不会留下混合状态。",
            ))
            .unwrap();
        config
            .update(|current| {
                current.application.locale = Some("zh-CN-live".into());
                Ok(())
            })
            .unwrap();
        let before_config = std::fs::read_to_string(directory.path().join("config.json")).unwrap();
        let before_files = material_files(directory.path());

        assert_eq!(
            backup
                .restore_failing_after_database(&archive)
                .unwrap_err()
                .code(),
            "BACKUP_OPERATION_FAILED"
        );
        assert_eq!(live_searchable(&database, "订单服务"), 1);
        assert_eq!(live_searchable(&database, "本轮补充内容"), 1);
        assert_eq!(material_files(directory.path()), before_files);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("config.json")).unwrap(),
            before_config
        );
        assert!(!directory.path().join(".restore-journal.json").exists());
        assert!(!directory.path().join(".restore-rollback").exists());
    }

    #[test]
    fn restore_restores_same_content_sha256() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        let source = write_import(&directory, "resume.md", IMPORT_BODY);
        let imported = materials.import_file(&source).unwrap();
        let sha_before = MaterialStore::new(&database)
            .list()
            .unwrap()
            .into_iter()
            .find(|item| item.id == imported.id)
            .unwrap()
            .content_sha256;
        let archive = directory.path().join("archive");
        let backup = BackupService::new(&database, directory.path(), &config);
        backup.create(&archive).unwrap();

        // 备份后现场继续变化：恢复必须回到备份时的内容哈希。
        materials
            .import_file(write_import(
                &directory,
                "later.txt",
                "备份之后才导入的资料不应在恢复后存活。",
            ))
            .unwrap();

        backup.restore(&archive).unwrap();
        let restored = MaterialStore::new(&database).list().unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, imported.id);
        assert_eq!(restored[0].content_sha256, sha_before);
    }

    #[test]
    fn restore_over_existing_same_hash_keeps_single_file() {
        let directory = tempfile::tempdir().unwrap();
        let database = opened(&directory);
        let config = seeded_config(&directory);
        let materials = MaterialService::new(&database, directory.path());
        let source = write_import(&directory, "resume.md", IMPORT_BODY);
        let imported = materials.import_file(&source).unwrap();
        let archive = directory.path().join("archive");
        let backup = BackupService::new(&database, directory.path(), &config);
        backup.create(&archive).unwrap();

        // 现场未变（同哈希文件仍在）时恢复：不得产生第二份文件或第二条记录。
        backup.restore(&archive).unwrap();
        assert_eq!(MaterialStore::new(&database).list().unwrap().len(), 1);
        assert_eq!(material_files(directory.path()).len(), 1);
        assert_eq!(
            materials.import_file(&source).unwrap().id,
            imported.id,
            "同内容再导入必须命中去重而不是落第二份"
        );
        assert_eq!(MaterialStore::new(&database).list().unwrap().len(), 1);
        assert_eq!(material_files(directory.path()).len(), 1);
    }
}
