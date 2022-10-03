use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::io::BufReader;
use std::time::SystemTime;

use http_types::StatusCode;
use serde::{de::DeserializeOwned, Deserialize};
use wiremock::ResponseTemplate;
use wiremock::{Request, Respond};
use xml_builder::XMLElement;

use crate::{
    random_md5, MockBranchOptions, MockEntry, MockPackage, MockPackageOptions, MockProject,
    MockRevision, MockRevisionOptions, MockSourceFile, MockSourceFileKey, ObsMock, ZERO_REV_SRCMD5,
};

use super::*;

fn source_file_not_found(name: &str) -> ApiError {
    ApiError::new(
        StatusCode::NotFound,
        "404".to_owned(),
        format!("{}: no such file", name),
    )
}

fn source_listing_xml(
    package_name: &str,
    package: &MockPackage,
    rev_id: usize,
    rev: &MockRevision,
) -> XMLElement {
    let mut xml = XMLElement::new("directory");
    xml.add_attribute("name", package_name);
    xml.add_attribute("rev", &rev_id.to_string());
    xml.add_attribute(
        "vrev",
        &rev.vrev
            .map_or_else(|| "".to_owned(), |vrev| vrev.to_string()),
    );
    xml.add_attribute("srcmd5", &rev.options.srcmd5);

    for linkinfo in &rev.linkinfo {
        let mut link_xml = XMLElement::new("linkinfo");
        link_xml.add_attribute("project", &linkinfo.project);
        link_xml.add_attribute("package", &linkinfo.package);
        link_xml.add_attribute("baserev", &linkinfo.baserev);
        link_xml.add_attribute("srcmd5", &linkinfo.srcmd5);
        link_xml.add_attribute("xsrcmd5", &linkinfo.xsrcmd5);
        link_xml.add_attribute("lsrcmd5", &linkinfo.lsrcmd5);

        if linkinfo.missingok {
            link_xml.add_attribute("missingok", "1");
        }

        xml.add_child(link_xml).unwrap();
    }

    for (path, entry) in &rev.entries {
        let contents = package
            .files
            .get(&MockSourceFileKey::borrowed(path, &entry.md5))
            .unwrap();

        let mut entry_xml = XMLElement::new("entry");
        entry_xml.add_attribute("name", path);
        entry_xml.add_attribute("md5", &entry.md5);
        entry_xml.add_attribute("size", &contents.len().to_string());
        entry_xml.add_attribute("mtime", &seconds_since_epoch(&entry.mtime).to_string());

        xml.add_child(entry_xml).unwrap();
    }

    xml
}

fn parse_xml_request<T: DeserializeOwned>(request: &Request) -> Result<T, ApiError> {
    quick_xml::de::from_reader(BufReader::new(&request.body[..]))
        .map_err(|e| ApiError::new(StatusCode::BadRequest, "400".to_string(), e.to_string()))
}

pub(crate) struct ProjectListingResponder {
    mock: ObsMock,
}

impl ProjectListingResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

impl Respond for ProjectListingResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().write().unwrap();
        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        let mut xml = XMLElement::new("directory");
        xml.add_attribute("count", &project.packages.len().to_string());

        for package_name in project.packages.keys() {
            let mut entry_xml = XMLElement::new("entry");
            entry_xml.add_attribute("name", package_name);

            xml.add_child(entry_xml).unwrap();
        }

        ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
    }
}

pub(crate) struct ProjectDeleteResponder {
    mock: ObsMock,
}

impl ProjectDeleteResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

impl Respond for ProjectDeleteResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let project_name = components.nth_back(0).unwrap();

        let mut projects = self.mock.projects().write().unwrap();

        match projects.remove(project_name) {
            Some(_) => ResponseTemplate::new(StatusCode::Ok)
                .set_body_xml(build_status_xml("ok", Some("Ok".to_owned()))),
            None => unknown_project(project_name.to_owned()).into_response(),
        }
    }
}

pub(crate) struct ProjectMetaResponder {
    mock: ObsMock,
}

impl ProjectMetaResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

impl Respond for ProjectMetaResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut components = request.url.path_segments().unwrap();
        let project_name = components.nth_back(1).unwrap();

        let projects = self.mock.projects().read().unwrap();
        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        let mut xml = XMLElement::new("project");
        xml.add_attribute("name", project_name);

        for (repo, arches) in &project.repos {
            let mut repository_xml = XMLElement::new("repository");
            repository_xml.add_attribute("name", repo);
            repository_xml.add_attribute("rebuild", &project.rebuild.to_string());
            repository_xml.add_attribute("block", &project.block.to_string());

            let mut path_xml = XMLElement::new("path");
            path_xml.add_attribute("project", project_name);
            path_xml.add_attribute("repository", repo);

            repository_xml.add_child(path_xml).unwrap();

            for arch in arches.keys() {
                let mut arch_xml = XMLElement::new("arch");
                arch_xml.add_text(arch.clone()).unwrap();

                repository_xml.add_child(arch_xml).unwrap();
            }

            xml.add_child(repository_xml).unwrap();
        }

        ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
    }
}

pub(crate) struct PackageSourceHistoryResponder {
    mock: ObsMock,
}

impl PackageSourceHistoryResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

impl Respond for PackageSourceHistoryResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(1).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();
        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        let package = try_api!(project
            .packages
            .get(package_name)
            .ok_or_else(|| unknown_package(package_name.to_owned())));

        let mut xml = XMLElement::new("revisionlist");
        for (rev_id, revision) in package.revisions.iter().enumerate() {
            // SAFETY: non-meta revisions should always have `vrev` set,
            // otherwise it's a bug.
            let vrev = revision.vrev.unwrap();

            let mut revision_xml = XMLElement::new("revision");
            revision_xml.add_attribute("rev", &(rev_id + 1).to_string());
            revision_xml.add_attribute("vrev", &vrev.to_string());

            let mut srcmd5_xml = XMLElement::new("srcmd5");
            srcmd5_xml
                .add_text(revision.options.srcmd5.clone())
                .unwrap();
            revision_xml.add_child(srcmd5_xml).unwrap();

            let mut version_xml = XMLElement::new("version");
            version_xml
                .add_text(
                    revision
                        .options
                        .version
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned()),
                )
                .unwrap();
            revision_xml.add_child(version_xml).unwrap();

            let mut time_xml = XMLElement::new("time");
            time_xml
                .add_text(seconds_since_epoch(&revision.options.time).to_string())
                .unwrap();
            revision_xml.add_child(time_xml).unwrap();

            let mut user_xml = XMLElement::new("user");
            user_xml.add_text(revision.options.user.clone()).unwrap();
            revision_xml.add_child(user_xml).unwrap();

            if let Some(comment) = &revision.options.comment {
                let mut comment_xml = XMLElement::new("comment");
                comment_xml.add_text(comment.clone()).unwrap();
                revision_xml.add_child(comment_xml).unwrap();
            }

            xml.add_child(revision_xml).unwrap();
        }

        ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
    }
}

pub(crate) struct PackageSourceListingResponder {
    mock: ObsMock,
}

impl PackageSourceListingResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

impl Respond for PackageSourceListingResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();
        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        let package = try_api!(project
            .packages
            .get(package_name)
            .ok_or_else(|| unknown_package(package_name.to_owned())));

        let list_meta = match find_query_param(request, "meta").as_deref() {
            Some("1") => true,
            None | Some("0") => false,
            Some(_) => {
                return ApiError::new(
                    StatusCode::BadRequest,
                    "400".to_owned(),
                    "not boolean".to_owned(),
                )
                .into_response()
            }
        };

        let revisions = if list_meta {
            &package.meta_revisions
        } else {
            &package.revisions
        };

        let rev_id = if let Some(rev_arg) = find_query_param(request, "rev") {
            let index: usize = try_api!(rev_arg.parse().map_err(|_| ApiError::new(
                StatusCode::BadRequest,
                "400".to_owned(),
                format!("bad revision '{}'", rev_arg)
            )));
            ensure!(
                index <= package.revisions.len() && (index > 0 || !list_meta),
                ApiError::new(
                    StatusCode::BadRequest,
                    "400".to_owned(),
                    "no such revision".to_owned(),
                )
            );

            index
        } else {
            revisions.len()
        };

        if rev_id == 0 {
            assert!(!list_meta);

            // OBS seems to have this weird zero revision that always has
            // the same md5 but no contents, so we just handle it in here.
            let mut xml = XMLElement::new("directory");
            xml.add_attribute("name", package_name);
            xml.add_attribute("srcmd5", ZERO_REV_SRCMD5);

            return ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml);
        }

        // -1 to skip the zero revision (see above).
        let rev = &revisions[rev_id - 1];
        ResponseTemplate::new(StatusCode::Ok).set_body_xml(source_listing_xml(
            package_name,
            package,
            rev_id,
            rev,
        ))
    }
}

pub(crate) struct PackageSourceFileResponder {
    mock: ObsMock,
}

impl PackageSourceFileResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

impl Respond for PackageSourceFileResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let file_name = components.nth_back(0).unwrap();
        let package_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();
        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        let package = try_api!(project
            .packages
            .get(package_name)
            .ok_or_else(|| unknown_package(package_name.to_owned())));

        if file_name == "_meta" {
            let entry = package
                .meta_revisions
                .last()
                .unwrap()
                .entries
                .get(MockSourceFile::META_PATH)
                .unwrap();
            let meta = package
                .files
                .get(&MockSourceFileKey::borrowed(
                    MockSourceFile::META_PATH,
                    &entry.md5,
                ))
                .unwrap();
            ResponseTemplate::new(200).set_body_raw(meta.clone(), "application/xml")
        } else {
            match package.revisions.last() {
                Some(rev) => {
                    let entry = try_api!(rev
                        .entries
                        .get(file_name)
                        .ok_or_else(|| source_file_not_found(file_name)));
                    let contents = package
                        .files
                        .get(&MockSourceFileKey::borrowed(file_name, &entry.md5))
                        .unwrap();
                    ResponseTemplate::new(200)
                        .set_body_raw(contents.clone(), "application/octet-stream")
                }
                None => source_file_not_found(file_name).into_response(),
            }
        }
    }
}

#[derive(Deserialize)]
struct DirectoryRequestEntry {
    name: String,
    md5: String,
}

#[derive(Deserialize)]
struct DirectoryRequest {
    #[serde(rename = "entry")]
    entries: Vec<DirectoryRequestEntry>,
}

pub(crate) struct PackageSourcePlacementResponder {
    mock: ObsMock,
}

impl PackageSourcePlacementResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

impl Respond for PackageSourcePlacementResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let file_name = components.nth_back(0).unwrap();
        let package_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let rev = find_query_param(request, "rev");

        let mut projects = self.mock.projects().write().unwrap();
        let project = try_api!(projects
            .get_mut(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        if file_name == "_meta" {
            // TODO: parse file, return errors if attributes don't match (the
            // API crate doesn't add these at all, so leaving this out for now
            // is relatively low-risk)

            project
                .packages
                .entry(package_name.to_owned())
                .or_insert_with(|| {
                    MockPackage::new_with_metadata(
                        project_name,
                        package_name,
                        MockPackageOptions {
                            meta_srcmd5: random_md5(),
                            time: SystemTime::now(),
                            user: self.mock.auth().username().to_owned(),
                            ..Default::default()
                        },
                    )
                });

            ResponseTemplate::new(StatusCode::Ok)
                .set_body_xml(build_status_xml("ok", Some("Ok".to_owned())))
        } else {
            let package = try_api!(project
                .packages
                .get_mut(package_name)
                .ok_or_else(|| unknown_package(package_name.to_owned())));

            if matches!(rev.as_ref().map(AsRef::as_ref), Some("repository")) {
                let file = MockSourceFile {
                    path: file_name.to_owned(),
                    contents: request.body.clone(),
                };
                let (key, contents) = file.into_key_and_contents();
                package.files.insert(key, contents);

                let mut xml = XMLElement::new("revision");
                xml.add_attribute("rev", "repository");

                let mut srcmd5_xml = XMLElement::new("srcmd5");
                srcmd5_xml.add_text(random_md5()).unwrap();

                xml.add_child(srcmd5_xml).unwrap();

                ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
            } else {
                ApiError::new(
                    StatusCode::MisdirectedRequest,
                    "unsupported".to_string(),
                    "Operation not supported by the OBS mock server".to_owned(),
                )
                .into_response()
            }
        }
    }
}

pub(crate) struct PackageSourceCommandResponder {
    mock: ObsMock,
}

impl PackageSourceCommandResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

fn do_commit(
    request: &Request,
    project_name: &str,
    package_name: &str,
    comment: Option<Cow<'_, str>>,
    mock: &ObsMock,
    projects: &mut HashMap<String, MockProject>,
) -> ResponseTemplate {
    let project = try_api!(projects
        .get_mut(project_name)
        .ok_or_else(|| unknown_project(project_name.to_owned())));

    let package = try_api!(project
        .packages
        .get_mut(package_name)
        .ok_or_else(|| unknown_package(package_name.to_owned())));

    let time = SystemTime::now();

    let mut entries = HashMap::new();

    let filelist: DirectoryRequest = try_api!(parse_xml_request(request));
    let mut missing = Vec::new();

    for req_entry in filelist.entries {
        let key = MockSourceFileKey::borrowed(&req_entry.name, &req_entry.md5);
        if package.files.get(&key).is_some() {
            entries.insert(
                key.path.into_owned(),
                MockEntry {
                    md5: key.md5.into_owned(),
                    mtime: time,
                },
            );
        } else {
            missing.push(req_entry);
        }
    }

    if !missing.is_empty() {
        let mut xml = XMLElement::new("directory");
        xml.add_attribute("name", package_name);
        xml.add_attribute("error", "missing");

        for req_entry in missing {
            let mut entry_xml = XMLElement::new("entry");
            entry_xml.add_attribute("name", &req_entry.name);
            entry_xml.add_attribute("md5", &req_entry.md5);

            xml.add_child(entry_xml).unwrap();
        }

        return ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml);
    }

    let options = MockRevisionOptions {
        srcmd5: random_md5(),
        // TODO: detect the source package version
        version: None,
        time,
        user: mock.auth().username().to_owned(),
        comment: comment.map(|c| c.into_owned()),
    };
    package.add_revision(options, entries);

    let rev_id = package.revisions.len();
    let rev = package.revisions.last().unwrap();
    ResponseTemplate::new(StatusCode::Ok).set_body_xml(source_listing_xml(
        package_name,
        package,
        rev_id,
        rev,
    ))
}

fn branch_data_xml(name: &str, value: String) -> XMLElement {
    let mut xml = XMLElement::new("data");
    xml.add_attribute("name", name);
    xml.add_text(value).unwrap();
    xml
}

fn project_meta_enum_error() -> ApiError {
    ApiError::new(
        StatusCode::BadRequest,
        "invalid_argument".to_owned(),
        "Internal Server Error".to_owned(),
    )
}

fn parse_project_meta_enum_param<T>(request: &Request, name: &str) -> Result<Option<T>, ApiError>
where
    for<'a> T: TryFrom<&'a str>,
{
    find_query_param(request, name)
        .map(|v| v.as_ref().try_into().map_err(|_| project_meta_enum_error()))
        .transpose()
}

fn do_branch(
    request: &Request,
    origin_project_name: &str,
    origin_package_name: &str,
    comment: Option<Cow<'_, str>>,
    mock: &ObsMock,
    projects: &mut HashMap<String, MockProject>,
) -> ResponseTemplate {
    let target_project_name = find_query_param(request, "target_project").unwrap_or_else(|| {
        Cow::Owned(format!(
            "home:{}:branches:{}",
            mock.auth().username(),
            origin_project_name
        ))
    });
    let target_package_name =
        find_query_param(request, "target_package").unwrap_or(Cow::Borrowed(origin_package_name));
    let force = find_query_param(request, "force").is_some();
    let missingok = find_query_param(request, "missingok").is_some();

    let rebuild = try_api!(parse_project_meta_enum_param(
        request,
        "add_repositories_rebuild"
    ))
    .unwrap_or_default();
    let block = try_api!(parse_project_meta_enum_param(
        request,
        "add_repositories_block"
    ))
    .unwrap_or_default();

    let origin = projects.get_mut(origin_project_name);
    ensure!(
        origin.is_some() || missingok,
        unknown_project(origin_project_name.to_owned())
    );

    let origin_repos = origin
        .as_ref()
        .map_or_else(HashMap::new, |origin| origin.repos.clone());
    let origin_package = origin.and_then(|project| project.packages.get_mut(origin_package_name));

    match (origin_package.is_some(), missingok) {
        // Package exists, missingok=true
        (true, true) => {
            return ApiError::new(
                StatusCode::BadRequest,
                "not_missing".to_owned(),
                format!(
                    "Branch call with missingok parameter but branched source ({}/{}) exists.",
                    origin_project_name, origin_package_name
                ),
            )
            .into_response();
        }
        // Package does not exist, missingok=false
        (false, false) => {
            return unknown_package(origin_package_name.to_owned()).into_response();
        }
        _ => {}
    }

    let target_package = MockPackage::new_branched(
        origin_project_name.to_owned(),
        origin_package_name.to_owned(),
        origin_package.as_deref(),
        &target_project_name,
        &target_package_name,
        MockBranchOptions {
            srcmd5: random_md5(),
            xsrcmd5: random_md5(),
            user: mock.auth().username().to_owned(),
            time: SystemTime::now(),
            comment: comment.map(Cow::into_owned),
            missingok,
        },
    );

    let target_project = projects
        .entry(target_project_name.clone().into_owned())
        .or_insert_with(|| MockProject {
            repos: origin_repos,
            rebuild,
            block,
            ..Default::default()
        });

    ensure!(
        force
            || !target_project
                .packages
                .contains_key(target_package_name.as_ref()),
        ApiError::new(
            StatusCode::BadRequest,
            "double_branch_package".to_owned(),
            format!(
                "branch target package already exists: {}/{}",
                target_project_name, target_package_name
            )
        )
    );

    target_project
        .packages
        .insert(target_package_name.to_string(), target_package);

    let mut xml = build_status_xml("ok", Some("Ok".to_owned()));

    xml.add_child(branch_data_xml(
        "targetproject",
        target_project_name.into_owned(),
    ))
    .unwrap();
    xml.add_child(branch_data_xml(
        "targetpackage",
        target_package_name.into_owned(),
    ))
    .unwrap();
    xml.add_child(branch_data_xml(
        "sourceproject",
        origin_project_name.to_owned(),
    ))
    .unwrap();
    xml.add_child(branch_data_xml(
        "sourcepackage",
        origin_package_name.to_owned(),
    ))
    .unwrap();

    ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
}

impl Respond for PackageSourceCommandResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let mut projects = self.mock.projects().write().unwrap();

        let cmd = try_api!(
            find_query_param(request, "cmd").ok_or_else(|| ApiError::new(
                StatusCode::BadRequest,
                "missing_parameter".to_string(),
                "POST request without given cmd parameter".to_string()
            ))
        );

        let comment = find_query_param(request, "comment");

        match cmd.as_ref() {
            "commitfilelist" => do_commit(
                request,
                project_name,
                package_name,
                comment,
                &self.mock,
                &mut *projects,
            ),
            "branch" => do_branch(
                request,
                project_name,
                package_name,
                comment,
                &self.mock,
                &mut *projects,
            ),
            _ => ApiError::new(
                StatusCode::NotFound,
                "illegal_request".to_string(),
                "invalid_command".to_string(),
            )
            .into_response(),
        }
    }
}

pub struct PackageSourceDeleteResponder {
    mock: ObsMock,
}

impl PackageSourceDeleteResponder {
    pub fn new(mock: ObsMock) -> Self {
        PackageSourceDeleteResponder { mock }
    }
}

impl Respond for PackageSourceDeleteResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let mut projects = self.mock.projects().write().unwrap();
        let project = try_api!(projects
            .get_mut(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        ensure!(
            project.packages.remove(package_name).is_some(),
            unknown_package(package_name.to_owned())
        );

        for arches in project.repos.values_mut() {
            for repo in arches.values_mut() {
                repo.packages.remove(package_name);
            }
        }

        ResponseTemplate::new(StatusCode::Ok)
            .set_body_xml(build_status_xml("ok", Some("Ok".to_owned())))
    }
}
