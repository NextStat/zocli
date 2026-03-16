use std::collections::BTreeMap;
use std::fs;

use crate::error::{Result, ZocliError};
use crate::model::{AccountConfig, AccountsFile, validate_datacenter};
use crate::paths::accounts_path;
use crate::persist::write_config_file;

#[derive(Clone, Debug)]
pub struct AccountStore {
    pub file: AccountsFile,
}

impl AccountStore {
    pub fn load() -> Result<Self> {
        let path = accounts_path()?;
        if !path.exists() {
            return Ok(Self {
                file: AccountsFile::default(),
            });
        }

        let content = fs::read_to_string(path)?;
        let file = toml::from_str::<AccountsFile>(&content)?;
        Ok(Self { file })
    }

    pub fn save(&self) -> Result<()> {
        let path = accounts_path()?;
        let content = toml::to_string_pretty(&self.file)?;
        write_config_file(&path, &content)
    }

    pub fn add_account(&mut self, name: String, mut account: AccountConfig) -> Result<()> {
        if self.file.accounts.contains_key(&name) {
            return Err(ZocliError::AccountExists(name));
        }
        if account.default || self.file.accounts.is_empty() {
            self.clear_default();
            account.default = true;
        }
        self.file.accounts.insert(name, account);
        Ok(())
    }

    pub fn set_current(&mut self, name: &str) -> Result<()> {
        if !self.file.accounts.contains_key(name) {
            return Err(ZocliError::AccountNotFound(name.to_string()));
        }

        self.clear_default();
        if let Some(account) = self.file.accounts.get_mut(name) {
            account.default = true;
        }
        Ok(())
    }

    pub fn resolved_account_name(&self, requested: Option<&str>) -> Result<String> {
        if let Some(name) = requested {
            if self.file.accounts.contains_key(name) {
                return Ok(name.to_string());
            }
            return Err(ZocliError::AccountNotFound(name.to_string()));
        }

        if let Some(name) = self
            .file
            .accounts
            .iter()
            .find_map(|(name, account)| account.default.then(|| name.clone()))
        {
            return Ok(name);
        }

        if self.file.accounts.len() == 1 {
            return Ok(self
                .file
                .accounts
                .keys()
                .next()
                .cloned()
                .expect("single account key exists"));
        }

        Err(ZocliError::CurrentAccountMissing)
    }

    pub fn current_account_name(&self) -> Result<String> {
        self.resolved_account_name(None)
    }

    pub fn is_current_account(&self, name: &str) -> bool {
        self.current_account_name()
            .map(|current| current == name)
            .unwrap_or(false)
    }

    pub fn get_account(&self, name: &str) -> Result<&AccountConfig> {
        self.file
            .accounts
            .get(name)
            .ok_or_else(|| ZocliError::AccountNotFound(name.to_string()))
    }

    pub fn summaries(&self) -> BTreeMap<String, &AccountConfig> {
        self.file
            .accounts
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect()
    }

    pub fn set_credential_ref(
        &mut self,
        account_name: &str,
        credential_ref: Option<String>,
    ) -> Result<()> {
        let account = self
            .file
            .accounts
            .get_mut(account_name)
            .ok_or_else(|| ZocliError::AccountNotFound(account_name.to_string()))?;

        account.credential_ref = credential_ref;
        Ok(())
    }

    pub fn set_account_id(&mut self, account_name: &str, account_id: &str) -> Result<()> {
        let account = self
            .file
            .accounts
            .get_mut(account_name)
            .ok_or_else(|| ZocliError::AccountNotFound(account_name.to_string()))?;
        account.account_id = account_id.to_string();
        Ok(())
    }

    pub fn set_zuid(&mut self, account_name: &str, zuid: &str) -> Result<()> {
        let account = self
            .file
            .accounts
            .get_mut(account_name)
            .ok_or_else(|| ZocliError::AccountNotFound(account_name.to_string()))?;
        account.zuid = Some(zuid.to_string());
        Ok(())
    }

    fn clear_default(&mut self) {
        for account in self.file.accounts.values_mut() {
            account.default = false;
        }
    }
}

#[derive(Debug)]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<String>,
}

pub fn validate_account(name: &str, account: &AccountConfig) -> ValidationReport {
    let mut errors = Vec::new();

    if name.trim().is_empty() {
        errors.push("account name must not be empty".to_string());
    }

    if !account.email.contains('@') {
        errors.push("email must contain @".to_string());
    }

    if !validate_datacenter(&account.datacenter) {
        errors.push(format!(
            "datacenter '{}' is not valid; expected one of: com, eu, in, com.au, jp, zohocloud.ca, sa, uk",
            account.datacenter
        ));
    }

    if account.account_id.trim().is_empty() {
        errors.push("account_id must not be empty".to_string());
    }

    if account.client_id.trim().is_empty() {
        errors.push("client_id must not be empty".to_string());
    }

    if let Some(reference) = account.credential_ref.as_deref() {
        if !reference.starts_with("env:") && !reference.starts_with("store:") {
            errors.push("credential_ref must use env:NAME or store:SERVICE".to_string());
        }
    }

    ValidationReport {
        valid: errors.is_empty(),
        errors,
    }
}
