use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::SystemTime,
};

use api::{
    ArchListingResponder, BuildLogResponder, BuildPackageStatusResponder, BuildResultsResponder,
    PackageSourceCommandResponder, PackageSourceFileResponder, PackageSourceListingResponder,
    PackageSourcePlacementResponder, RepoListingResponder,
};

use http_types::auth::BasicAuth;
use md5::{Digest, Md5};
use strum_macros::{Display, EnumString};
use wiremock::{
    http::Url,
    matchers::{method, path_regex},
    Mock, MockServer,
};
use xml_builder::XMLElement;

mod api;

pub const ADMIN_USER: &str = "Admin";

pub fn random_md5() -> String {
    let md5bytes: [u8; 16] = rand::random();
    base16ct::lower::encode_string(&md5bytes)
}

#[derive(Debug, Clone)]
struct MockLinkInfo {
    project: String,
    package: String,
    baserev: String,
    srcmd5: String,
    lsrcmd5: String,
    xsrcmd5: String,
}

#[derive(Debug, Clone)]
pub struct MockEntry {
    md5: String,
    mtime: SystemTime,
    contents: Vec<u8>,
}

impl MockEntry {
    const META_NAME: &'static str = "_meta";

    pub fn new_with_contents(mtime: SystemTime, contents: Vec<u8>) -> MockEntry {
        let md5 = base16ct::lower::encode_string(&Md5::digest(&contents));
        MockEntry {
            mtime,
            contents,
            md5,
        }
    }

    fn new_metadata(project: &str, package: &str, mtime: SystemTime) -> MockEntry {
        let mut xml = XMLElement::new("package");
        xml.add_attribute("name", project);
        xml.add_attribute("project", package);

        xml.add_child(XMLElement::new("title")).unwrap();
        xml.add_child(XMLElement::new("description")).unwrap();

        let mut contents = vec![];
        xml.render(&mut contents, false, true).unwrap();

        MockEntry::new_with_contents(mtime, contents)
    }
}

#[derive(Debug, Clone)]
// Temporarily add this, because there are fields here that are needed for
// revisions in the future but are currently unused.
#[allow(unused)]
pub struct MockRevisionOptions {
    pub srcmd5: String,
    pub version: Option<String>,
    pub time: SystemTime,
    pub user: String,
    pub comment: Option<String>,
    pub entries: HashMap<String, MockEntry>,
}

impl Default for MockRevisionOptions {
    fn default() -> Self {
        Self {
            srcmd5: random_md5(),
            version: None,
            time: SystemTime::now(),
            user: ADMIN_USER.to_owned(),
            entries: HashMap::new(),
            comment: None,
        }
    }
}

#[derive(Debug, Clone)]
struct MockRevision {
    vrev: usize,
    linkinfo: Vec<MockLinkInfo>,
    options: MockRevisionOptions,
}

#[derive(Copy, Clone, Debug, Display, EnumString, Eq, PartialEq)]
#[strum(serialize_all = "snake_case")]
pub enum MockRepositoryCode {
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

#[derive(Copy, Clone, Debug, Display, EnumString, Eq, PartialEq)]
#[strum(serialize_all = "snake_case")]
pub enum MockPackageCode {
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

impl Default for MockPackageCode {
    fn default() -> Self {
        MockPackageCode::Unknown
    }
}

#[derive(Default)]
struct MockPackage {
    revisions: Vec<MockRevision>,
    latest_vrevs: HashMap<Option<String>, usize>,
    pending_rev_entries: HashMap<String, MockEntry>,
}

impl MockPackage {
    fn add_revision(&mut self, options: MockRevisionOptions) {
        let vrev = self
            .latest_vrevs
            .entry(options.version.clone())
            .or_default();
        *vrev += 1;

        self.revisions.push(MockRevision {
            vrev: *vrev,
            options,
            linkinfo: self
                .revisions
                .last()
                .map_or_else(Vec::new, |rev| rev.linkinfo.clone()),
        });
    }
}

pub struct MockBranchOptions {
    pub srcmd5: String,
    pub xsrcmd5: String,
    pub user: String,
    pub time: SystemTime,
    pub comment: Option<String>,
}

impl Default for MockBranchOptions {
    fn default() -> Self {
        Self {
            srcmd5: random_md5(),
            xsrcmd5: random_md5(),
            time: SystemTime::now(),
            user: ADMIN_USER.to_owned(),
            comment: None,
        }
    }
}

type ArchMap<Value> = HashMap<String, Value>;

#[derive(Default)]
pub struct MockBuildStatus {
    pub code: MockPackageCode,
    pub dirty: bool,
}

impl MockBuildStatus {
    pub fn new(code: MockPackageCode) -> Self {
        Self {
            code,
            ..Self::default()
        }
    }
}

#[derive(Clone)]
pub struct MockBuildLog {
    pub contents: String,
    pub mtime: SystemTime,
    pub chunk_size: Option<usize>,
}

impl MockBuildLog {
    pub fn new(contents: String) -> MockBuildLog {
        MockBuildLog {
            contents,
            mtime: SystemTime::now(),
            chunk_size: None,
        }
    }
}

#[derive(Default)]
struct MockRepositoryPackage {
    status: MockBuildStatus,

    latest_log: Option<MockBuildLog>,
    latest_successful_log: Option<MockBuildLog>,
}

struct MockRepository {
    code: MockRepositoryCode,
    packages: HashMap<String, MockRepositoryPackage>,
}

#[derive(Default)]
struct MockProject {
    packages: HashMap<String, MockPackage>,
    repos: HashMap<String, ArchMap<MockRepository>>,
}

type ProjectMap = HashMap<String, MockProject>;

fn get_project<'p, 'n>(projects: &'p mut ProjectMap, name: &'n str) -> &'p mut MockProject {
    projects
        .get_mut(name)
        .unwrap_or_else(|| panic!("Unknown project: {}", name))
}

struct Inner {
    server: MockServer,
    auth: BasicAuth,
    projects: RwLock<ProjectMap>,
}

#[derive(Clone)]
pub struct ObsMock {
    inner: Arc<Inner>,
}

impl ObsMock {
    pub async fn start(username: &str, password: &str) -> Self {
        let inner = Inner {
            auth: BasicAuth::new(username, password),
            server: MockServer::start().await,
            projects: RwLock::new(HashMap::new()),
        };

        let server = Self {
            inner: Arc::new(inner),
        };

        Mock::given(method("GET"))
            .and(path_regex("^/source/[^/]+/[^/]+$"))
            .respond_with(PackageSourceListingResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("POST"))
            .and(path_regex("^/source/[^/]+/[^/]+$"))
            .respond_with(PackageSourceCommandResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/source/[^/]+/[^/]+/[^/]+$"))
            .respond_with(PackageSourceFileResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("PUT"))
            .and(path_regex("^/source/[^/]+/[^/]+/[^/]+$"))
            .respond_with(PackageSourcePlacementResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/build/[^/]+/_result$"))
            .respond_with(BuildResultsResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/build/[^/]+$"))
            .respond_with(RepoListingResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("/build/[^/]+/[^/]+$"))
            .respond_with(ArchListingResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/build/[^/]+/[^/]+/[^/]+/[^/]+/_log$"))
            .respond_with(BuildLogResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/build/[^/]+/[^/]+/[^/]+/[^/]+/_status$"))
            .respond_with(BuildPackageStatusResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        server
    }

    pub fn uri(&self) -> Url {
        self.inner.server.uri().parse().expect("uri is not a Url")
    }

    pub fn auth(&self) -> &BasicAuth {
        &self.inner.auth
    }

    fn projects(&self) -> &RwLock<ProjectMap> {
        &self.inner.projects
    }

    pub fn add_project(&self, project_name: String) {
        let mut projects = self.inner.projects.write().unwrap();
        projects.entry(project_name).or_default();
    }

    pub fn add_package_revision(
        &self,
        project_name: &str,
        package_name: String,
        mut options: MockRevisionOptions,
    ) {
        let meta = MockEntry::new_metadata(&project_name, &package_name, options.time);
        options
            .entries
            .insert(MockEntry::META_NAME.to_owned(), meta);

        let mut projects = self.inner.projects.write().unwrap();
        let project = projects
            .get_mut(project_name)
            .unwrap_or_else(|| panic!("Unknown project: {}", project_name));

        let package = project.packages.entry(package_name).or_default();
        package.add_revision(options);
    }

    pub fn branch(
        &self,
        origin_project_name: String,
        origin_package_name: String,
        branched_project_name: &str,
        branched_package_name: String,
        options: MockBranchOptions,
    ) {
        let meta =
            MockEntry::new_metadata(&branched_project_name, &branched_package_name, options.time);

        let mut projects = self.inner.projects.write().unwrap();

        let origin = get_project(&mut *projects, &origin_project_name)
            .packages
            .get(&origin_package_name)
            .unwrap_or_else(|| {
                panic!(
                    "Unknown package: {}/{}",
                    origin_project_name, origin_package_name
                )
            });
        let origin_rev = origin.revisions.last().unwrap();
        let mut origin_entries = origin_rev.options.entries.clone();
        let origin_srcmd5 = origin_rev.options.srcmd5.clone();

        origin_entries.insert(MockEntry::META_NAME.to_owned(), meta);

        let linkinfo = MockLinkInfo {
            project: origin_project_name,
            package: origin_package_name,
            baserev: origin_srcmd5.clone(),
            srcmd5: origin_srcmd5,
            xsrcmd5: options.xsrcmd5,
            lsrcmd5: options.srcmd5.clone(),
        };

        let mut latest_vrevs = HashMap::new();
        latest_vrevs.insert(None, 1);

        let project = get_project(&mut *projects, branched_project_name);

        project.packages.insert(
            branched_package_name,
            MockPackage {
                revisions: vec![MockRevision {
                    vrev: 1,
                    options: MockRevisionOptions {
                        srcmd5: options.srcmd5,
                        version: None,
                        time: options.time,
                        user: options.user,
                        comment: options.comment,
                        entries: origin_entries,
                    },
                    linkinfo: vec![linkinfo],
                }],
                latest_vrevs,
                pending_rev_entries: HashMap::new(),
            },
        );
    }

    pub fn add_or_update_repository(
        &self,
        project_name: &str,
        repo_name: String,
        arch: String,
        code: MockRepositoryCode,
    ) {
        let mut projects = self.inner.projects.write().unwrap();
        let project = get_project(&mut *projects, project_name);

        project
            .repos
            .entry(repo_name)
            .or_insert_with(HashMap::new)
            .entry(arch)
            .and_modify(|repo| repo.code = code)
            .or_insert_with(|| MockRepository {
                code,
                packages: HashMap::new(),
            });
    }

    fn with_repo_package<R, F: FnOnce(&mut MockRepositoryPackage) -> R>(
        &self,
        project_name: &str,
        repo_name: &str,
        arch: &str,
        package_name: String,
        func: F,
    ) -> R {
        let mut projects = self.inner.projects.write().unwrap();
        let project = get_project(&mut *projects, project_name);

        let package = project
            .repos
            .get_mut(repo_name)
            .unwrap_or_else(|| panic!("Unknown repo: {}/{}", project_name, repo_name))
            .get_mut(arch)
            .unwrap_or_else(|| panic!("Unknown arch: {}/{}/{}", project_name, repo_name, arch))
            .packages
            .entry(package_name)
            .or_default();
        func(package)
    }

    pub fn set_package_build_status(
        &self,
        project_name: &str,
        repo_name: &str,
        arch: &str,
        package_name: String,
        status: MockBuildStatus,
    ) {
        self.with_repo_package(project_name, repo_name, arch, package_name, |package| {
            package.status = status;
        });
    }

    pub fn add_completed_build_log(
        &self,
        project_name: &str,
        repo_name: &str,
        arch: &str,
        package_name: String,
        log: MockBuildLog,
        success: bool,
    ) {
        self.with_repo_package(project_name, repo_name, arch, package_name, |package| {
            if success {
                package.latest_successful_log = Some(log.clone());
            }

            package.latest_log = Some(log);
        });
    }
}
