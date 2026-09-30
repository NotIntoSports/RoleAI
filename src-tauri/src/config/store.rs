use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use super::{AppConfigV1, ConfigError, ConfigPatch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLoadOutcome {
    Ready(AppConfigV1),
    Migrated(AppConfigV1),
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn load(&self) -> Result<AppConfigV1, ConfigError> {
        if !self.path.exists() {
            return Ok(AppConfigV1::default());
        }
        let json = fs::read_to_string(&self.path)
            .map_err(|error| ConfigError::new("CONFIG_READ_FAILED", error.to_string()))?;
        AppConfigV1::from_json(&json)
    }

    pub fn load_for_startup(&self) -> Result<ConfigLoadOutcome, ConfigError> {
        let outcome = self.load_for_startup_inner()?;
        let mut config = match &outcome {
            ConfigLoadOutcome::Ready(config) | ConfigLoadOutcome::Migrated(config) => {
                config.clone()
            }
        };
        let mut changed = super::presets::ensure_role_presets(&mut config);
        changed |= super::presets_local::ensure_local_model_presets(&mut config);
        if changed {
            let _guard = self.lock_writes()?;
            self.write_validated(&config)?;
            return Ok(match outcome {
                ConfigLoadOutcome::Ready(_) => ConfigLoadOutcome::Ready(config),
                ConfigLoadOutcome::Migrated(_) => ConfigLoadOutcome::Migrated(config),
            });
        }
        Ok(outcome)
    }

    fn load_for_startup_inner(&self) -> Result<ConfigLoadOutcome, ConfigError> {
        if !self.path.exists() {
            return self.restore_defaults().map(ConfigLoadOutcome::Migrated);
        }
        let json = fs::read_to_string(&self.path)
            .map_err(|error| ConfigError::new("CONFIG_READ_FAILED", error.to_string()))?;
        let mut value: serde_json::Value = serde_json::from_str(&json)
            .map_err(|error| ConfigError::new("CONFIG_INVALID", error.to_string()))?;
        if value
            .get("configVersion")
            .and_then(serde_json::Value::as_u64)
            == Some(0)
        {
            value["configVersion"] = serde_json::json!(1);
            let config = AppConfigV1::from_json(&value.to_string())?;
            self.write_validated(&config)?;
            return Ok(ConfigLoadOutcome::Migrated(config));
        }
        AppConfigV1::from_json(&json).map(ConfigLoadOutcome::Ready)
    }

    pub fn save_patch(&self, patch: ConfigPatch) -> Result<AppConfigV1, ConfigError> {
        let _guard = self.lock_writes()?;
        let mut config = self.load()?;
        if let Some(diagnostics) = patch.diagnostics
            && let Some(days) = diagnostics.log_retention_days
        {
            config.diagnostics.log_retention_days = days;
        }
        config.validate()?;
        self.write_validated(&config)?;
        Ok(config)
    }

    pub fn update<F>(&self, mutation: F) -> Result<AppConfigV1, ConfigError>
    where
        F: FnOnce(&mut AppConfigV1) -> Result<(), ConfigError>,
    {
        let _guard = self.lock_writes()?;
        let mut config = self.load()?;
        mutation(&mut config)?;
        self.write_validated(&config)?;
        Ok(config)
    }

    pub fn last_good_path(&self) -> PathBuf {
        self.path.with_extension("backup.json")
    }

    pub fn restore_last_good(&self) -> Result<AppConfigV1, ConfigError> {
        let _guard = self.lock_writes()?;
        let config = self.load_last_good()?;
        self.write_validated(&config)?;
        Ok(config)
    }

    pub fn load_last_good(&self) -> Result<AppConfigV1, ConfigError> {
        let json = fs::read_to_string(self.last_good_path())
            .map_err(|error| ConfigError::new("CONFIG_BACKUP_READ_FAILED", error.to_string()))?;
        AppConfigV1::from_json(&json)
    }

    pub fn restore_defaults(&self) -> Result<AppConfigV1, ConfigError> {
        let _guard = self.lock_writes()?;
        let config = AppConfigV1::default();
        self.write_validated(&config)?;
        Ok(config)
    }

    fn lock_writes(&self) -> Result<std::sync::MutexGuard<'_, ()>, ConfigError> {
        self.write_lock.lock().map_err(|_| {
            ConfigError::new(
                "CONFIG_WRITE_FAILED",
                "Configuration write lock is unavailable",
            )
        })
    }

    fn write_validated(&self, config: &AppConfigV1) -> Result<(), ConfigError> {
        config.validate()?;
        let json = serde_json::to_vec_pretty(config)
            .map_err(|error| ConfigError::new("CONFIG_WRITE_FAILED", error.to_string()))?;
        // The primary file is the commit point. Write the recovery copy first so
        // a backup failure can never leave callers observing a committed primary
        // while they roll back a related Credential Manager mutation.
        let backup_path = self.last_good_path();
        let previous_backup = fs::read(&backup_path).ok();
        atomic_write(&backup_path, &json)?;
        if let Err(error) = atomic_write(&self.path, &json) {
            match previous_backup {
                Some(bytes) => atomic_write(&backup_path, &bytes)?,
                None => {
                    let _ = fs::remove_file(&backup_path);
                }
            }
            return Err(error);
        }
        Ok(())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let parent = path.parent().ok_or_else(|| {
        ConfigError::new("CONFIG_WRITE_FAILED", "Configuration path has no parent")
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| ConfigError::new("CONFIG_WRITE_FAILED", error.to_string()))?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config"),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| ConfigError::new("CONFIG_WRITE_FAILED", error.to_string()))?;
        file.write_all(bytes)
            .map_err(|error| ConfigError::new("CONFIG_WRITE_FAILED", error.to_string()))?;
        file.sync_all()
            .map_err(|error| ConfigError::new("CONFIG_WRITE_FAILED", error.to_string()))?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), ConfigError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(ConfigError::new(
            "CONFIG_WRITE_FAILED",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), ConfigError> {
    fs::rename(source, destination)
        .map_err(|error| ConfigError::new("CONFIG_WRITE_FAILED", error.to_string()))
}
