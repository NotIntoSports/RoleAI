//! 旧版数据迁移：探测旧安装根目录、校验归档、备份并导入会话与密钥槽。
//! 首次启动自动执行（try_first_run_legacy_migration），其余入口由设置页命令调用。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, backup::Backup};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{
    config::{AppConfigV1, ConfigDirs, ConfigStore},
    database::Database,
    materials::backup::{
        BackupError, file_sha256, reject_secret_bytes, require_integrity, verify_file_hashes,
        write_scrubbed_config,
    },
};

mod sessions;

#[cfg(test)]
mod tests;

pub use sessions::{LegacySessionImport, import_legacy_sessions_from_sqlite};

const ARCHIVE_MANIFEST: &str = "manifest.json";
const ARCHIVE_CONFIG: &str = "config.json";
const ARCHIVE_SQLITE: &str = "app.sqlite";
const ARCHIVE_MATERIALS: &str = "materials";
const DEST_SQLITE: &str = "app.sqlite3";
const MIGRATED_FROM: &str = "migrated-from";
const STAGING_DIR: &str = ".migrating";
const STAGING_PREVIOUS: &str = ".migrating-previous";
const ELECTRON_APP_NAMES: &[&str] = &["AI Virtual Assistant", "authorized-interview-screen-helper"];
const REMOTE_ACCOUNT_TABLES: &[&str] = &[
    "accounts",
    "users",
    "login_sessions",
    "auth_tokens",
    "remote_accounts",
];
const BACKUP_PAGES: i32 = 100;
const BACKUP_PAUSE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyKind {
    Repository,
    ElectronUserData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacySearchRoot {
    Repository(PathBuf),
    AppData(PathBuf),
    UserSelected(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRoot {
    pub kind: LegacyKind,
    pub path: PathBuf,
    pub config_path: Option<PathBuf>,
    pub sqlite_path: Option<PathBuf>,
    pub materials_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyBackupReport {
    pub archive_path: PathBuf,
    pub files: Vec<String>,
    pub omitted: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateError {
    Operation,
    HashMismatch,
    Integrity,
    SecretForbidden,
    AlreadyApplied,
    PayloadInvalid,
}

impl MigrateError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Operation => "MIGRATE_OPERATION_FAILED",
            Self::HashMismatch => "MIGRATE_HASH_MISMATCH",
            Self::Integrity => "MIGRATE_INTEGRITY_FAILED",
            Self::SecretForbidden => "MIGRATE_SECRET_FORBIDDEN",
            Self::AlreadyApplied => "MIGRATE_ALREADY_APPLIED",
            Self::PayloadInvalid => "MIGRATE_PAYLOAD_INVALID",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedLegacy {
    pub archive_path: PathBuf,
    pub files: Vec<String>,
    pub omitted: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchReport {
    pub dest_data_dir: PathBuf,
    pub files: Vec<String>,
    pub omitted: Vec<String>,
    pub marker_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LegacyMigrationStatus {
    pub applied: bool,
    pub reenter_secrets: bool,
    pub omitted: Vec<String>,
}

pub fn legacy_search_roots(dirs: &ConfigDirs, development: bool) -> Vec<LegacySearchRoot> {
    if development {
        vec![LegacySearchRoot::Repository(dirs.repository.clone())]
    } else {
        vec![LegacySearchRoot::AppData(dirs.roaming_app_data.clone())]
    }
}

pub fn try_first_run_legacy_migration(
    dest_data_dir: &Path,
    search_roots: &[LegacySearchRoot],
    app_data: &Path,
    now: DateTime<Utc>,
) -> Result<Option<SwitchReport>, MigrateError> {
    if dest_already_applied(dest_data_dir) {
        return Ok(None);
    }
    let found = detect_legacy_roots(search_roots);
    let Some(root) = found.into_iter().next() else {
        return Ok(None);
    };
    let backup = backup_legacy_into_app_data(&root, app_data, now)?;
    let _verified = verify_legacy_archive(&backup.archive_path)?;
    let report = switch_legacy_archive(&backup.archive_path, dest_data_dir)?;
    if let Err(error) = sessions::complete_legacy_session_import(dest_data_dir) {
        rollback_failed_semantic_import(dest_data_dir);
        return Err(error);
    }
    Ok(Some(report))
}

pub fn import_legacy_sessions_from_user_path(
    source: &Path,
    dest: &Database,
) -> Result<LegacySessionImport, MigrateError> {
    let root = detect_user_selected(source).ok_or(MigrateError::Operation)?;
    let sqlite = root.sqlite_path.ok_or(MigrateError::Operation)?;
    sessions::import_legacy_sessions_from_sqlite(&sqlite, dest)
}

pub fn secret_slots_configured(config: &AppConfigV1) -> bool {
    config.models.providers.iter().any(|provider| {
        provider
            .credential
            .as_ref()
            .is_some_and(|slot| slot.configured)
    })
}

pub fn legacy_migration_status(
    dest_data_dir: &Path,
    secrets_configured: bool,
) -> LegacyMigrationStatus {
    match read_migrated_from(dest_data_dir) {
        Some(marker) => LegacyMigrationStatus {
            applied: true,
            reenter_secrets: !secrets_configured,
            omitted: marker.omitted,
        },
        None => LegacyMigrationStatus {
            applied: false,
            reenter_secrets: false,
            omitted: Vec::new(),
        },
    }
}

fn read_migrated_from(dest_data_dir: &Path) -> Option<MigratedFromMarker> {
    let bytes = fs::read(dest_data_dir.join(MIGRATED_FROM)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn verify_legacy_archive(archive: &Path) -> Result<VerifiedLegacy, MigrateError> {
    let staged = stage_archive_copy(archive)?;
    let staged_root = staged.as_path();
    let manifest = read_legacy_manifest(&staged_root.join(ARCHIVE_MANIFEST))?;
    verify_file_hashes(staged_root, &manifest.hashes).map_err(map_backup)?;
    verify_present_files(staged_root, &manifest)?;

    if staged_root.join(ARCHIVE_CONFIG).is_file() {
        let bytes =
            fs::read(staged_root.join(ARCHIVE_CONFIG)).map_err(|_| MigrateError::Operation)?;
        reject_secret_bytes(&bytes).map_err(map_backup)?;
    }

    if staged_root.join(ARCHIVE_SQLITE).is_file() {
        let database = Database::open(staged_root.join(ARCHIVE_SQLITE))
            .map_err(|_| MigrateError::Integrity)?;
        require_integrity(&database).map_err(map_backup)?;
    }

    let mut files: Vec<String> = manifest.hashes.keys().cloned().collect();
    files.sort();
    let mut omitted = manifest.omitted;
    omitted.sort();
    omitted.dedup();

    Ok(VerifiedLegacy {
        archive_path: archive.to_path_buf(),
        files,
        omitted,
    })
}

pub fn switch_legacy_archive(
    archive: &Path,
    dest_data_dir: &Path,
) -> Result<SwitchReport, MigrateError> {
    let verified = verify_legacy_archive(archive)?;
    if dest_already_applied(dest_data_dir) {
        return Err(MigrateError::AlreadyApplied);
    }
    if dest_data_dir.is_file() {
        return Err(MigrateError::Operation);
    }

    let dest_was_missing = !dest_data_dir.exists();
    let staging = dest_data_dir.join(STAGING_DIR);
    let previous = dest_data_dir.join(STAGING_PREVIOUS);
    let mut placed = Vec::new();

    let result = (|| {
        fs::create_dir_all(dest_data_dir).map_err(|_| MigrateError::Operation)?;
        remove_path_if_exists(&staging)?;
        remove_path_if_exists(&previous)?;
        fs::create_dir_all(&staging).map_err(|_| MigrateError::Operation)?;

        if archive.join(ARCHIVE_CONFIG).is_file() {
            fs::copy(archive.join(ARCHIVE_CONFIG), staging.join(ARCHIVE_CONFIG))
                .map_err(|_| MigrateError::Operation)?;
        }
        if archive.join(ARCHIVE_SQLITE).is_file() {
            fs::copy(archive.join(ARCHIVE_SQLITE), staging.join(DEST_SQLITE))
                .map_err(|_| MigrateError::Operation)?;
        }
        if archive.join(ARCHIVE_MATERIALS).is_dir() {
            copy_tree(
                &archive.join(ARCHIVE_MATERIALS),
                &staging.join(ARCHIVE_MATERIALS),
            )?;
        }
        make_tree_writable(&staging)?;
        write_migrated_from_marker(&staging.join(MIGRATED_FROM), archive, &verified.omitted)?;

        for name in dest_artifact_names(&verified.files) {
            place_item(&staging.join(&name), &dest_data_dir.join(&name), &previous)?;
            placed.push(name);
        }
        place_item(
            &staging.join(MIGRATED_FROM),
            &dest_data_dir.join(MIGRATED_FROM),
            &previous,
        )?;
        placed.push(MIGRATED_FROM.to_owned());
        Ok(())
    })();

    if let Err(error) = result {
        for name in placed.iter().rev() {
            restore_item(dest_data_dir, name, &previous);
        }
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&previous);
        if dest_was_missing {
            let _ = fs::remove_dir_all(dest_data_dir);
        }
        return Err(error);
    }

    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_dir_all(&previous);

    Ok(SwitchReport {
        dest_data_dir: dest_data_dir.to_path_buf(),
        files: dest_file_list(&verified.files),
        omitted: verified.omitted,
        marker_path: dest_data_dir.join(MIGRATED_FROM),
    })
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyBackupManifest {
    schema_version: i64,
    kind: String,
    hashes: BTreeMap<String, String>,
    omitted: Vec<String>,
}

pub fn detect_legacy_roots(roots: &[LegacySearchRoot]) -> Vec<LegacyRoot> {
    let mut found = Vec::new();
    for root in roots {
        match root {
            LegacySearchRoot::Repository(repository) => {
                if let Some(detected) = detect_repository(repository) {
                    push_unique(&mut found, detected);
                }
            }
            LegacySearchRoot::AppData(app_data) => {
                for name in ELECTRON_APP_NAMES {
                    if let Some(detected) = detect_electron_user_data(&app_data.join(name)) {
                        push_unique(&mut found, detected);
                    }
                }
            }
            LegacySearchRoot::UserSelected(path) => {
                if let Some(detected) = detect_user_selected(path) {
                    push_unique(&mut found, detected);
                }
            }
        }
    }
    found
}

pub fn backup_legacy(root: &LegacyRoot, dest: &Path) -> Result<LegacyBackupReport, MigrateError> {
    fs::create_dir_all(dest).map_err(|_| MigrateError::Operation)?;
    let mut files = Vec::new();
    let mut omitted = scan_forbidden_sidecars(&root.path);
    let mut hashes = BTreeMap::new();

    if let Some(config_path) = &root.config_path {
        match copy_config(config_path, &dest.join(ARCHIVE_CONFIG))? {
            CopyOutcome::Copied => {
                files.push(ARCHIVE_CONFIG.to_owned());
                hashes.insert(
                    ARCHIVE_CONFIG.to_owned(),
                    hash_file(&dest.join(ARCHIVE_CONFIG))?,
                );
            }
            CopyOutcome::Omitted => omitted.push(ARCHIVE_CONFIG.to_owned()),
            CopyOutcome::Skipped => {}
        }
    }

    match root.sqlite_path.as_deref() {
        Some(sqlite_path) if sqlite_path.is_file() => {
            match copy_sqlite(sqlite_path, &dest.join(ARCHIVE_SQLITE))? {
                CopyOutcome::Copied => {
                    files.push(ARCHIVE_SQLITE.to_owned());
                    hashes.insert(
                        ARCHIVE_SQLITE.to_owned(),
                        hash_file(&dest.join(ARCHIVE_SQLITE))?,
                    );
                }
                CopyOutcome::Omitted => omitted.push(ARCHIVE_SQLITE.to_owned()),
                CopyOutcome::Skipped => {}
            }
        }
        _ => {}
    }

    if let Some(materials_path) = &root.materials_path
        && materials_path.is_dir()
    {
        copy_materials(
            materials_path,
            &dest.join(ARCHIVE_MATERIALS),
            &mut files,
            &mut omitted,
            &mut hashes,
        )?;
    }

    files.sort();
    files.dedup();
    omitted.sort();
    omitted.dedup();

    let manifest = LegacyBackupManifest {
        schema_version: 1,
        kind: "legacy".into(),
        hashes,
        omitted: omitted.clone(),
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|_| MigrateError::Operation)?;
    reject_secret_bytes(&manifest_bytes).map_err(|_| MigrateError::Operation)?;
    fs::write(dest.join(ARCHIVE_MANIFEST), manifest_bytes).map_err(|_| MigrateError::Operation)?;
    make_tree_readonly(dest)?;

    Ok(LegacyBackupReport {
        archive_path: dest.to_path_buf(),
        files,
        omitted,
    })
}

pub fn backup_legacy_into_app_data(
    root: &LegacyRoot,
    app_data: &Path,
    now: DateTime<Utc>,
) -> Result<LegacyBackupReport, MigrateError> {
    let dest = app_data
        .join("backups")
        .join(format!("legacy-{}", now.format("%Y%m%dT%H%M%SZ")));
    backup_legacy(root, &dest)
}

fn map_backup(error: BackupError) -> MigrateError {
    match error {
        BackupError::HashMismatch => MigrateError::HashMismatch,
        BackupError::Integrity => MigrateError::Integrity,
        BackupError::SecretForbidden => MigrateError::SecretForbidden,
        BackupError::MaterialMissing | BackupError::Operation => MigrateError::Operation,
    }
}

fn read_legacy_manifest(path: &Path) -> Result<LegacyBackupManifest, MigrateError> {
    let bytes = fs::read(path).map_err(|_| MigrateError::Operation)?;
    reject_secret_bytes(&bytes).map_err(map_backup)?;
    serde_json::from_slice(&bytes).map_err(|_| MigrateError::Operation)
}

fn verify_present_files(
    staged: &Path,
    manifest: &LegacyBackupManifest,
) -> Result<(), MigrateError> {
    if staged.join(ARCHIVE_CONFIG).is_file() && !manifest.hashes.contains_key(ARCHIVE_CONFIG) {
        return Err(MigrateError::HashMismatch);
    }
    if staged.join(ARCHIVE_SQLITE).is_file() && !manifest.hashes.contains_key(ARCHIVE_SQLITE) {
        return Err(MigrateError::HashMismatch);
    }
    if staged.join(ARCHIVE_MATERIALS).is_dir() {
        for key in collect_material_keys(&staged.join(ARCHIVE_MATERIALS))? {
            if !manifest.hashes.contains_key(&key) {
                return Err(MigrateError::HashMismatch);
            }
        }
    }
    Ok(())
}

fn collect_material_keys(root: &Path) -> Result<Vec<String>, MigrateError> {
    let mut keys = Vec::new();
    collect_material_keys_into(root, root, &mut keys)?;
    Ok(keys)
}

fn collect_material_keys_into(
    root: &Path,
    current: &Path,
    keys: &mut Vec<String>,
) -> Result<(), MigrateError> {
    for entry in fs::read_dir(current).map_err(|_| MigrateError::Operation)? {
        let entry = entry.map_err(|_| MigrateError::Operation)?;
        let file_type = entry.file_type().map_err(|_| MigrateError::Operation)?;
        if file_type.is_dir() {
            collect_material_keys_into(root, &entry.path(), keys)?;
        } else if file_type.is_file() {
            keys.push(format!(
                "{ARCHIVE_MATERIALS}/{}",
                relative_key(root, &entry.path())?
            ));
        }
    }
    Ok(())
}

fn rollback_failed_semantic_import(dest_data_dir: &Path) {
    let _ = fs::remove_file(dest_data_dir.join(MIGRATED_FROM));
    let _ = fs::remove_file(dest_data_dir.join(DEST_SQLITE));
    let _ = fs::remove_file(dest_data_dir.join(format!("{DEST_SQLITE}-wal")));
    let _ = fs::remove_file(dest_data_dir.join(format!("{DEST_SQLITE}-shm")));
}

fn dest_already_applied(dest: &Path) -> bool {
    if dest.join(MIGRATED_FROM).is_file() {
        return true;
    }
    let sqlite = dest.join(DEST_SQLITE);
    sqlite.is_file() && sqlite_has_user_schema(&sqlite)
}

fn sqlite_has_user_schema(path: &Path) -> bool {
    let Ok(connection) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return true;
    };
    let Ok(count) = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get::<_, i64>(0),
    ) else {
        return true;
    };
    count > 0
}

fn dest_file_list(archive_files: &[String]) -> Vec<String> {
    let mut files = archive_files
        .iter()
        .map(|name| dest_name(name))
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files
}

fn dest_artifact_names(archive_files: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    if archive_files.iter().any(|name| name == ARCHIVE_CONFIG) {
        names.push(ARCHIVE_CONFIG.to_owned());
    }
    if archive_files.iter().any(|name| name == ARCHIVE_SQLITE) {
        names.push(DEST_SQLITE.to_owned());
    }
    if archive_files
        .iter()
        .any(|name| name == ARCHIVE_MATERIALS || name.starts_with("materials/"))
    {
        names.push(ARCHIVE_MATERIALS.to_owned());
    }
    names
}

fn dest_name(archive_key: &str) -> String {
    if archive_key == ARCHIVE_SQLITE {
        DEST_SQLITE.to_owned()
    } else {
        archive_key.to_owned()
    }
}

fn write_migrated_from_marker(
    path: &Path,
    archive: &Path,
    omitted: &[String],
) -> Result<(), MigrateError> {
    let marker = MigratedFromMarker {
        archive_path: archive.to_string_lossy().into_owned(),
        utc: Utc::now().to_rfc3339(),
        omitted: omitted.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&marker).map_err(|_| MigrateError::Operation)?;
    reject_secret_bytes(&bytes).map_err(map_backup)?;
    fs::write(path, bytes).map_err(|_| MigrateError::Operation)
}

fn place_item(from: &Path, to: &Path, previous_root: &Path) -> Result<(), MigrateError> {
    if !from.exists() {
        return Err(MigrateError::Operation);
    }
    if to.exists() {
        fs::create_dir_all(previous_root).map_err(|_| MigrateError::Operation)?;
        let backup = previous_root.join(to.file_name().ok_or(MigrateError::Operation)?);
        remove_path_if_exists(&backup)?;
        fs::rename(to, &backup).map_err(|_| MigrateError::Operation)?;
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|_| MigrateError::Operation)?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if from.is_dir() {
        copy_tree(from, to)?;
        let _ = fs::remove_dir_all(from);
    } else {
        fs::copy(from, to).map_err(|_| MigrateError::Operation)?;
        let _ = fs::remove_file(from);
    }
    Ok(())
}

fn restore_item(dest: &Path, name: &str, previous_root: &Path) {
    let live = dest.join(name);
    let backup = previous_root.join(name);
    let _ = remove_path_if_exists(&live);
    if backup.exists() {
        let _ = fs::rename(&backup, &live);
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), MigrateError> {
    fs::create_dir_all(to).map_err(|_| MigrateError::Operation)?;
    for entry in fs::read_dir(from).map_err(|_| MigrateError::Operation)? {
        let entry = entry.map_err(|_| MigrateError::Operation)?;
        let destination = to.join(entry.file_name());
        let file_type = entry.file_type().map_err(|_| MigrateError::Operation)?;
        if file_type.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination).map_err(|_| MigrateError::Operation)?;
        }
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<(), MigrateError> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|_| MigrateError::Operation)?;
    } else if path.is_file() {
        fs::remove_file(path).map_err(|_| MigrateError::Operation)?;
    }
    Ok(())
}

fn stage_archive_copy(archive: &Path) -> Result<StagingDir, MigrateError> {
    let staged = std::env::temp_dir().join(format!("legacy-verify-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staged).map_err(|_| MigrateError::Operation)?;
    let staged = StagingDir(staged);
    let manifest = archive.join(ARCHIVE_MANIFEST);
    if !manifest.is_file() {
        return Err(MigrateError::Operation);
    }
    fs::copy(&manifest, staged.as_path().join(ARCHIVE_MANIFEST))
        .map_err(|_| MigrateError::Operation)?;
    if archive.join(ARCHIVE_CONFIG).is_file() {
        fs::copy(
            archive.join(ARCHIVE_CONFIG),
            staged.as_path().join(ARCHIVE_CONFIG),
        )
        .map_err(|_| MigrateError::Operation)?;
    }
    if archive.join(ARCHIVE_SQLITE).is_file() {
        fs::copy(
            archive.join(ARCHIVE_SQLITE),
            staged.as_path().join(ARCHIVE_SQLITE),
        )
        .map_err(|_| MigrateError::Operation)?;
    }
    if archive.join(ARCHIVE_MATERIALS).is_dir() {
        copy_tree(
            &archive.join(ARCHIVE_MATERIALS),
            &staged.as_path().join(ARCHIVE_MATERIALS),
        )?;
    }
    make_tree_writable(staged.as_path())?;
    Ok(staged)
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigratedFromMarker {
    archive_path: String,
    utc: String,
    omitted: Vec<String>,
}

fn detect_repository(repository: &Path) -> Option<LegacyRoot> {
    let config_path = first_existing_file([repository.join("config").join("local.json")]);
    let sqlite_path = first_existing_file(legacy_sqlite_candidates(repository));
    let materials_path = existing_dir(repository.join("data").join(ARCHIVE_MATERIALS));
    let has_avatar = repository.join("data").join("avatar").is_dir()
        || repository.join("data").join("avatars").is_dir();
    if config_path.is_none() && sqlite_path.is_none() && materials_path.is_none() && !has_avatar {
        return None;
    }
    Some(LegacyRoot {
        kind: LegacyKind::Repository,
        path: repository.to_path_buf(),
        config_path,
        sqlite_path,
        materials_path,
    })
}

fn detect_electron_user_data(user_data: &Path) -> Option<LegacyRoot> {
    let config_path = first_existing_file([
        user_data.join("config.json"),
        user_data.join("local.json"),
        user_data.join("config").join("local.json"),
    ]);
    let sqlite_path = first_existing_file(
        [
            user_data.join("app.sqlite"),
            user_data.join("app.sqlite3"),
            user_data.join("data").join("app.sqlite"),
            user_data.join("data").join("app.sqlite3"),
        ]
        .into_iter()
        .chain(legacy_sqlite_candidates(user_data)),
    );
    let materials_path = existing_dir(user_data.join(ARCHIVE_MATERIALS))
        .or_else(|| existing_dir(user_data.join("data").join(ARCHIVE_MATERIALS)));
    if config_path.is_none() && sqlite_path.is_none() {
        return None;
    }
    Some(LegacyRoot {
        kind: LegacyKind::ElectronUserData,
        path: user_data.to_path_buf(),
        config_path,
        sqlite_path,
        materials_path,
    })
}

fn push_unique(found: &mut Vec<LegacyRoot>, candidate: LegacyRoot) {
    if found.iter().any(|existing| existing.path == candidate.path) {
        return;
    }
    found.push(candidate);
}

fn first_existing_file(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|path| path.is_file())
}

fn legacy_sqlite_candidates(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("data").join("app.sqlite"),
        root.join("data").join("app.sqlite3"),
        root.join(".desktop-runtime")
            .join("data")
            .join("app.sqlite"),
        root.join(".desktop-runtime")
            .join("data")
            .join("app.sqlite3"),
        root.join("resources")
            .join(".desktop-runtime")
            .join("data")
            .join("app.sqlite"),
        root.join("resources")
            .join(".desktop-runtime")
            .join("data")
            .join("app.sqlite3"),
    ]
}

fn detect_user_selected(path: &Path) -> Option<LegacyRoot> {
    if path.is_file() {
        return Some(LegacyRoot {
            kind: LegacyKind::Repository,
            path: path.parent().unwrap_or(path).to_path_buf(),
            config_path: None,
            sqlite_path: Some(path.to_path_buf()),
            materials_path: None,
        });
    }
    detect_desktop_runtime(path)
        .or_else(|| detect_repository(path))
        .or_else(|| detect_electron_user_data(path))
}

fn detect_desktop_runtime(root: &Path) -> Option<LegacyRoot> {
    let runtime = if root
        .file_name()
        .is_some_and(|name| name == ".desktop-runtime")
    {
        root.to_path_buf()
    } else if root.join(".desktop-runtime").is_dir() {
        root.join(".desktop-runtime")
    } else if root.join("resources").join(".desktop-runtime").is_dir() {
        root.join("resources").join(".desktop-runtime")
    } else {
        return None;
    };
    let sqlite_path = first_existing_file([
        runtime.join("data").join("app.sqlite"),
        runtime.join("data").join("app.sqlite3"),
        runtime.join("app.sqlite"),
        runtime.join("app.sqlite3"),
    ]);
    sqlite_path.as_ref()?;
    Some(LegacyRoot {
        kind: LegacyKind::Repository,
        path: runtime,
        config_path: None,
        sqlite_path,
        materials_path: None,
    })
}

fn existing_dir(path: PathBuf) -> Option<PathBuf> {
    path.is_dir().then_some(path)
}

fn scan_forbidden_sidecars(root: &Path) -> Vec<String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut omitted = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_forbidden_sidecar(&name) {
            omitted.push(name);
        }
    }
    omitted
}

pub(crate) fn legacy_login_cookie_name() -> String {
    ["control", "api", "token"].join("_")
}

fn is_forbidden_sidecar(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower == "cookies"
        || lower == "cookies-journal"
        || lower == "desktop_session"
        || lower == legacy_login_cookie_name()
}

enum CopyOutcome {
    Copied,
    Omitted,
    Skipped,
}

fn copy_config(source: &Path, dest: &Path) -> Result<CopyOutcome, MigrateError> {
    if !source.is_file() {
        return Ok(CopyOutcome::Skipped);
    }
    let store = ConfigStore::new(source.to_path_buf());
    if store.load().is_ok() {
        return match write_scrubbed_config(&store, dest) {
            Ok(()) => Ok(CopyOutcome::Copied),
            Err(BackupError::SecretForbidden) => Ok(CopyOutcome::Omitted),
            Err(_) => Err(MigrateError::Operation),
        };
    }
    let bytes = fs::read(source).map_err(|_| MigrateError::Operation)?;
    if looks_like_secrets(&bytes) {
        return Ok(CopyOutcome::Omitted);
    }
    reject_secret_bytes(&bytes).map_err(|_| MigrateError::Operation)?;
    fs::write(dest, bytes).map_err(|_| MigrateError::Operation)?;
    Ok(CopyOutcome::Copied)
}

fn looks_like_secrets(bytes: &[u8]) -> bool {
    if reject_secret_bytes(bytes).is_err() {
        return true;
    }
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .is_some_and(json_has_secret_fields)
}

fn json_has_secret_fields(value: Value) -> bool {
    match value {
        Value::Object(map) => map
            .into_iter()
            .any(|(key, child)| secret_named_field(&key, &child) || json_has_secret_fields(child)),
        Value::Array(items) => items.into_iter().any(json_has_secret_fields),
        _ => false,
    }
}

fn secret_named_field(key: &str, value: &Value) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
    let canonical_secret_slot =
        matches!(normalized.as_str(), "apikey" | "apisecret") && is_secret_slot_value(value);
    if canonical_secret_slot {
        return false;
    }
    normalized.contains("password")
        || normalized == "cookies"
        || normalized == "controlapitoken"
        || normalized == "desktopsession"
        || normalized == "token"
        || normalized.ends_with("token")
        || normalized.contains("apikey")
        || (normalized.contains("secret") && normalized != "secretref")
}

fn is_secret_slot_value(value: &Value) -> bool {
    let Some(slot) = value.as_object() else {
        return false;
    };
    slot.len() == 2
        && slot.get("reference").is_some_and(Value::is_string)
        && slot.get("configured").is_some_and(Value::is_boolean)
}

fn copy_sqlite(source: &Path, dest: &Path) -> Result<CopyOutcome, MigrateError> {
    let source_conn = match Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(connection) => connection,
        Err(_) => return Ok(CopyOutcome::Omitted),
    };
    let check: String = match source_conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))
    {
        Ok(check) => check,
        Err(_) => return Ok(CopyOutcome::Omitted),
    };
    if check != "ok" || has_remote_account_tables(&source_conn) {
        return Ok(CopyOutcome::Omitted);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|_| MigrateError::Operation)?;
    }
    let mut dest_conn = Connection::open(dest).map_err(|_| MigrateError::Operation)?;
    {
        let backup =
            Backup::new(&source_conn, &mut dest_conn).map_err(|_| MigrateError::Operation)?;
        backup
            .run_to_completion(BACKUP_PAGES, BACKUP_PAUSE, None)
            .map_err(|_| MigrateError::Operation)?;
    }
    Ok(CopyOutcome::Copied)
}

fn has_remote_account_tables(connection: &Connection) -> bool {
    let Ok(mut statement) =
        connection.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
    else {
        return false;
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) else {
        return false;
    };
    rows.flatten().any(|name| {
        REMOTE_ACCOUNT_TABLES
            .iter()
            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
    })
}

fn copy_materials(
    source: &Path,
    dest: &Path,
    files: &mut Vec<String>,
    omitted: &mut Vec<String>,
    hashes: &mut BTreeMap<String, String>,
) -> Result<(), MigrateError> {
    fs::create_dir_all(dest).map_err(|_| MigrateError::Operation)?;
    copy_materials_dir(source, dest, source, files, omitted, hashes)
}

fn copy_materials_dir(
    root: &Path,
    dest_root: &Path,
    current: &Path,
    files: &mut Vec<String>,
    omitted: &mut Vec<String>,
    hashes: &mut BTreeMap<String, String>,
) -> Result<(), MigrateError> {
    for entry in fs::read_dir(current).map_err(|_| MigrateError::Operation)? {
        let entry = entry.map_err(|_| MigrateError::Operation)?;
        let file_type = entry.file_type().map_err(|_| MigrateError::Operation)?;
        let relative = relative_key(root, &entry.path())?;
        let archive_key = format!("{ARCHIVE_MATERIALS}/{relative}");
        let destination = dest_root.join(&relative);
        if file_type.is_dir() {
            fs::create_dir_all(&destination).map_err(|_| MigrateError::Operation)?;
            copy_materials_dir(root, dest_root, &entry.path(), files, omitted, hashes)?;
        } else if file_type.is_file() {
            let bytes = fs::read(entry.path()).map_err(|_| MigrateError::Operation)?;
            if reject_secret_bytes(&bytes).is_err() {
                omitted.push(archive_key);
                continue;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|_| MigrateError::Operation)?;
            }
            fs::write(&destination, bytes).map_err(|_| MigrateError::Operation)?;
            hashes.insert(archive_key.clone(), hash_file(&destination)?);
            files.push(archive_key);
        }
    }
    Ok(())
}

fn relative_key(root: &Path, file: &Path) -> Result<String, MigrateError> {
    let relative = file
        .strip_prefix(root)
        .map_err(|_| MigrateError::Operation)?;
    Ok(relative
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn hash_file(path: &Path) -> Result<String, MigrateError> {
    file_sha256(path).map_err(|_| MigrateError::Operation)
}

fn make_tree_readonly(root: &Path) -> Result<(), MigrateError> {
    if root.is_file() {
        return make_readonly(root);
    }
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|_| MigrateError::Operation)? {
        let entry = entry.map_err(|_| MigrateError::Operation)?;
        make_tree_readonly(&entry.path())?;
    }
    Ok(())
}

fn make_tree_writable(root: &Path) -> Result<(), MigrateError> {
    if root.is_file() {
        return make_writable(root);
    }
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|_| MigrateError::Operation)? {
        let entry = entry.map_err(|_| MigrateError::Operation)?;
        make_tree_writable(&entry.path())?;
    }
    make_writable(root)
}

#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &Path) -> Result<(), MigrateError> {
    let mut permissions = fs::metadata(path)
        .map_err(|_| MigrateError::Operation)?
        .permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).map_err(|_| MigrateError::Operation)?;
    }
    Ok(())
}

fn make_readonly(path: &Path) -> Result<(), MigrateError> {
    let mut permissions = fs::metadata(path)
        .map_err(|_| MigrateError::Operation)?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|_| MigrateError::Operation)
}
