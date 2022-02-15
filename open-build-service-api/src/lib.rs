use bytes::Bytes;
use futures::future::BoxFuture;
use futures::prelude::*;
use futures::ready;
use futures::stream::BoxStream;
use quick_xml::de::DeError;
use reqwest::{RequestBuilder, Response};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context, Poll};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Request failed: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("Request deserialization failed: {0}")]
    DeError(#[from] DeError),
    #[error("{0}")]
    ApiError(ApiError),
    #[error("Unexpected result")]
    UnexpectedResult,
    #[error("Invalid client url")]
    InvalidUrl,
}

#[derive(Clone, Deserialize, Debug)]
pub struct ApiErrorSummary {
    #[serde(rename = "$value")]
    pub summary: String,
}

#[derive(Clone, Deserialize, Debug)]
pub struct ApiError {
    pub code: String,
    pub summary: ApiErrorSummary,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        write!(f, "{}: {}", self.code, self.summary.summary)
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Copy, Clone, Deserialize, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RepositoryCode {
    Unknown,
    Broken,
    Scheduling,
    Blocked,
    Building,
    Finished,
    Publishing,
    Published,
    Unpublished,
}

impl std::fmt::Display for RepositoryCode {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        self.serialize(fmt)
    }
}

#[derive(Copy, Clone, Deserialize, Serialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PackageCode {
    Unresolvable,
    Succeeded,
    Dispatching,
    Failed,
    Broken,
    Disabled,
    Excluded,
    Blocked,
    Locked,
    Unknown,
    Scheduled,
    Building,
    Finished,
}

impl PackageCode {
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Self::Disabled | Self::Succeeded | Self::Failed | Self::Excluded
        )
    }
}

impl std::fmt::Display for PackageCode {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        self.serialize(fmt)
    }
}

#[derive(Deserialize, Debug)]
pub struct JobStatus {
    pub code: Option<RepositoryCode>,
    pub details: Option<String>,
    pub workerid: Option<String>,
    pub starttime: Option<u64>,
    pub endtime: Option<u64>,
    pub lastduration: Option<u64>,
    pub hostarch: Option<String>,
    pub arch: Option<String>,
    pub jobid: Option<String>,
    pub job: Option<String>,
    pub attempt: Option<u32>,
}

#[derive(Deserialize, Debug)]
pub struct BuildStatus {
    pub package: String,
    pub code: PackageCode,
    #[serde(default)]
    pub dirty: bool,
    pub details: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct BuildHistoryEntry {
    pub rev: String,
    pub srcmd5: String,
    pub versrel: String,
    pub bcnt: String,
    pub time: String,
}

#[derive(Deserialize, Debug)]
pub struct BuildHistory {
    #[serde(rename = "entry")]
    pub entries: Vec<BuildHistoryEntry>,
}

#[derive(Deserialize, Debug)]
pub struct LinkInfo {
    pub project: String,
    pub package: String,
    pub srcmd5: String,
    pub xsrcmd5: String,
    pub lsrcmd5: String,
}

#[derive(Deserialize, Debug)]
pub struct DirectoryEntry {
    pub name: String,
    pub size: u64,
    pub md5: String,
    pub mtime: u64,
    pub originproject: Option<String>,
    //available ?
    //recommended ?
    pub hash: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Directory {
    pub name: String,
    pub rev: String,
    pub vrev: String,
    pub srcmd5: String,
    #[serde(rename = "entry")]
    pub entries: Vec<DirectoryEntry>,
    #[serde(default, rename = "linkinfo")]
    pub linkinfo: Vec<LinkInfo>,
}

#[derive(Deserialize, Debug)]
pub struct ResultListResult {
    pub project: String,
    pub repository: String,
    pub arch: String,
    pub code: RepositoryCode,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default, rename = "status")]
    pub statuses: Vec<BuildStatus>,
}

impl ResultListResult {
    pub fn get_status(&self, package: &str) -> Option<&BuildStatus> {
        self.statuses.iter().find(|s| s.package == package)
    }
}

#[derive(Deserialize, Debug)]
pub struct ResultList {
    pub state: String,
    #[serde(rename = "result")]
    pub results: Vec<ResultListResult>,
}

#[derive(Deserialize, Debug)]
struct LogEntryEntry {
    size: usize,
    mtime: u64,
}

#[derive(Deserialize, Debug)]
struct LogEntry {
    #[serde(rename = "entry")]
    pub entries: Vec<LogEntryEntry>,
}

enum PackageLogRequest {
    Initial,
    Request(BoxFuture<'static, Result<Response>>),
    Stream((BoxStream<'static, reqwest::Result<Bytes>>, bool)),
}

pub struct PackageLogStream<'a> {
    client: &'a Client,
    url: Url,
    offset: usize,
    request: PackageLogRequest,
}

impl<'a> PackageLogStream<'a> {
    fn new(client: &'a Client, offset: usize, url: Url) -> Self {
        Self {
            client,
            url,
            offset,
            request: PackageLogRequest::Initial,
        }
    }

    fn request_log(&self, offset: usize) -> Result<Url> {
        let mut url = self.url.clone();
        url.query_pairs_mut()
            .append_pair("nostream", "1")
            .append_pair("start", &format!("{}", offset));
        Ok(url)
    }
}

impl Stream for PackageLogStream<'_> {
    type Item = Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let me = self.get_mut();

        loop {
            match me.request {
                PackageLogRequest::Initial => {
                    let u = match me.request_log(me.offset) {
                        Ok(u) => u,
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    };
                    let r = me.client.get(u);
                    let r = Client::send_with_error(r).boxed();
                    me.request = PackageLogRequest::Request(r);
                }
                PackageLogRequest::Request(ref mut r) => match ready!(r.as_mut().poll(cx)) {
                    Ok(r) => {
                        me.request = PackageLogRequest::Stream((r.bytes_stream().boxed(), false))
                    }
                    Err(e) => return Poll::Ready(Some(Err(e))),
                },
                PackageLogRequest::Stream((ref mut stream, ref mut gotdata)) => {
                    match ready!(stream.as_mut().poll_next(cx)) {
                        Some(Err(e)) => return Poll::Ready(Some(Err(e.into()))),
                        Some(Ok(b)) => {
                            me.offset += b.len();
                            *gotdata = true;
                            return Poll::Ready(Some(Ok(b)));
                        }
                        None => {
                            let gotdata = *gotdata;
                            me.request = PackageLogRequest::Initial;
                            if !gotdata {
                                return Poll::Ready(None);
                            }
                        }
                    }
                }
            }
        }
    }
}

pub struct PackageLog<'a> {
    client: &'a Client,
    project: String,
    package: String,
    repository: String,
    arch: String,
}

impl<'a> PackageLog<'a> {
    fn request(&self) -> Result<Url> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project)
            .push(&self.repository)
            .push(&self.arch)
            .push(&self.package)
            .push("_log");
        Ok(u)
    }

    pub fn stream(&self, offset: usize) -> Result<PackageLogStream<'a>> {
        let u = self.request()?;
        Ok(PackageLogStream::new(self.client, offset, u))
    }

    /// Returns size and mtime
    pub async fn entry(&self) -> Result<(usize, u64)> {
        let mut u = self.request()?;
        u.query_pairs_mut().append_pair("view", "entry");

        let e: LogEntry = self.client.request(u).await?;
        if let Some(entry) = e.entries.get(0) {
            Ok((entry.size, entry.mtime))
        } else {
            Err(Error::UnexpectedResult)
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackageBuilder<'a> {
    pub client: &'a Client,
    pub project: String,
    pub package: String,
}

impl<'a> PackageBuilder<'a> {
    fn full_request(&self, repository: &str, arch: &str, command: &str) -> Result<Url> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project)
            .push(repository)
            .push(arch)
            .push(&self.package)
            .push(command);
        Ok(u)
    }

    pub async fn jobstatus(&self, repository: &str, arch: &str) -> Result<JobStatus> {
        let u = self.full_request(repository, arch, "_jobstatus")?;
        self.client.request(u).await
    }

    pub async fn history(&self, repository: &str, arch: &str) -> Result<BuildHistory> {
        let u = self.full_request(repository, arch, "_history")?;
        self.client.request(u).await
    }

    pub async fn status(&self, repository: &str, arch: &str) -> Result<BuildStatus> {
        let u = self.full_request(repository, arch, "_status")?;
        self.client.request(u).await
    }

    pub fn log(&self, repository: &str, arch: &str) -> PackageLog<'a> {
        PackageLog {
            client: self.client,
            project: self.project.clone(),
            package: self.package.clone(),
            repository: repository.to_owned(),
            arch: arch.to_owned(),
        }
    }

    pub async fn list(&self) -> Result<Directory> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package);
        self.client.request(u).await
    }

    pub async fn result(&self) -> Result<ResultList> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project)
            .push("_result");
        u.query_pairs_mut().append_pair("package", &self.package);
        self.client.request(u).await
    }
}

pub struct ProjectBuilder<'a> {
    client: &'a Client,
    project: String,
}

impl<'a> ProjectBuilder<'a> {
    pub fn package(self, package: String) -> PackageBuilder<'a> {
        PackageBuilder {
            client: self.client,
            project: self.project,
            package,
        }
    }

    pub async fn result(&self) -> Result<ResultList> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project)
            .push("_result");
        self.client.request(u).await
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    base: Url,
    user: String,
    pass: String,
    client: reqwest::Client,
}

impl Client {
    pub fn new(url: Url, user: String, pass: String) -> Self {
        Client {
            base: url,
            user,
            pass,
            client: reqwest::Client::new(),
        }
    }

    pub fn project(&self, project: String) -> ProjectBuilder {
        ProjectBuilder {
            client: self,
            project,
        }
    }

    fn get(&self, url: Url) -> RequestBuilder {
        self.client
            .get(url)
            .basic_auth(&self.user, Some(&self.pass))
    }

    async fn send_with_error(request: RequestBuilder) -> Result<Response> {
        let response = request.send().await?;

        match response.error_for_status_ref() {
            Ok(_) => Ok(response),
            Err(e) => {
                if let Some(status) = e.status() {
                    if status.is_client_error() {
                        let data = response.text().await?;
                        let error = quick_xml::de::from_str(&data)?;
                        Err(Error::ApiError(error))
                    } else {
                        Err(e.into())
                    }
                } else {
                    Err(e.into())
                }
            }
        }
    }

    async fn request<T: DeserializeOwned + std::fmt::Debug>(&self, url: Url) -> Result<T> {
        let data = Self::send_with_error(self.get(url)).await?.text().await?;
        quick_xml::de::from_str(&data).map_err(|e| e.into())
    }
}
