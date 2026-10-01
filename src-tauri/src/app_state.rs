use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, atomic::AtomicU64},
};

use chrono::Utc;

use crate::{
    config::{ConfigLoadOutcome, ConfigStore},
    contracts::StartupState,
    database::{Database, DatabaseError},
    diagnostics::{DiagnosticError, DiagnosticEvent, DiagnosticWriter},
    error::PublicError,
    migrate::{self, LegacySearchRoot},
    secrets::{SecretError, SecretService, SecretStore},
    services::{SessionControl, SessionService},
    sessions::SessionStore,
};
// Windows 凭据管理器后端仅在 Windows 编译；其它平台回退内存实现（见 default_secret_store）。
#[cfg(not(windows))]
use crate::secrets::MemorySecretStore;
#[cfg(windows)]
use crate::secrets::WindowsSecretStore;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_directory: PathBuf,
    pub logs_directory: PathBuf,
    pub config_path: PathBuf,
    pub legacy_search_roots: Vec<LegacySearchRoot>,
}

/// Tauri 全局状态：密钥服务、数据库、配置与运行时门面的聚合根。
pub struct AppState {
    pub secrets: SecretService,
    pub database: Mutex<Option<Arc<Database>>>,
    pub diagnostics: DiagnosticWriter,
    pub config: ConfigStore,
    pub service_lock: Mutex<()>,
    pub sessions: Mutex<SessionService>,
    pub session_control: Arc<SessionControl>,
    pub event_seq: AtomicU64,
    pub audio_routing: Mutex<Option<crate::prerequisites::AudioRoutingChange>>,
    pub livestream: Mutex<Option<crate::livestream::LivestreamScript>>,
    pub livestream_stage: Mutex<Option<crate::livestream::LivestreamStageState>>,
    pub livestream_playback_cancel: Mutex<Arc<std::sync::atomic::AtomicBool>>,
    pub livestream_voice: Mutex<Option<crate::livestream::LivestreamVoiceSnapshot>>,
    pub obs_previous_scene: Mutex<Option<String>>,
    pub operator_monitor: Mutex<Option<std::process::Child>>,
    /// 本机麦克风无锁热路径句柄（会话期间由 commands 层持有）。
    pub mic_ingest: Mutex<Option<crate::audio::capture::MicIngestHandle>>,
    database_path: PathBuf,
    secret_backend_ready: bool,
    startup: RwLock<StartupState>,
    pub paths: AppPaths,
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Ok(slot) = self.operator_monitor.get_mut()
            && let Some(mut child) = slot.take()
        {
            crate::audio::monitor::stop(&mut child);
        }
        if let Ok(slot) = self.audio_routing.get_mut()
            && let Some(change) = slot.take()
            && crate::prerequisites::restore_default_capture_mic(&change).is_ok()
        {
            crate::prerequisites::clear_persisted_audio_routing(&self.paths.data_directory);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error(transparent)]
    Diagnostics(#[from] DiagnosticError),
    #[error(transparent)]
    Secrets(#[from] SecretError),
}

/// 生产默认密钥后端：Windows 用凭据管理器持久化；其它平台回退到内存实现
/// （应用退出即丢失），并在启动日志明确提示，避免用户误以为密钥已持久化。
#[cfg(windows)]
fn default_secret_store() -> Arc<dyn SecretStore> {
    Arc::new(WindowsSecretStore::new())
}

#[cfg(not(windows))]
fn default_secret_store() -> Arc<dyn SecretStore> {
    tracing::warn!("当前平台不持久化密钥：API Key 仅保存在内存中，应用退出后需重新配置");
    Arc::new(MemorySecretStore::default())
}

impl AppState {
    pub fn production(paths: AppPaths) -> Result<Self, AppStateError> {
        Self::initialize(paths, default_secret_store())
    }

    pub fn production_namespaced(
        paths: AppPaths,
        secret_namespace: String,
    ) -> Result<Self, AppStateError> {
        Self::initialize_namespaced(paths, secret_namespace, default_secret_store())
    }

    pub fn initialize(
        paths: AppPaths,
        secret_store: Arc<dyn SecretStore>,
    ) -> Result<Self, AppStateError> {
        Self::initialize_namespaced(paths, "default", secret_store)
    }

    pub fn initialize_namespaced(
        paths: AppPaths,
        secret_namespace: impl Into<String>,
        secret_store: Arc<dyn SecretStore>,
    ) -> Result<Self, AppStateError> {
        std::fs::create_dir_all(&paths.data_directory).map_err(|_| DiagnosticError::Operation)?;
        let diagnostics = DiagnosticWriter::new(paths.logs_directory.clone())?;
        if let Err(error) = migrate::try_first_run_legacy_migration(
            &paths.data_directory,
            &paths.legacy_search_roots,
            &paths.data_directory,
            Utc::now(),
        ) {
            let _ = diagnostics.record(&DiagnosticEvent {
                timestamp: Utc::now(),
                level: "warn".into(),
                area: "migrate".into(),
                code: error.code().to_owned(),
                request_id: "legacy-first-run".into(),
                session_id: None,
                snapshot_id: None,
                provider_id: None,
                duration_ms: None,
                retry_count: None,
            });
        }
        adopt_switched_config(&paths);
        let secrets = SecretService::new(secret_namespace, secret_store)?;
        let secret_backend_ready = secrets.status("system/startup-probe").is_ok();
        let config = ConfigStore::new(paths.config_path.clone());
        let mut startup = if secret_backend_ready {
            match config.load_for_startup() {
                Ok(ConfigLoadOutcome::Ready(_)) => StartupState::Ready,
                Ok(ConfigLoadOutcome::Migrated(_)) => StartupState::Migrated,
                Err(error) if config.load_last_good().is_ok() => StartupState::Recoverable {
                    error: public_startup_error(error.code()),
                },
                Err(error) => StartupState::Invalid {
                    error: public_startup_error(error.code()),
                },
            }
        } else {
            StartupState::Invalid {
                error: public_startup_error("SECRET_BACKEND_UNAVAILABLE"),
            }
        };
        let database_path = paths.data_directory.join("app.sqlite3");
        let database = if secret_backend_ready
            && matches!(startup, StartupState::Ready | StartupState::Migrated)
        {
            match open_and_recover(&database_path, &paths.data_directory, &config) {
                Ok(database) => Some(database),
                Err(error) => {
                    startup = StartupState::Invalid {
                        error: public_startup_error(error.code()),
                    };
                    None
                }
            }
        } else {
            None
        };
        let sessions = SessionService::new();
        let session_control = sessions.control();
        // Best-effort only: a killed process never runs Drop. Leftover device IDs
        // can restore the previous communications microphone if it is still CABLE.
        let audio_routing =
            crate::prerequisites::recover_persisted_audio_routing(&paths.data_directory);
        Ok(Self {
            secrets,
            // Arc：会话收尾的网络阶段需要不持锁使用数据库（内部自带连接互斥），
            // 外层 Option 只表达“未就绪”；替换走 repair_config。
            database: Mutex::new(database.map(Arc::new)),
            diagnostics,
            config,
            service_lock: Mutex::new(()),
            sessions: Mutex::new(sessions),
            session_control,
            event_seq: AtomicU64::new(0),
            audio_routing: Mutex::new(audio_routing),
            livestream: Mutex::new(None),
            livestream_stage: Mutex::new(None),
            livestream_playback_cancel: Mutex::new(Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            ))),
            livestream_voice: Mutex::new(None),
            obs_previous_scene: Mutex::new(None),
            operator_monitor: Mutex::new(None),
            mic_ingest: Mutex::new(None),
            database_path,
            secret_backend_ready,
            startup: RwLock::new(startup),
            paths,
        })
    }

    pub fn startup_state(&self) -> StartupState {
        self.startup
            .read()
            .map(|state| state.clone())
            .unwrap_or_else(|_| StartupState::Invalid {
                error: public_startup_error("STARTUP_STATE_UNAVAILABLE"),
            })
    }

    pub fn restore_last_good(&self) -> StartupState {
        self.repair_config(|| self.config.restore_last_good())
    }

    pub fn restore_defaults(&self) -> StartupState {
        self.repair_config(|| self.config.restore_defaults())
    }

    fn repair_config(
        &self,
        repair: impl FnOnce() -> Result<crate::config::AppConfigV1, crate::config::ConfigError>,
    ) -> StartupState {
        let next = if !self.secret_backend_ready {
            StartupState::Invalid {
                error: public_startup_error("SECRET_BACKEND_UNAVAILABLE"),
            }
        } else if let Err(error) = repair() {
            StartupState::Invalid {
                error: public_startup_error(error.code()),
            }
        } else {
            match open_and_recover(
                &self.database_path,
                &self.paths.data_directory,
                &self.config,
            ) {
                Ok(database) => {
                    if let Ok(mut slot) = self.database.lock() {
                        *slot = Some(Arc::new(database));
                    }
                    StartupState::Ready
                }
                Err(error) => StartupState::Invalid {
                    error: public_startup_error(error.code()),
                },
            }
        };
        if let Ok(mut state) = self.startup.write() {
            *state = next.clone();
        }
        next
    }
}

fn adopt_switched_config(paths: &AppPaths) {
    let switched = paths.data_directory.join("config.json");
    if switched.is_file() && !paths.config_path.exists() {
        if let Some(parent) = paths.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(&switched, &paths.config_path);
    }
}

fn open_and_recover(
    path: &std::path::Path,
    data_directory: &std::path::Path,
    config: &ConfigStore,
) -> Result<Database, DatabaseError> {
    let database = Database::open(path)?;
    if data_directory.join(".restore-journal.json").is_file() {
        crate::materials::BackupService::new(&database, data_directory, config)
            .recover_interrupted_restore()
            .map_err(|_| DatabaseError::Operation)?;
    }
    database.migrate()?;
    SessionStore::new(&database).mark_interrupted_open()?;
    Ok(database)
}

fn public_startup_error(code: &str) -> PublicError {
    let message = match code {
        "CONFIG_READ_FAILED" => "无法读取配置文件",
        "CONFIG_BACKUP_READ_FAILED" => "无法读取上次可用配置",
        "CONFIG_VERSION_UNSUPPORTED" => "配置版本不受支持",
        "DATABASE_VERSION_NEWER" => "数据文件来自更新版本的应用",
        "SECRET_BACKEND_UNAVAILABLE" => "Windows 凭据存储不可用",
        _ if code.starts_with("CONFIG_") => "配置格式无效",
        _ if code.starts_with("DATABASE_") => "本地数据库无法初始化",
        _ => "桌面服务无法初始化",
    };
    PublicError::new(code, message, false)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zeroize::Zeroizing;

    use super::{AppPaths, AppState};
    use crate::{
        contracts::StartupState,
        secrets::{MemorySecretStore, SecretError, SecretStore},
    };

    fn app_paths(directory: &tempfile::TempDir) -> AppPaths {
        AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        }
    }

    fn initialize(paths: AppPaths) -> AppState {
        AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap()
    }

    #[test]
    fn startup_state_ready_migrated_and_optional_missing_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let paths = app_paths(&directory);
        std::fs::write(&paths.config_path, r#"{"configVersion":1}"#).unwrap();
        assert!(matches!(
            initialize(paths).startup_state(),
            StartupState::Ready
        ));

        let directory = tempfile::tempdir().unwrap();
        let paths = app_paths(&directory);
        std::fs::write(&paths.config_path, r#"{"configVersion":0}"#).unwrap();
        assert!(matches!(
            initialize(paths).startup_state(),
            StartupState::Migrated
        ));
    }

    #[test]
    fn startup_state_distinguishes_recoverable_and_invalid_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let paths = app_paths(&directory);
        std::fs::write(&paths.config_path, "not-json").unwrap();
        std::fs::write(
            paths.config_path.with_extension("backup.json"),
            r#"{"configVersion":1}"#,
        )
        .unwrap();
        let state = initialize(paths.clone());
        assert!(matches!(
            state.startup_state(),
            StartupState::Recoverable { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(&paths.config_path).unwrap(),
            "not-json"
        );
        assert!(matches!(state.restore_last_good(), StartupState::Ready));
        assert!(state.config.load().is_ok());

        let directory = tempfile::tempdir().unwrap();
        let paths = app_paths(&directory);
        std::fs::write(&paths.config_path, "not-json").unwrap();
        std::fs::write(paths.config_path.with_extension("backup.json"), "also-bad").unwrap();
        assert!(matches!(
            initialize(paths.clone()).startup_state(),
            StartupState::Invalid { .. }
        ));
        let state = initialize(paths);
        assert!(matches!(state.restore_defaults(), StartupState::Ready));

        let directory = tempfile::tempdir().unwrap();
        let paths = app_paths(&directory);
        std::fs::create_dir_all(&paths.config_path).unwrap();
        assert!(matches!(
            initialize(paths).startup_state(),
            StartupState::Invalid { .. }
        ));
    }

    #[test]
    fn startup_state_fails_closed_for_database_and_secret_backend() {
        let directory = tempfile::tempdir().unwrap();
        let paths = app_paths(&directory);
        std::fs::create_dir_all(&paths.data_directory).unwrap();
        let connection =
            rusqlite::Connection::open(paths.data_directory.join("app.sqlite3")).unwrap();
        connection.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL) STRICT; INSERT INTO schema_migrations VALUES(99, 'future');").unwrap();
        drop(connection);
        assert!(
            matches!(initialize(paths).startup_state(), StartupState::Invalid { ref error } if error.code == "DATABASE_VERSION_NEWER")
        );

        struct MissingBackend;
        impl SecretStore for MissingBackend {
            fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
                Err(SecretError::Backend)
            }
            fn get(&self, _: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
                Err(SecretError::Backend)
            }
            fn delete(&self, _: &str) -> Result<bool, SecretError> {
                Err(SecretError::Backend)
            }
            fn contains(&self, _: &str) -> Result<bool, SecretError> {
                Err(SecretError::Backend)
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::initialize(app_paths(&directory), Arc::new(MissingBackend)).unwrap();
        assert!(
            matches!(state.startup_state(), StartupState::Invalid { ref error } if error.code == "SECRET_BACKEND_UNAVAILABLE")
        );
    }

    #[test]
    fn initialize_is_noop_without_legacy_and_creates_sqlite_after() {
        let directory = tempfile::tempdir().unwrap();
        let paths = app_paths(&directory);
        std::fs::write(&paths.config_path, r#"{"configVersion":1}"#).unwrap();
        let state = initialize(paths.clone());
        assert!(!paths.data_directory.join("migrated-from").exists());
        assert!(paths.data_directory.join("app.sqlite3").is_file());
        assert!(matches!(state.startup_state(), StartupState::Ready));
    }

    #[test]
    fn initialize_switches_legacy_before_creating_sqlite() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("legacy-repo");
        std::fs::create_dir_all(repo.join("config")).unwrap();
        std::fs::write(repo.join("config/local.json"), r#"{"configVersion":1}"#).unwrap();
        let mut paths = app_paths(&directory);
        paths.legacy_search_roots = vec![crate::migrate::LegacySearchRoot::Repository(repo)];
        assert!(!paths.data_directory.join("app.sqlite3").exists());

        let state = initialize(paths.clone());

        assert!(
            paths.data_directory.join("migrated-from").is_file(),
            "switch must run before AppState creates a live schema"
        );
        assert!(paths.data_directory.join("app.sqlite3").is_file());
        assert!(matches!(
            state.startup_state(),
            StartupState::Ready | StartupState::Migrated
        ));
        let status = crate::migrate::legacy_migration_status(&paths.data_directory, false);
        assert!(status.applied);
        assert!(status.reenter_secrets);
        let encoded = serde_json::to_string(&status).unwrap();
        for needle in [
            "password",
            "sk-live",
            "secretvalue",
            &crate::migrate::legacy_login_cookie_name(),
        ] {
            assert!(
                !encoded.to_ascii_lowercase().contains(needle),
                "status leaked {needle}: {encoded}"
            );
        }
    }

    #[test]
    fn initialize_continues_empty_when_legacy_first_run_fails() {
        let directory = tempfile::tempdir().unwrap();
        let mut paths = app_paths(&directory);
        std::fs::write(&paths.config_path, r#"{"configVersion":1}"#).unwrap();
        std::fs::create_dir_all(&paths.data_directory).unwrap();
        std::fs::write(paths.data_directory.join("backups"), b"not-a-directory").unwrap();
        let repo = directory.path().join("legacy-repo");
        std::fs::create_dir_all(repo.join("config")).unwrap();
        std::fs::write(repo.join("config/local.json"), r#"{"configVersion":1}"#).unwrap();
        paths.legacy_search_roots = vec![crate::migrate::LegacySearchRoot::Repository(repo)];

        let state = initialize(paths.clone());

        assert!(!paths.data_directory.join("migrated-from").exists());
        assert!(paths.data_directory.join("app.sqlite3").is_file());
        assert!(matches!(state.startup_state(), StartupState::Ready));
    }
}
