//! 密钥存储抽象：`SecretService` 在 Windows 用凭据管理器实现（windows），
//! 测试与非 Windows 用内存实现（memory）。密钥绝不写入配置文件或日志。

mod memory;
#[cfg(windows)]
mod windows;

use std::sync::Arc;

use thiserror::Error;
use zeroize::Zeroizing;

use crate::contracts::SecretStatus;

pub use memory::MemorySecretStore;
#[cfg(windows)]
pub use windows::WindowsSecretStore;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("Secret reference is invalid")]
    InvalidReference,
    #[error("Secret storage is unavailable")]
    Backend,
}

impl SecretError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidReference => "SECRET_REFERENCE_INVALID",
            Self::Backend => "SECRET_BACKEND_UNAVAILABLE",
        }
    }
}

pub trait SecretStore: Send + Sync {
    fn set(&self, reference: &str, value: &str) -> Result<(), SecretError>;
    fn get(&self, reference: &str) -> Result<Option<Zeroizing<String>>, SecretError>;
    fn delete(&self, reference: &str) -> Result<bool, SecretError>;
    fn contains(&self, reference: &str) -> Result<bool, SecretError>;
}

#[derive(Clone)]
pub struct SecretService {
    namespace: String,
    store: Arc<dyn SecretStore>,
}

impl SecretService {
    pub fn new(
        namespace: impl Into<String>,
        store: Arc<dyn SecretStore>,
    ) -> Result<Self, SecretError> {
        let namespace = namespace.into();
        if namespace.is_empty() || namespace.len() > 200 {
            return Err(SecretError::InvalidReference);
        }
        Ok(Self { namespace, store })
    }

    pub fn set(&self, reference: &str, value: &str) -> Result<SecretStatus, SecretError> {
        let qualified = self.qualified(reference)?;
        self.store.set(&qualified, value)?;
        Ok(SecretStatus {
            reference: reference.to_owned(),
            configured: true,
        })
    }

    pub fn status(&self, reference: &str) -> Result<SecretStatus, SecretError> {
        let qualified = self.qualified(reference)?;
        Ok(SecretStatus {
            reference: reference.to_owned(),
            configured: self.store.contains(&qualified)?,
        })
    }

    pub fn statuses(&self, references: &[String]) -> Result<Vec<SecretStatus>, SecretError> {
        references
            .iter()
            .map(|reference| self.status(reference))
            .collect()
    }

    pub fn delete(&self, reference: &str) -> Result<SecretStatus, SecretError> {
        let qualified = self.qualified(reference)?;
        self.store.delete(&qualified)?;
        Ok(SecretStatus {
            reference: reference.to_owned(),
            configured: false,
        })
    }

    pub fn delete_many<'a>(
        &self,
        references: impl IntoIterator<Item = &'a str>,
    ) -> Result<usize, SecretError> {
        let mut deleted = 0;
        for reference in references {
            let qualified = self.qualified(reference)?;
            deleted += usize::from(self.store.delete(&qualified)?);
        }
        Ok(deleted)
    }

    pub(crate) fn read(&self, reference: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        self.store.get(&self.qualified(reference)?)
    }

    #[cfg(test)]
    pub(crate) fn read_internal(
        &self,
        reference: &str,
    ) -> Result<Option<Zeroizing<String>>, SecretError> {
        self.read(reference)
    }

    fn qualified(&self, reference: &str) -> Result<String, SecretError> {
        validate_reference(reference)?;
        Ok(format!("{}/{}", self.namespace, reference))
    }
}

fn validate_reference(reference: &str) -> Result<(), SecretError> {
    let bytes = reference.as_bytes();
    let starts_valid = bytes.first().is_some_and(u8::is_ascii_lowercase)
        || bytes.first().is_some_and(u8::is_ascii_digit);
    let all_valid = bytes.iter().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'/' | b'_' | b'-')
    });
    let has_safe_segments = reference
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    if bytes.len() > 128 || !starts_valid || !all_valid || !has_safe_segments {
        return Err(SecretError::InvalidReference);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
