use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

use api::{
    ArchListingResponder, BuildBinaryFileResponder, BuildBinaryListResponder,
    BuildHistoryResponder, BuildLogResponder, BuildPackageStatusResponder, BuildResultsResponder,
    PackageSourceCommandResponder, PackageSourceDeleteResponder, PackageSourceFileResponder,
    PackageSourceHistoryResponder, PackageSourceListingResponder, PackageSourcePlacementResponder,
    ProjectBuildCommandResponder, ProjectDeleteResponder, ProjectListingResponder,
    ProjectMetaResponder, RepoListingResponder,
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

// MD5 of the empty string, used as the srcmd5 of the "zero revision".
pub const ZERO_REV_SRCMD5: &str = "d41d8cd98f00b204e9800998ecf8427e";

pub fn random_md5() -> String {
    let md5bytes: [u8; 16] = rand::random();
    base16ct::lower::encode_string(&md5bytes)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MockSourceFileKey<'path, 'md5> {
    pub path: Cow<'path, str>,
    pub md5: Cow<'md5, str>,
}

impl<'path, 'md5> MockSourceFileKey<'path, 'md5> {
    pub fn borrowed(path: &'path str, md5: &'md5 str) -> Self {
        MockSourceFileKey {
            path: Cow::Borrowed(path),
            md5: Cow::Borrowed(md5),
        }
    }

    pub fn into_owned(self) -> MockSourceFileKey<'static, 'static> {
        MockSourceFileKey {
            path: Cow::Owned(self.path.into_owned()),
            md5: Cow::Owned(self.md5.into_owned()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MockSourceFile {
    pub path: String,
    pub contents: Vec<u8>,
}

impl MockSourceFile {
    const META_PATH: &'static str = "_meta";

    pub fn new_metadata(
        project: &str,
        package: &str,
        disabled: &[MockPackageDisabledBuild],
    ) -> MockSourceFile {
        let mut xml = XMLElement::new("package");
        xml.add_attribute("project", project);
        xml.add_attribute("name", package);

        xml.add_child(XMLElement::new("title")).unwrap();
        xml.add_child(XMLElement::new("description")).unwrap();

        if !disabled.is_empty() {
            let mut build_xml = XMLElement::new("build");

            for disabled_build in disabled {
                let mut disable_xml = XMLElement::new("disable");

                if let Some(repository) = &disabled_build.repository {
                    disable_xml.add_attribute("repository", repository);
                }

                if let Some(arch) = &disabled_build.arch {
                    disable_xml.add_attribute("arch", arch);
                }

                build_xml.add_child(disable_xml).unwrap();
            }

            xml.add_child(build_xml).unwrap();
        }

        let mut contents = vec![];
        xml.render(&mut contents, false, true).unwrap();

        MockSourceFile {
            path: MockSourceFile::META_PATH.to_owned(),
            contents,
        }
    }

    fn md5(&self) -> String {
        base16ct::lower::encode_string(&Md5::digest(&self.contents))
    }

    fn into_key_and_contents(self) -> (MockSourceFileKey<'static, 'static>, Vec<u8>) {
        (
            MockSourceFileKey {
                md5: Cow::Owned(self.md5()),
                path: Cow::Owned(self.path),
            },
            self.contents,
        )
    }
}

#[derive(Debug, Clone)]
struct MockLinkInfo {
    project: String,
    package: String,
    baserev: String,
    srcmd5: String,
    lsrcmd5: String,
    xsrcmd5: String,
    missingok: bool,
}

#[derive(Debug, Clone)]
pub struct MockEntry {
    pub md5: String,
    pub mtime: SystemTime,
}

impl MockEntry {
    pub fn from_key(key: &MockSourceFileKey, mtime: SystemTime) -> MockEntry {
        MockEntry {
            md5: key.md5.clone().into_owned(),
            mtime,
        }
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
}

impl Default for MockRevisionOptions {
    fn default() -> Self {
        Self {
            srcmd5: random_md5(),
            version: None,
            time: SystemTime::now(),
            user: ADMIN_USER.to_owned(),
            comment: None,
        }
    }
}

#[derive(Debug, Clone)]
struct MockRevision {
    vrev: Option<usize>,
    linkinfo: Vec<MockLinkInfo>,
    options: MockRevisionOptions,
    entries: HashMap<String, MockEntry>,
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

#[derive(Clone, Debug, Default)]
pub struct MockPackageDisabledBuild {
    pub repository: Option<String>,
    pub arch: Option<String>,
}

pub struct MockPackageOptions {
    pub meta_srcmd5: String,
    pub time: SystemTime,
    pub user: String,
    pub disabled: Vec<MockPackageDisabledBuild>,
}

impl Default for MockPackageOptions {
    fn default() -> Self {
        Self {
            meta_srcmd5: random_md5(),
            time: SystemTime::now(),
            user: ADMIN_USER.to_owned(),
            disabled: Vec::new(),
        }
    }
}

struct MockPackage {
    files: HashMap<MockSourceFileKey<'static, 'static>, Vec<u8>>,
    revisions: Vec<MockRevision>,
    meta_revisions: Vec<MockRevision>,
    latest_vrevs: HashMap<Option<String>, usize>,
    disabled: Vec<MockPackageDisabledBuild>,
}

impl MockPackage {
    fn new_with_metadata(
        project_name: &str,
        package_name: &str,
        options: MockPackageOptions,
    ) -> MockPackage {
        let (meta_key, meta_contents) =
            MockSourceFile::new_metadata(project_name, package_name, &options.disabled)
                .into_key_and_contents();
        let meta_entry = MockEntry::from_key(&meta_key, options.time);
        MockPackage {
            files: [(meta_key, meta_contents)].into(),
            revisions: Vec::new(),
            meta_revisions: vec![MockRevision {
                vrev: None,
                options: MockRevisionOptions {
                    srcmd5: options.meta_srcmd5,
                    version: None,
                    time: options.time,
                    user: options.user,
                    comment: None,
                },
                entries: [(MockSourceFile::META_PATH.to_owned(), meta_entry)].into(),
                linkinfo: vec![],
            }],
            latest_vrevs: HashMap::new(),
            disabled: options.disabled,
        }
    }

    pub fn new_branched(
        origin_project_name: String,
        origin_package_name: String,
        origin: Option<&MockPackage>,
        branched_project_name: &str,
        branched_package_name: &str,
        options: MockBranchOptions,
    ) -> MockPackage {
        let (mut files, entries, origin_srcmd5, disabled) = if let Some((origin, origin_rev)) =
            origin.and_then(|origin| origin.revisions.last().map(|rev| (origin, rev)))
        {
            (
                origin.files.clone(),
                origin_rev.entries.clone(),
                origin_rev.options.srcmd5.clone(),
                origin.disabled.clone(),
            )
        } else {
            (
                HashMap::new(),
                HashMap::new(),
                ZERO_REV_SRCMD5.to_owned(),
                Vec::new(),
            )
        };

        let (meta_key, meta_contents) =
            MockSourceFile::new_metadata(branched_project_name, branched_package_name, &disabled)
                .into_key_and_contents();
        let meta_entry = MockEntry::from_key(&meta_key, options.time);
        files.insert(meta_key, meta_contents);

        let linkinfo = MockLinkInfo {
            project: origin_project_name,
            package: origin_package_name,
            baserev: origin_srcmd5.clone(),
            srcmd5: origin_srcmd5,
            xsrcmd5: options.xsrcmd5,
            lsrcmd5: options.srcmd5.clone(),
            missingok: options.missingok,
        };

        let mut latest_vrevs = HashMap::new();
        latest_vrevs.insert(None, 1);

        MockPackage {
            files,
            revisions: vec![MockRevision {
                vrev: Some(1),
                options: MockRevisionOptions {
                    srcmd5: options.srcmd5,
                    version: None,
                    time: options.time,
                    user: options.user.clone(),
                    comment: options.comment.clone(),
                },
                linkinfo: vec![linkinfo],
                entries,
            }],
            meta_revisions: vec![MockRevision {
                vrev: None,
                options: MockRevisionOptions {
                    srcmd5: random_md5(),
                    version: None,
                    time: options.time,
                    user: options.user,
                    comment: options.comment,
                },
                linkinfo: vec![],
                entries: [(MockSourceFile::META_PATH.to_owned(), meta_entry)].into(),
            }],
            latest_vrevs,
            disabled,
        }
    }

    fn add_revision(&mut self, options: MockRevisionOptions, entries: HashMap<String, MockEntry>) {
        let vrev = self
            .latest_vrevs
            .entry(options.version.clone())
            .or_default();
        *vrev += 1;

        for (path, entry) in &entries {
            assert!(self
                .files
                .contains_key(&MockSourceFileKey::borrowed(path, &entry.md5)));
        }

        self.revisions.push(MockRevision {
            vrev: Some(*vrev),
            options,
            entries,
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
    pub missingok: bool,
}

impl Default for MockBranchOptions {
    fn default() -> Self {
        Self {
            srcmd5: random_md5(),
            xsrcmd5: random_md5(),
            time: SystemTime::now(),
            user: ADMIN_USER.to_owned(),
            comment: None,
            missingok: false,
        }
    }
}

type ArchMap<Value> = HashMap<String, Value>;

#[derive(Clone, Default)]
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
pub struct MockBinary {
    pub contents: Vec<u8>,
    pub mtime: SystemTime,
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

#[derive(Clone)]
pub struct MockBuildHistoryEntry {
    pub rev: String,
    pub srcmd5: String,
    pub versrel: String,
    pub bcnt: u32,
    pub time: SystemTime,
    pub duration: Duration,
}

impl Default for MockBuildHistoryEntry {
    fn default() -> Self {
        Self {
            rev: "1".to_owned(),
            srcmd5: random_md5(),
            versrel: "0".to_owned(),
            bcnt: 1,
            time: SystemTime::now(),
            duration: Duration::ZERO,
        }
    }
}

#[derive(Clone, Default)]
struct MockRepositoryPackage {
    status: MockBuildStatus,

    binaries: HashMap<String, MockBinary>,

    latest_log: Option<MockBuildLog>,
    latest_successful_log: Option<MockBuildLog>,

    history: Vec<MockBuildHistoryEntry>,
}

#[derive(Clone)]
struct MockRepository {
    code: MockRepositoryCode,
    packages: HashMap<String, MockRepositoryPackage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum MockRebuildMode {
    Transitive,
    Direct,
    Local,
}

impl Default for MockRebuildMode {
    fn default() -> Self {
        MockRebuildMode::Transitive
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum MockBlockMode {
    All,
    Local,
    Never,
}

impl Default for MockBlockMode {
    fn default() -> Self {
        MockBlockMode::All
    }
}

#[derive(Default)]
struct MockProject {
    packages: HashMap<String, MockPackage>,
    repos: HashMap<String, ArchMap<MockRepository>>,

    rebuild: MockRebuildMode,
    block: MockBlockMode,

    rebuild_status: MockBuildStatus,
}

type ProjectMap = HashMap<String, MockProject>;

fn get_project<'p, 'n>(projects: &'p mut ProjectMap, name: &'n str) -> &'p mut MockProject {
    projects
        .get_mut(name)
        .unwrap_or_else(|| panic!("Unknown project: {}", name))
}

fn get_package<'p, 'n>(project: &'p mut MockProject, name: &'n str) -> &'p mut MockPackage {
    project
        .packages
        .get_mut(name)
        .unwrap_or_else(|| panic!("Unknown package: {}", name))
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
            .and(path_regex("^/source/[^/]+$"))
            .respond_with(ProjectListingResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("DELETE"))
            .and(path_regex("^/source/[^/]+$"))
            .respond_with(ProjectDeleteResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/source/[^/]+/_meta$"))
            .respond_with(ProjectMetaResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

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

        Mock::given(method("DELETE"))
            .and(path_regex("^/source/[^/]+/[^/]+$"))
            .respond_with(PackageSourceDeleteResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/source/[^/]+/[^/]+/_history$"))
            .respond_with(PackageSourceHistoryResponder::new(server.clone()))
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

        Mock::given(method("POST"))
            .and(path_regex("^/build/[^/]+$"))
            .respond_with(ProjectBuildCommandResponder::new(server.clone()))
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
            .and(path_regex("^/build/[^/]+/[^/]+/[^/]+/[^/]+$"))
            .respond_with(BuildBinaryListResponder::new(server.clone()))
            .mount(&server.inner.server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/build/[^/]+/[^/]+/[^/]+/[^/]+/_history$"))
            .respond_with(BuildHistoryResponder::new(server.clone()))
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

        Mock::given(method("GET"))
            .and(path_regex("^/build/[^/]+/[^/]+/[^/]+/[^/]+/[^/]+$"))
            .respond_with(BuildBinaryFileResponder::new(server.clone()))
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

    pub fn set_project_modes(
        &self,
        project_name: &str,
        rebuild: MockRebuildMode,
        block: MockBlockMode,
    ) {
        let mut projects = self.inner.projects.write().unwrap();
        let project = get_project(&mut *projects, project_name);
        project.rebuild = rebuild;
        project.block = block;
    }

    pub fn add_new_package(
        &self,
        project_name: &str,
        package_name: String,
        options: MockPackageOptions,
    ) {
        let mut projects = self.inner.projects.write().unwrap();
        let project = get_project(&mut *projects, project_name);
        let package = MockPackage::new_with_metadata(project_name, &package_name, options);
        project.packages.insert(package_name, package);
    }

    pub fn set_package_metadata(
        &self,
        project_name: &str,
        package_name: &str,
        options: MockPackageOptions,
    ) {
        let mut projects = self.inner.projects.write().unwrap();
        let project = projects
            .get_mut(project_name)
            .unwrap_or_else(|| panic!("Unknown project: {}", project_name));
        let package = get_package(project, package_name);

        let meta = MockSourceFile::new_metadata(project_name, package_name, &options.disabled);
        let (key, contents) = meta.into_key_and_contents();
        package.files.insert(key.clone(), contents);

        let meta_entry = MockEntry::from_key(&key, options.time);

        package.meta_revisions.push(MockRevision {
            vrev: None,
            options: MockRevisionOptions {
                srcmd5: options.meta_srcmd5,
                version: None,
                time: options.time,
                user: options.user,
                comment: None,
            },
            entries: [(MockSourceFile::META_PATH.to_owned(), meta_entry)].into(),
            linkinfo: vec![],
        });
    }

    pub fn add_package_files(
        &self,
        project_name: &str,
        package_name: &str,
        file: MockSourceFile,
    ) -> MockSourceFileKey {
        let mut projects = self.inner.projects.write().unwrap();
        let project = projects
            .get_mut(project_name)
            .unwrap_or_else(|| panic!("Unknown project: {}", project_name));
        let package = get_package(project, package_name);

        let (key, contents) = file.into_key_and_contents();
        package.files.insert(key.clone(), contents);
        key
    }

    pub fn add_package_revision(
        &self,
        project_name: &str,
        package_name: &str,
        options: MockRevisionOptions,
        entries: HashMap<String, MockEntry>,
    ) {
        let mut projects = self.inner.projects.write().unwrap();
        let project = get_project(&mut *projects, project_name);
        let package = get_package(project, package_name);
        package.add_revision(options, entries);
    }

    pub fn branch(
        &self,
        origin_project_name: String,
        origin_package_name: String,
        branched_project_name: &str,
        branched_package_name: String,
        options: MockBranchOptions,
    ) {
        let mut projects = self.inner.projects.write().unwrap();
        let origin = get_package(
            get_project(&mut *projects, &origin_project_name),
            &origin_package_name,
        );

        let package = MockPackage::new_branched(
            origin_project_name,
            origin_package_name,
            Some(origin),
            branched_project_name,
            &branched_package_name,
            options,
        );

        let project = get_project(&mut *projects, branched_project_name);
        project.packages.insert(branched_package_name, package);
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

        // Make sure the source package exists.
        assert!(
            project.packages.contains_key(&package_name),
            "Unknown package: {}",
            package_name
        );

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

    pub fn set_package_build_status_for_rebuilds(
        &self,
        project_name: &str,
        status: MockBuildStatus,
    ) {
        let mut projects = self.inner.projects.write().unwrap();
        let project = get_project(&mut *projects, project_name);
        project.rebuild_status = status;
    }

    pub fn set_package_binaries(
        &self,
        project_name: &str,
        repo_name: &str,
        arch: &str,
        package_name: String,
        binaries: HashMap<String, MockBinary>,
    ) {
        self.with_repo_package(project_name, repo_name, arch, package_name, |package| {
            package.binaries = binaries;
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

    pub fn add_build_history(
        &self,
        project_name: &str,
        repo_name: &str,
        arch: &str,
        package_name: String,
        entry: MockBuildHistoryEntry,
    ) {
        self.with_repo_package(project_name, repo_name, arch, package_name, |package| {
            package.history.push(entry);
        });
    }
}
