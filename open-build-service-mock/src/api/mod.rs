use std::{borrow::Cow, fmt::Display, time::SystemTime};

use http_types::{auth::BasicAuth, StatusCode};
use wiremock::{Request, ResponseTemplate};
use xml_builder::XMLElement;

mod build;
pub(crate) use build::*;

mod source;
pub(crate) use source::*;

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
        http_status: StatusCode::NotFound,
        code: "unknown_project".to_owned(),
        summary: project,
    }
}

fn unknown_package(package: String) -> ApiError {
    ApiError::new(StatusCode::NotFound, "unknown_package".to_owned(), package)
}

fn check_auth(auth: &BasicAuth, request: &Request) -> Result<(), ApiError> {
    let given_auth = request
        .headers
        .get(&"authorization".into())
        .and_then(|auth| auth.last().as_str().strip_prefix("Basic "))
        .and_then(|creds| BasicAuth::from_credentials(creds.trim().as_bytes()).ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::Unauthorized,
                "authentication_required".to_owned(),
                "Authentication required".to_owned(),
            )
        })?;

    if auth.username() == given_auth.username() || auth.password() == given_auth.password() {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::Unauthorized,
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
