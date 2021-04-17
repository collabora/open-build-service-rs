use secret_service::EncryptionType;
use secret_service::SecretService;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use thiserror::Error;
use url::Url;

const SECRET_SERVICE: &str =
    "osc.credentials.KeyringCredentialsManager:keyring.backends.SecretService.Keyring";

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to read: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Failed to parse: {0}")]
    ParseError(#[from] serde_ini::de::Error),
}

#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("No data for service")]
    UnknownUrl,
    #[error("Missing password for service")]
    MissingPass,
    #[error("Password not in secrets service")]
    MissingSecretsPass,
    #[error("Mallformed Password: {0}")]
    MalformedPass(#[from] std::string::FromUtf8Error),
    #[error("Failed to get password from secret service: {0}")]
    SecretService(#[from] secret_service::Error),
    #[error("Unknown credentail manager: {0}")]
    UnknownCredMgr(String),
}

#[derive(Deserialize, Debug)]
struct Service {
    user: String,
    credentials_mgr_class: Option<String>,
    pass: Option<String>,
}

#[derive(Deserialize, Debug)]
struct General {
    apiurl: Url,
}

#[derive(Deserialize, Debug)]
pub struct Oscrc {
    general: General,
    #[serde(flatten)]
    services: HashMap<Url, Service>,
}

impl Oscrc {
    fn pass_from_secretservice(user: &str, service: &Url) -> Result<String, CredentialsError> {
        let ss = SecretService::new(EncryptionType::Dh).unwrap();
        let service = service.domain().ok_or(CredentialsError::UnknownUrl)?;

        let items = ss.search_items(vec![("username", user), ("service", service)])?;
        let item = items.get(0).ok_or(CredentialsError::MissingSecretsPass)?;
        let secret = item.get_secret()?;
        let pass = String::from_utf8(secret)?;

        Ok(pass)
    }

    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let oscrc = File::open(path)?;
        serde_ini::from_read(oscrc).map_err(|e| e.into())
    }

    pub fn default_service(&self) -> &Url {
        &self.general.apiurl
    }

    pub fn credentials(&self, service: &Url) -> Result<(String, String), CredentialsError> {
        let s = self
            .services
            .get(service)
            .ok_or(CredentialsError::UnknownUrl)?;
        let user = s.user.clone();
        let pass = if let Some(pass) = &s.pass {
            pass.clone()
        } else if let Some(credmgr) = &s.credentials_mgr_class {
            match credmgr.as_str() {
                SECRET_SERVICE => Self::pass_from_secretservice(&user, service)?,
                _ => return Err(CredentialsError::UnknownCredMgr(credmgr.clone())),
            }
        } else {
            return Err(CredentialsError::MissingPass);
        };

        Ok((user, pass))
    }
}
