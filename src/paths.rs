use std::path::PathBuf;

use crate::error::{Result, ZocliError};

pub fn config_dir() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("ZOCLI_CONFIG_DIR") {
        return Ok(PathBuf::from(value));
    }

    let base = dirs::config_dir()
        .ok_or_else(|| ZocliError::Config("Could not resolve config directory".to_string()))?;
    Ok(base.join("zocli"))
}

pub fn accounts_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("accounts.toml"))
}

pub fn credentials_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("credentials.toml"))
}
