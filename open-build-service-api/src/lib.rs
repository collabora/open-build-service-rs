use bytes::Bytes;
use futures::future::BoxFuture;
use futures::prelude::*;
use futures::ready;
use futures::stream::BoxStream;
use md5::{Digest, Md5};
use quick_xml::{de::DeError, events::Event};
use reqwest::{Body, Method, RequestBuilder, Response, header::CONTENT_TYPE};
use serde::de::IntoDeserializer;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use strum_macros::Display;
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

#[derive(Clone, Copy, Default, Deserialize, Debug, Eq, PartialEq, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum RebuildMode {
    #[default]
    Transitive,
    Direct,
    Local,
}

#[derive(Clone, Copy, Deserialize, Default, Debug, Eq, PartialEq, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum BlockMode {
    #[default]
    All,
    Local,
    Never,
}

#[derive(Deserialize, Debug)]
pub struct RepositoryMeta {
    pub name: String,
    #[serde(default)]
    pub rebuild: RebuildMode,
    #[serde(default)]
    pub block: BlockMode,

    #[serde(default, rename = "arch")]
    pub arches: Vec<String>,
}

#[derive(Deserialize, Debug)]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default, rename = "repository")]
    pub repositories: Vec<RepositoryMeta>,
}

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
            Self::Broken | Self::Disabled | Self::Excluded | Self::Failed | Self::Succeeded
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
    pub duration: String,
}

#[derive(Deserialize, Debug)]
pub struct BuildHistory {
    #[serde(default, rename = "entry")]
    pub entries: Vec<BuildHistoryEntry>,
}

#[derive(Deserialize, Debug)]
pub struct LinkInfo {
    pub project: String,
    pub package: String,
    pub srcmd5: String,
    pub xsrcmd5: String,
    pub lsrcmd5: String,
    #[serde(default)]
    pub missingok: bool,
}

#[derive(Deserialize, Debug)]
pub struct SourceDirectoryEntry {
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
pub struct SourceDirectory {
    pub name: String,
    pub rev: Option<String>,
    pub vrev: Option<String>,
    pub srcmd5: String,
    #[serde(default, rename = "entry")]
    pub entries: Vec<SourceDirectoryEntry>,
    #[serde(default, rename = "linkinfo")]
    pub linkinfo: Vec<LinkInfo>,
}

#[derive(Clone, Deserialize, Debug)]
pub struct Revision {
    pub rev: String,
    pub vrev: String,
    pub srcmd5: String,
    pub version: String,
    pub time: u64,
    pub user: String,
    pub comment: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct RevisionList {
    #[serde(default, rename = "revision")]
    pub revisions: Vec<Revision>,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
pub struct CommitEntry {
    pub name: String,
    pub md5: String,
}

impl CommitEntry {
    pub fn from_contents<T: AsRef<[u8]>>(name: String, contents: T) -> CommitEntry {
        let md5 = base16ct::lower::encode_string(&Md5::digest(&contents));
        CommitEntry { name, md5 }
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "error", rename = "missing")]
pub struct MissingEntries {
    #[serde(rename = "entry")]
    pub entries: Vec<CommitEntry>,
}

#[derive(Debug)]
pub enum CommitResult {
    Success(SourceDirectory),
    MissingEntries(MissingEntries),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename = "directory")]
pub struct CommitFileList {
    #[serde(rename = "entry")]
    entries: Vec<CommitEntry>,
}

impl CommitFileList {
    pub fn new() -> Self {
        CommitFileList::default()
    }

    pub fn add_entry(&mut self, entry: CommitEntry) {
        self.entries.push(entry);
    }

    pub fn add_file_md5(&mut self, name: String, md5: String) {
        self.add_entry(CommitEntry { name, md5 });
    }

    pub fn add_file_from_contents(&mut self, name: String, contents: &[u8]) {
        self.add_entry(CommitEntry::from_contents(name, contents));
    }

    pub fn entry(mut self, entry: CommitEntry) -> Self {
        self.add_entry(entry);
        self
    }

    pub fn file_md5(mut self, name: String, md5: String) -> Self {
        self.add_file_md5(name, md5);
        self
    }

    pub fn file_from_contents(mut self, name: String, contents: &[u8]) -> Self {
        self.add_file_from_contents(name, contents);
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct CommitOptions {
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct BranchOptions {
    pub target_project: Option<String>,
    pub target_package: Option<String>,
    pub comment: Option<String>,
    pub force: bool,
    pub missingok: bool,

    pub add_repositories_rebuild: Option<RebuildMode>,
    pub add_repositories_block: Option<BlockMode>,
}

#[derive(Clone, Debug)]
pub struct BranchStatus {
    pub source_project: String,
    pub source_package: String,
    pub target_project: String,
    pub target_package: String,
}

impl<'de> Deserialize<'de> for BranchStatus {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct BranchStatusDataItem {
            name: String,
            #[serde(rename = "$value")]
            value: String,
        }

        #[derive(Deserialize)]
        struct BranchStatusData {
            data: Vec<BranchStatusDataItem>,
        }

        #[derive(Deserialize)]
        struct BranchStatusExpanded {
            sourceproject: String,
            sourcepackage: String,
            targetproject: String,
            targetpackage: String,
        }

        let data: HashMap<String, String> = BranchStatusData::deserialize(deserializer)?
            .data
            .into_iter()
            .map(|BranchStatusDataItem { name, value }| (name, value))
            .collect();

        let expanded = BranchStatusExpanded::deserialize(data.into_deserializer())?;
        Ok(BranchStatus {
            source_project: expanded.sourceproject,
            source_package: expanded.sourcepackage,
            target_project: expanded.targetproject,
            target_package: expanded.targetpackage,
        })
    }
}

#[derive(Deserialize, Debug)]
pub struct PackageBuildMetaDisable {
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
pub struct PackageBuildMeta {
    #[serde(rename = "disable")]
    pub disabled: Vec<PackageBuildMetaDisable>,
}

#[derive(Deserialize, Debug)]
pub struct PackageMeta {
    pub name: String,
    pub project: String,
    #[serde(default)]
    pub build: PackageBuildMeta,
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

#[derive(Clone, Deserialize, Debug)]
pub struct Binary {
    pub filename: String,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Clone, Deserialize, Debug)]
pub struct BinaryList {
    #[serde(default, rename = "binary")]
    pub binaries: Vec<Binary>,
}

#[derive(Deserialize, Debug)]
pub struct DirectoryEntry {
    pub name: String,
}

#[derive(Deserialize, Debug)]
pub struct Directory {
    #[serde(default, rename = "entry")]
    pub entries: Vec<DirectoryEntry>,
}

#[derive(Clone, Debug)]
pub struct RebuildFilters {
    packages: Vec<String>,
}

impl RebuildFilters {
    pub fn empty() -> Self {
        RebuildFilters {
            packages: Vec::new(),
        }
    }

    pub fn only_package(package: String) -> Self {
        RebuildFilters::empty().package(package)
    }

    pub fn add_package(&mut self, package: String) {
        self.packages.push(package);
    }

    pub fn package(mut self, package: String) -> Self {
        self.add_package(package);
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct JobHistoryFilters {
    packages: Vec<String>,
    codes: Vec<PackageCode>,
    limit: Option<usize>,
}

impl JobHistoryFilters {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn only_package(package: String) -> Self {
        JobHistoryFilters::empty().package(package)
    }

    pub fn add_package(&mut self, package: String) {
        self.packages.push(package);
    }

    pub fn add_code(&mut self, code: PackageCode) {
        self.codes.push(code);
    }

    pub fn set_limit(&mut self, limit: Option<usize>) {
        self.limit = limit;
    }

    pub fn package(mut self, package: String) -> Self {
        self.add_package(package);
        self
    }

    pub fn code(mut self, code: PackageCode) -> Self {
        self.add_code(code);
        self
    }

    pub fn limit(mut self, limit: Option<usize>) -> Self {
        self.set_limit(limit);
        self
    }
}

#[derive(Deserialize, Debug)]
pub struct JobHist {
    pub package: String,
    pub rev: String,
    pub srcmd5: String,
    pub versrel: String,
    pub bcnt: String,
    pub readytime: u64,
    pub starttime: u64,
    pub endtime: u64,
    pub code: PackageCode,
    pub uri: String,
    pub workerid: String,
    pub hostarch: String,
    pub reason: String,
    pub verifymd5: String,
}

#[derive(Deserialize, Debug)]
pub struct JobHistList {
    #[serde(default)]
    pub jobhist: Vec<JobHist>,
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

#[derive(Default)]
pub struct PackageLogStreamOptions {
    pub offset: Option<usize>,
    pub end: Option<usize>,
}

pub struct PackageLogStream<'a> {
    client: &'a Client,
    url: Url,
    offset: usize,
    options: PackageLogStreamOptions,
    request: PackageLogRequest,
}

impl<'a> PackageLogStream<'a> {
    fn new(client: &'a Client, options: PackageLogStreamOptions, url: Url) -> Self {
        Self {
            client,
            url,
            offset: options.offset.unwrap_or(0),
            options,
            request: PackageLogRequest::Initial,
        }
    }

    fn request_log(&self, offset: usize) -> Result<Url> {
        let mut url = self.url.clone();
        url.query_pairs_mut()
            .append_pair("nostream", "1")
            .append_pair("start", &format!("{offset}"));
        if let Some(end) = self.options.end {
            url.query_pairs_mut().append_pair("end", &end.to_string());
        }
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
                    let r = me.client.authenticated_request(Method::GET, u);
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
                            if !gotdata || matches!(me.options.end, Some(end) if me.offset >= end) {
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

    pub fn stream(&self, options: PackageLogStreamOptions) -> Result<PackageLogStream<'a>> {
        let u = self.request()?;
        Ok(PackageLogStream::new(self.client, options, u))
    }

    /// Returns size and mtime
    pub async fn entry(&self) -> Result<(usize, u64)> {
        let mut u = self.request()?;
        u.query_pairs_mut().append_pair("view", "entry");

        let e: LogEntry = self.client.request(u).await?;
        if let Some(entry) = e.entries.first() {
            Ok((entry.size, entry.mtime))
        } else {
            Err(Error::UnexpectedResult)
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum BuildCommand<'b> {
    JobStatus,
    History,
    Status,
    DownloadBinary(&'b str),
}

impl AsRef<str> for BuildCommand<'_> {
    fn as_ref(&self) -> &str {
        match self {
            BuildCommand::JobStatus => "_jobstatus",
            BuildCommand::History => "_history",
            BuildCommand::Status => "_status",
            BuildCommand::DownloadBinary(binary) => binary,
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
    fn full_request(
        &self,
        repository: &str,
        arch: &str,
        command: Option<BuildCommand<'_>>,
    ) -> Result<Url> {
        let mut u = self.client.base.clone();

        {
            let mut path = u.path_segments_mut().map_err(|_| Error::InvalidUrl)?;

            path.push("build")
                .push(&self.project)
                .push(repository)
                .push(arch)
                .push(&self.package);

            if let Some(command) = command {
                path.push(command.as_ref());
            }
        }

        Ok(u)
    }

    async fn upload_file<T: Into<Body>>(
        &self,
        file: &str,
        rev: Option<&str>,
        data: T,
    ) -> Result<()> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package)
            .push(file);

        if let Some(rev) = rev {
            u.query_pairs_mut().append_pair("rev", rev);
        }

        Client::send_with_error(
            self.client
                .authenticated_request(Method::PUT, u)
                .header(CONTENT_TYPE, "application/octet-stream")
                .body(data),
        )
        .await?;

        Ok(())
    }

    pub async fn jobstatus(&self, repository: &str, arch: &str) -> Result<JobStatus> {
        let u = self.full_request(repository, arch, Some(BuildCommand::JobStatus))?;
        self.client.request(u).await
    }

    pub async fn history(&self, repository: &str, arch: &str) -> Result<BuildHistory> {
        let u = self.full_request(repository, arch, Some(BuildCommand::History))?;
        self.client.request(u).await
    }

    pub async fn status(&self, repository: &str, arch: &str) -> Result<BuildStatus> {
        let u = self.full_request(repository, arch, Some(BuildCommand::Status))?;
        self.client.request(u).await
    }

    pub async fn binary_file(
        &self,
        repository: &str,
        arch: &str,
        file: &str,
    ) -> Result<impl Stream<Item = Result<Bytes>>> {
        let u = self.full_request(repository, arch, Some(BuildCommand::DownloadBinary(file)))?;
        Ok(
            Client::send_with_error(self.client.authenticated_request(Method::GET, u))
                .await?
                .bytes_stream()
                .map_err(|e| e.into()),
        )
    }

    pub async fn binaries(&self, repository: &str, arch: &str) -> Result<BinaryList> {
        let u = self.full_request(repository, arch, None)?;
        self.client.request(u).await
    }

    pub async fn rebuild(&self) -> Result<()> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project);

        u.query_pairs_mut().append_pair("cmd", "rebuild");
        u.query_pairs_mut().append_pair("package", &self.package);

        Client::send_with_error(self.client.authenticated_request(Method::POST, u)).await?;

        Ok(())
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

    pub async fn create(&self) -> Result<()> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package)
            .push("_meta");

        self.upload_file("_meta", None, "<package/>").await?;
        Ok(())
    }

    pub async fn delete(&self) -> Result<()> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package);

        Client::send_with_error(self.client.authenticated_request(Method::DELETE, u)).await?;

        Ok(())
    }

    pub async fn revisions(&self) -> Result<RevisionList> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package)
            .push("_history");
        self.client.request(u).await
    }

    fn list_url(&self, rev: Option<&str>) -> Result<reqwest::Url> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package);

        if let Some(rev) = rev {
            u.query_pairs_mut().append_pair("rev", rev);
        }

        Ok(u)
    }

    pub async fn list(&self, rev: Option<&str>) -> Result<SourceDirectory> {
        let u = self.list_url(rev)?;
        self.client.request(u).await
    }

    pub async fn list_meta(&self, rev: Option<&str>) -> Result<SourceDirectory> {
        let mut u = self.list_url(rev)?;
        u.query_pairs_mut().append_pair("meta", "1");
        self.client.request(u).await
    }

    pub async fn meta(&self) -> Result<PackageMeta> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package)
            .push("_meta");
        self.client.request(u).await
    }

    pub async fn source_file(&self, file: &str) -> Result<impl Stream<Item = Result<Bytes>>> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package)
            .push(file);
        Ok(
            Client::send_with_error(self.client.authenticated_request(Method::GET, u))
                .await?
                .bytes_stream()
                .map_err(|e| e.into()),
        )
    }

    pub async fn upload_for_commit<T: Into<Body>>(&self, file: &str, data: T) -> Result<()> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package)
            .push(file);
        self.upload_file(file, Some("repository"), data).await?;
        Ok(())
    }

    pub async fn commit(
        &self,
        filelist: &CommitFileList,
        options: &CommitOptions,
    ) -> Result<CommitResult> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package);
        u.query_pairs_mut().append_pair("cmd", "commitfilelist");

        if let Some(comment) = &options.comment {
            u.query_pairs_mut().append_pair("comment", comment);
        }

        let mut body = Vec::new();
        quick_xml::se::to_writer(&mut body, filelist)?;

        let response = Client::send_with_error(
            self.client
                .authenticated_request(Method::POST, u)
                .header(CONTENT_TYPE, "application/xml")
                .body(body),
        )
        .await?
        .text()
        .await?;

        // We determine whether or not there were missing entries by the
        // presence of the "error" key, then use that to choose what enum value
        // to deserialize to. Ideally, we would be able to use untagged enum
        // magic: https://stackoverflow.com/a/61219284/2097780
        // Unfortunately, serde implementation details collide with quick-xml to
        // result in that not functioning here:
        // https://github.com/serde-rs/serde/issues/1183
        // https://github.com/tafia/quick-xml/issues/190
        // https://github.com/tafia/quick-xml/issues/203
        // Untagged enum deserialization logic depends on private serde API
        // functions, so it's not possible to implement it cleanly in a custom
        // "Deserialize".

        let mut reader = quick_xml::Reader::from_str(&response);
        reader.trim_text(true);
        let mut buf = Vec::new();
        if let Event::Start(e) = reader.read_event(&mut buf).map_err(DeError::from)? {
            let mut is_missing = false;
            for attr in e.attributes() {
                let attr = attr.map_err(DeError::from)?;
                if attr.key == b"error" {
                    if attr.value.as_ref() != b"missing" {
                        return Err(DeError::Custom(
                            "only supported value for 'error' is 'missing'".to_owned(),
                        )
                        .into());
                    }

                    is_missing = true;
                    break;
                }
            }

            Ok(if is_missing {
                CommitResult::MissingEntries(quick_xml::de::from_str(&response)?)
            } else {
                CommitResult::Success(quick_xml::de::from_str(&response)?)
            })
        } else {
            Err(DeError::Start.into())
        }
    }

    pub async fn branch(&self, options: &BranchOptions) -> Result<BranchStatus> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push(&self.package);
        u.query_pairs_mut().append_pair("cmd", "branch");

        if let Some(target_project) = &options.target_project {
            u.query_pairs_mut()
                .append_pair("target_project", target_project);
        }

        if let Some(target_package) = &options.target_package {
            u.query_pairs_mut()
                .append_pair("target_package", target_package);
        }

        if let Some(comment) = &options.comment {
            u.query_pairs_mut().append_pair("comment", comment);
        }

        if let Some(rebuild) = &options.add_repositories_rebuild {
            u.query_pairs_mut()
                .append_pair("add_repositories_rebuild", &rebuild.to_string());
        }

        if let Some(block) = &options.add_repositories_block {
            u.query_pairs_mut()
                .append_pair("add_repositories_block", &block.to_string());
        }

        if options.force {
            u.query_pairs_mut().append_pair("force", "1");
        }

        if options.missingok {
            u.query_pairs_mut().append_pair("missingok", "1");
        }

        self.client.post_request(u).await
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

    pub async fn delete(&self) -> Result<()> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project);

        Client::send_with_error(self.client.authenticated_request(Method::DELETE, u)).await?;

        Ok(())
    }

    pub async fn list_packages(&self) -> Result<Directory> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project);
        self.client.request(u).await
    }

    pub async fn meta(&self) -> Result<ProjectMeta> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("source")
            .push(&self.project)
            .push("_meta");
        self.client.request(u).await
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

    pub async fn repositories(&self) -> Result<Vec<String>> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project);
        Ok(self
            .client
            .request::<Directory>(u)
            .await?
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect())
    }

    pub async fn arches(&self, repository: &str) -> Result<Vec<String>> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project)
            .push(repository);
        Ok(self
            .client
            .request::<Directory>(u)
            .await?
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect())
    }

    pub async fn rebuild(&self, filters: &RebuildFilters) -> Result<()> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project);

        u.query_pairs_mut().append_pair("cmd", "rebuild");
        for package in &filters.packages {
            u.query_pairs_mut().append_pair("package", package);
        }

        Client::send_with_error(self.client.authenticated_request(Method::POST, u)).await?;

        Ok(())
    }

    pub async fn jobhistory(
        &self,
        repository: &str,
        arch: &str,
        filters: &JobHistoryFilters,
    ) -> Result<JobHistList> {
        let mut u = self.client.base.clone();
        u.path_segments_mut()
            .map_err(|_| Error::InvalidUrl)?
            .push("build")
            .push(&self.project)
            .push(repository)
            .push(arch)
            .push("_jobhistory");

        for package in &filters.packages {
            u.query_pairs_mut().append_pair("package", package);
        }

        for code in &filters.codes {
            u.query_pairs_mut().append_pair("code", &code.to_string());
        }

        if let Some(limit) = &filters.limit {
            u.query_pairs_mut().append_pair("limit", &limit.to_string());
        }

        self.client.request(u).await
    }
}

#[derive(Clone)]
pub struct Client {
    base: Url,
    user: String,
    pass: String,
    client: reqwest::Client,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &format_args!("{:?}", self.base))
            .field("user", &self.user)
            .field("pass", &"[redacted]")
            .field("client", &format_args!("{:?}", self.client))
            .finish()
    }
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

    pub fn url(&self) -> &Url {
        &self.base
    }

    pub fn project(&self, project: String) -> ProjectBuilder {
        ProjectBuilder {
            client: self,
            project,
        }
    }

    fn authenticated_request(&self, method: Method, url: Url) -> RequestBuilder {
        self.client
            .request(method, url)
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
        let data = Self::send_with_error(self.authenticated_request(Method::GET, url))
            .await?
            .text()
            .await?;
        quick_xml::de::from_str(&data).map_err(|e| e.into())
    }

    async fn post_request<T: DeserializeOwned + std::fmt::Debug>(&self, url: Url) -> Result<T> {
        let data = Self::send_with_error(self.authenticated_request(Method::POST, url))
            .await?
            .text()
            .await?;
        quick_xml::de::from_str(&data).map_err(|e| e.into())
    }
}
