use std::{borrow::Cow, fmt::Display, time::SystemTime};

use http::{header::AUTHORIZATION, StatusCode};
use wiremock::{Request, ResponseTemplate};
use xml_builder::XMLElement;

mod build;
pub(crate) use build::*;

mod source;
pub(crate) use source::*;

// BasicAuth Adapted from http-rs/http-types crate
pub struct BasicAuth {
    username: String,
    password: String,
}

impl BasicAuth {
    pub fn new<U: AsRef<str>, P: AsRef<str>>(username: U, password: P) -> Self {
        Self {
            username: username.as_ref().to_owned(),
            password: password.as_ref().to_owned(),
        }
    }

    pub fn from_credentials(credentials: impl AsRef<[u8]>) -> Result<Self, ()> {
        use base64ct::{Base64, Encoding};
        let credentials = std::str::from_utf8(credentials.as_ref()).map_err(|_| ())?;
        let bytes = Base64::decode_vec(credentials).map_err(|_| ())?;

        let credentials = String::from_utf8(bytes).map_err(|_| ())?;

        let mut iter = credentials.splitn(2, ':');
        let username = iter.next();
        let password = iter.next();

        let (username, password) = match (username, password) {
            (Some(username), Some(password)) => (username.to_string(), password.to_string()),
            (Some(_), None) => return Err(()),
            (None, _) => return Err(()),
        };

        Ok(Self { username, password })
    }

    pub fn username(&self) -> &str {
        self.username.as_str()
    }

    pub fn password(&self) -> &str {
        self.password.as_str()
    }
}

fn build_status_xml(code: &str, summary: Option<String>) -> XMLElement {
    let mut status_xml = XMLElement::new("status");
    status_xml.add_attribute("code", code);

    if let Some(summary) = summary {
        let mut summary_xml = XMLElement::new("summary");
        summary_xml.add_text(summary).unwrap();

        status_xml.add_child(summary_xml).unwrap();
    }

    status_xml
}

trait ResponseTemplateUtils {
    fn set_body_xml(self, xml: XMLElement) -> Self;
}

impl ResponseTemplateUtils for ResponseTemplate {
    fn set_body_xml(self, xml: XMLElement) -> Self {
        let mut body = vec![];
        xml.render(&mut body, false, true).unwrap();
        self.set_body_raw(body, "application/xml")
    }
}

#[derive(Debug)]
struct ApiError {
    http_status: StatusCode,
    code: String,
    summary: String,
}

impl ApiError {
    fn new(http_status: StatusCode, code: String, summary: String) -> ApiError {
        ApiError {
            http_status,
            code,
            summary,
        }
    }

    fn into_xml(self) -> XMLElement {
        build_status_xml(&self.code, Some(self.summary))
    }

    fn into_response(self) -> ResponseTemplate {
        ResponseTemplate::new(self.http_status).set_body_xml(self.into_xml())
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}: {}", self.http_status, self.code, self.summary)
    }
}

fn unknown_project(project: String) -> ApiError {
    ApiError {
        http_status: StatusCode::NOT_FOUND,
        code: "unknown_project".to_owned(),
        summary: project,
    }
}

fn unknown_package(package: String) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "unknown_package".to_owned(), package)
}

fn check_auth(auth: &BasicAuth, request: &Request) -> Result<(), ApiError> {
    let given_auth = request
        .headers
        .get(AUTHORIZATION)
        .and_then(|auth| auth.to_str().ok())
        .and_then(|s| s.strip_prefix("Basic "))
        .and_then(|creds| BasicAuth::from_credentials(creds.trim().as_bytes()).ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "authentication_required".to_owned(),
                "Authentication required".to_owned(),
            )
        })?;

    if auth.username() == given_auth.username() || auth.password() == given_auth.password() {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "authentication_required".to_owned(),
            format!(
                "Unknown user '{}' or invalid password",
                given_auth.username()
            ),
        ))
    }
}

fn find_query_param<'r>(request: &'r Request, name: &str) -> Option<Cow<'r, str>> {
    request
        .url
        .query_pairs()
        .find_map(|(key, value)| if key == name { Some(value) } else { None })
}

fn seconds_since_epoch(time: &SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// Some anyhow-inspired helper macros to make error checking easier.

macro_rules! ensure {
    ($test:expr, $err:expr $(,)?) => {
        if !$test {
            return $err.into_response();
        }
    };
}

macro_rules! try_api {
    ($value:expr $(,)?) => {
        match $value {
            Ok(_v) => _v,
            Err(_err) => return _err.into_response(),
        }
    };
}

pub(crate) use ensure;
pub(crate) use try_api;
