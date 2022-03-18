use std::borrow::Cow;
use std::collections::HashMap;
use std::time::SystemTime;

use wiremock::ResponseTemplate;
use wiremock::{Request, Respond};
use xml_builder::XMLElement;

use crate::{MockBuildStatus, MockPackageCode, ObsMock};

use super::*;

fn unknown_repo(project: &str, repo: &str) -> ApiError {
    ApiError::new(
        StatusCode::NotFound,
        "404".to_owned(),
        format!("project '{}' has no repository '{}'", project, repo),
    )
}

fn unknown_arch(project: &str, repo: &str, arch: &str) -> ApiError {
    ApiError::new(
        StatusCode::NotFound,
        "404".to_owned(),
        format!(
            "repository '{}/{}' has no architecture '{}'",
            project, repo, arch
        ),
    )
}

fn unknown_package(package: &str) -> ApiError {
    ApiError::new(
        StatusCode::NotFound,
        "404".to_owned(),
        format!("unknown package '{}'", package),
    )
}

fn unknown_parameter(param: &str) -> ApiError {
    ApiError::new(
        StatusCode::BadRequest,
        "400".to_owned(),
        format!("unknown parameter '{}'", param),
    )
}

pub(crate) struct ProjectBuildCommandResponder {
    mock: ObsMock,
}

impl ProjectBuildCommandResponder {
    pub fn new(mock: ObsMock) -> Self {
        ProjectBuildCommandResponder { mock }
    }
}

impl Respond for ProjectBuildCommandResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let components = request.url.path_segments().unwrap();
        let project_name = components.last().unwrap();

        let mut projects = self.mock.projects().write().unwrap();
        let project = try_api!(projects
            .get_mut(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        let cmd = try_api!(
            find_query_param(request, "cmd").ok_or_else(|| ApiError::new(
                StatusCode::BadRequest,
                "missing_parameter".to_string(),
                "Missing parameter 'cmd'".to_string()
            ))
        );

        match cmd.as_ref() {
            "rebuild" => {
                let mut package_names = Vec::new();
                for (key, value) in request.url.query_pairs() {
                    match key.as_ref() {
                        "cmd" => continue,
                        "package" => package_names.push(value.clone().into_owned()),
                        "arch" | "repository" | "code" | "lastbuild" => {
                            return ApiError::new(
                                StatusCode::MisdirectedRequest,
                                "unsupported".to_string(),
                                "Operation not supported by the OBS mock server".to_owned(),
                            )
                            .into_response();
                        }
                        _ => {
                            return unknown_parameter(&key).into_response();
                        }
                    }
                }

                if package_names.is_empty() {
                    package_names.extend(project.packages.keys().cloned());
                }

                let mut packages = HashMap::new();

                for package_name in &package_names {
                    if let Some(package) = project.packages.get(package_name) {
                        packages.insert(package_name, package);
                    } else {
                        // OBS is...strange here, the standard missing package
                        // error is wrapped *as a string* inside of a different
                        // error. Mimic the behavior here.
                        let inner_xml = unknown_package(package_name).into_xml();
                        let mut inner = Vec::new();
                        inner_xml.render(&mut inner, false, true).unwrap();

                        return ApiError::new(
                            StatusCode::NotFound,
                            "not_found".to_owned(),
                            String::from_utf8_lossy(&inner).into_owned(),
                        )
                        .into_response();
                    }
                }

                for (repo_name, arches) in &mut project.repos {
                    for (arch, repo) in arches {
                        for (package_name, package) in &packages {
                            for disabled in &package.disabled {
                                if (disabled.repository.is_none()
                                    || disabled.repository.as_deref() == Some(repo_name))
                                    && (disabled.arch.is_none()
                                        || disabled.arch.as_deref() == Some(arch))
                                {
                                    continue;
                                }
                            }

                            let repo_package =
                                repo.packages.entry((*package_name).clone()).or_default();
                            repo_package.status = project.rebuild_status.clone();
                        }
                    }
                }

                ResponseTemplate::new(StatusCode::Ok).set_body_xml(build_status_xml("ok", None))
            }
            _ => ApiError::new(
                StatusCode::BadRequest,
                "illegal_request".to_owned(),
                format!("unsupported POST command {} to {}", cmd, request.url),
            )
            .into_response(),
        }
    }
}

pub(crate) struct RepoListingResponder {
    mock: ObsMock,
}

impl RepoListingResponder {
    pub fn new(mock: ObsMock) -> Self {
        RepoListingResponder { mock }
    }
}

impl Respond for RepoListingResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let components = request.url.path_segments().unwrap();
        let project_name = components.last().unwrap();

        let projects = self.mock.projects().read().unwrap();
        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        let mut xml = XMLElement::new("directory");
        for repo_name in project.repos.keys() {
            let mut entry_xml = XMLElement::new("entry");
            entry_xml.add_attribute("name", repo_name);
            xml.add_child(entry_xml).unwrap();
        }

        ResponseTemplate::new(200).set_body_xml(xml)
    }
}

pub(crate) struct ArchListingResponder {
    mock: ObsMock,
}

impl ArchListingResponder {
    pub fn new(mock: ObsMock) -> Self {
        ArchListingResponder { mock }
    }
}

impl Respond for ArchListingResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let repo_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();
        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));
        let arches = try_api!(project
            .repos
            .get(repo_name)
            .ok_or_else(|| unknown_repo(project_name, repo_name)));

        let mut xml = XMLElement::new("directory");
        for arch in arches.keys() {
            let mut child_xml = XMLElement::new("entry");
            child_xml.add_attribute("name", arch);
            xml.add_child(child_xml).unwrap();
        }

        ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
    }
}

pub(crate) struct BuildResultsResponder {
    mock: ObsMock,
}

impl BuildResultsResponder {
    pub fn new(mock: ObsMock) -> BuildResultsResponder {
        BuildResultsResponder { mock }
    }
}

fn package_status_xml(package_name: &str, status: &MockBuildStatus) -> XMLElement {
    let mut xml = XMLElement::new("status");
    xml.add_attribute("package", package_name);
    xml.add_attribute("code", &status.code.to_string());
    if status.dirty {
        xml.add_attribute("dirty", "true");
    }
    xml
}

impl Respond for BuildResultsResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let project_name = components.nth_back(1).unwrap();

        let mut package_filters = vec![];
        for (key, value) in request.url.query_pairs() {
            ensure!(key == "package", unknown_parameter(&key));
            package_filters.push(value);
        }

        let projects = self.mock.projects().read().unwrap();
        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        for package_name in &package_filters {
            ensure!(
                project.packages.contains_key(package_name.as_ref()),
                unknown_package(package_name)
            );
        }

        let mut xml = XMLElement::new("resultlist");
        // Using a random 'state' value for now, need to figure out how
        // these are computed.
        xml.add_attribute("state", "3ff37f67d60b76bd0491a5243311ba81");

        for (repo_name, arches) in &project.repos {
            for (arch, repo) in arches {
                let mut result_xml = XMLElement::new("result");
                result_xml.add_attribute("project", project_name);
                result_xml.add_attribute("repository", repo_name);
                result_xml.add_attribute("arch", arch);
                result_xml.add_attribute("code", &repo.code.to_string());
                // Deprecated alias for 'code'.
                result_xml.add_attribute("state", &repo.code.to_string());

                if package_filters.is_empty() {
                    for (package_name, package) in &repo.packages {
                        result_xml
                            .add_child(package_status_xml(package_name, &package.status))
                            .unwrap();
                    }
                } else {
                    for package_name in &package_filters {
                        if let Some(package) = repo.packages.get(package_name.as_ref()) {
                            result_xml
                                .add_child(package_status_xml(package_name, &package.status))
                                .unwrap();
                        }
                    }
                }

                xml.add_child(result_xml).unwrap();
            }
        }

        ResponseTemplate::new(200).set_body_xml(xml)
    }
}

pub(crate) struct BuildBinaryListResponder {
    mock: ObsMock,
}

impl BuildBinaryListResponder {
    pub fn new(mock: ObsMock) -> BuildBinaryListResponder {
        BuildBinaryListResponder { mock }
    }
}

impl Respond for BuildBinaryListResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(0).unwrap();
        let arch = components.nth_back(0).unwrap();
        let repo_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();

        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name)
        );

        let arches = try_api!(project
            .repos
            .get(repo_name)
            .ok_or_else(|| unknown_repo(project_name, repo_name)));
        let arch =
            try_api!(arches
                .get(arch)
                .ok_or_else(|| unknown_arch(project_name, repo_name, arch)));

        let mut xml = XMLElement::new("binarylist");
        if let Some(package) = arch.packages.get(package_name) {
            for (name, binary) in &package.binaries {
                let mut binary_xml = XMLElement::new("binary");
                binary_xml.add_attribute("filename", name);
                binary_xml.add_attribute("size", &binary.contents.len().to_string());
                binary_xml.add_attribute(
                    "mtime",
                    &binary
                        .mtime
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        .to_string(),
                );

                xml.add_child(binary_xml).unwrap();
            }
        }

        ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
    }
}

pub(crate) struct BuildBinaryFileResponder {
    mock: ObsMock,
}

impl BuildBinaryFileResponder {
    pub fn new(mock: ObsMock) -> BuildBinaryFileResponder {
        BuildBinaryFileResponder { mock }
    }
}

impl Respond for BuildBinaryFileResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let file_name = components.nth_back(0).unwrap();
        let package_name = components.nth_back(0).unwrap();
        let arch = components.nth_back(0).unwrap();
        let repo_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();

        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name)
        );

        let arches = try_api!(project
            .repos
            .get(repo_name)
            .ok_or_else(|| unknown_repo(project_name, repo_name)));
        let arch =
            try_api!(arches
                .get(arch)
                .ok_or_else(|| unknown_arch(project_name, repo_name, arch)));
        let package = arch.packages.get(package_name);

        let file = try_api!(package
            .and_then(|package| package.binaries.get(file_name))
            .ok_or_else(|| ApiError::new(
                StatusCode::NotFound,
                "404".to_owned(),
                format!("{}: No such file or directory", file_name)
            )));
        ResponseTemplate::new(StatusCode::Ok)
            .set_body_raw(file.contents.clone(), "application/octet-stream")
    }
}

pub(crate) struct BuildPackageStatusResponder {
    mock: ObsMock,
}

impl BuildPackageStatusResponder {
    pub fn new(mock: ObsMock) -> BuildPackageStatusResponder {
        BuildPackageStatusResponder { mock }
    }
}

impl Respond for BuildPackageStatusResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(1).unwrap();
        let arch = components.nth_back(0).unwrap();
        let repo_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();

        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name)
        );

        let arches = try_api!(project
            .repos
            .get(repo_name)
            .ok_or_else(|| unknown_repo(project_name, repo_name)));
        let arch =
            try_api!(arches
                .get(arch)
                .ok_or_else(|| unknown_arch(project_name, repo_name, arch)));

        let package = arch.packages.get(package_name);
        ResponseTemplate::new(StatusCode::Ok).set_body_xml(package.map_or_else(
            || {
                package_status_xml(
                    package_name,
                    &MockBuildStatus::new(MockPackageCode::Disabled),
                )
            },
            |package| package_status_xml(package_name, &package.status),
        ))
    }
}

pub(crate) struct BuildLogResponder {
    mock: ObsMock,
}

impl BuildLogResponder {
    pub fn new(mock: ObsMock) -> BuildLogResponder {
        BuildLogResponder { mock }
    }
}

fn parse_number_param(value: Cow<str>) -> Result<usize, ApiError> {
    if value.is_empty() {
        return Err(ApiError::new(
            StatusCode::BadRequest,
            "400".to_owned(),
            "number is empty".to_owned(),
        ));
    }

    value.as_ref().parse().map_err(|_| {
        ApiError::new(
            StatusCode::BadRequest,
            "400".to_owned(),
            format!("not a number: '{}'", value),
        )
    })
}

fn parse_bool_param(value: Cow<str>) -> Result<bool, ApiError> {
    match value.as_ref() {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(ApiError::new(
            StatusCode::BadRequest,
            "400".to_owned(),
            "not a boolean".to_owned(),
        )),
    }
}

impl Respond for BuildLogResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut start = 0usize;
        let mut end = None;
        // Note that these APIs have no concept of an incomplete build log at
        // the moment.
        let mut last_successful = false;
        // Streamed logs are not supported.
        let mut entry_view = false;

        for (key, value) in request.url.query_pairs() {
            match key.as_ref() {
                "start" => start = try_api!(parse_number_param(value)),
                "end" => end = Some(try_api!(parse_number_param(value))),
                // We don't support incomplete build logs yet, so this does
                // nothing.
                "last" => {
                    try_api!(parse_bool_param(value));
                }
                "lastsucceeded" => last_successful = try_api!(parse_bool_param(value)),
                // All build logs are nostream at the moment.
                "nostream" => {
                    try_api!(parse_bool_param(value));
                }
                // For some reason, OBS returns a different error if the value is
                // empty, so mimic that here.
                "view" if !value.is_empty() => {
                    ensure!(
                        value == "entry",
                        ApiError::new(
                            StatusCode::BadRequest,
                            "400".to_owned(),
                            format!("unknown view '{}'", value)
                        )
                    );
                    entry_view = true;
                }
                _ => return unknown_parameter(&key).into_response(),
            }
        }

        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(1).unwrap();
        let arch = components.nth_back(0).unwrap();
        let repo_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();

        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name)
        );

        let arches = try_api!(project
            .repos
            .get(repo_name)
            .ok_or_else(|| unknown_repo(project_name, repo_name)));
        let arch =
            try_api!(arches
                .get(arch)
                .ok_or_else(|| unknown_arch(project_name, repo_name, arch)));
        let package = try_api!(arch.packages.get(package_name).ok_or_else(|| ApiError::new(
            StatusCode::BadRequest,
            "400".to_owned(),
            format!("remote error: {} no logfile", package_name)
        )));

        let log = if last_successful {
            &package.latest_successful_log
        } else {
            &package.latest_log
        };

        if entry_view {
            let mut xml = XMLElement::new("directory");
            // XXX: Not sure what to do if no logs are present, for now just
            // return no file.
            if let Some(log) = log {
                let mut entry_xml = XMLElement::new("entry");
                entry_xml.add_attribute("name", "_log");
                entry_xml.add_attribute("size", &log.contents.len().to_string());
                entry_xml.add_attribute(
                    "mtime",
                    &log.mtime
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        .to_string(),
                );

                xml.add_child(entry_xml).unwrap();
            }

            ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
        } else {
            let contents = log.as_ref().map_or("", |log| &log.contents);
            ensure!(
                start <= contents.len(),
                ApiError::new(
                    StatusCode::BadRequest,
                    "400".to_owned(),
                    format!("remote error: start out of range  {}", start)
                )
            );

            let end = std::cmp::min(end.unwrap_or(contents.len()), contents.len());
            let end = std::cmp::min(
                end,
                log.as_ref()
                    .and_then(|log| log.chunk_size)
                    .map(|chunk_size| start + chunk_size)
                    .unwrap_or(end),
            );

            ResponseTemplate::new(StatusCode::Ok).set_body_string(&contents[start..end])
        }
    }
}

pub(crate) struct BuildHistoryResponder {
    mock: ObsMock,
}

impl BuildHistoryResponder {
    pub fn new(mock: ObsMock) -> BuildHistoryResponder {
        BuildHistoryResponder { mock }
    }
}

impl Respond for BuildHistoryResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(1).unwrap();
        let arch = components.nth_back(0).unwrap();
        let repo_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        if let Some((param, _)) = request.url.query_pairs().next() {
            return unknown_parameter(&param).into_response();
        }

        let projects = self.mock.projects().read().unwrap();

        let project = try_api!(projects
            .get(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name)
        );

        let arches = try_api!(project
            .repos
            .get(repo_name)
            .ok_or_else(|| unknown_repo(project_name, repo_name)));
        let arch =
            try_api!(arches
                .get(arch)
                .ok_or_else(|| unknown_arch(project_name, repo_name, arch)));

        let mut xml = XMLElement::new("buildhistory");

        if let Some(package) = arch.packages.get(package_name) {
            for entry in &package.history {
                let mut entry_xml = XMLElement::new("entry");
                entry_xml.add_attribute("rev", &entry.rev);
                entry_xml.add_attribute("srcmd5", &entry.srcmd5);
                entry_xml.add_attribute("versrel", &entry.versrel);
                entry_xml.add_attribute("bcnt", &entry.bcnt.to_string());
                entry_xml.add_attribute(
                    "time",
                    &entry
                        .time
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        .to_string(),
                );
                entry_xml.add_attribute("duration", &entry.duration.as_secs().to_string());

                xml.add_child(entry_xml).unwrap();
            }
        }

        ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
    }
}
