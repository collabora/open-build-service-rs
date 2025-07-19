use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use wiremock::ResponseTemplate;
use wiremock::{Request, Respond};

use crate::{MockBuildStatus, MockPackageCode, ObsMock};

use super::*;

fn unknown_repo(project: &str, repo: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "404".to_owned(),
        format!("project '{}' has no repository '{}'", project, repo),
    )
}

fn unknown_arch(project: &str, repo: &str, arch: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "404".to_owned(),
        format!(
            "repository '{}/{}' has no architecture '{}'",
            project, repo, arch
        ),
    )
}

fn unknown_parameter(param: &str) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
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
        let project = try_api!(
            projects
                .get_mut(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );

        let cmd = try_api!(
            find_query_param(request, "cmd").ok_or_else(|| ApiError::new(
                StatusCode::BAD_REQUEST,
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
                                StatusCode::MISDIRECTED_REQUEST,
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
                        let inner_xml = unknown_package(package_name.to_owned()).into_xml();
                        let inner = inner_xml.into_inner().into_inner();

                        return ApiError::new(
                            StatusCode::NOT_FOUND,
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

                ResponseTemplate::new(StatusCode::OK)
                    .set_body_xml(build_status_xml("ok", None, |_| Ok(())).unwrap())
            }
            _ => ApiError::new(
                StatusCode::BAD_REQUEST,
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
        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );

        let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
        xml.create_element("directory")
            .write_inner_content(|writer| {
                for repo_name in project.repos.keys() {
                    writer
                        .create_element("entry")
                        .with_attribute(("name", repo_name.as_str()))
                        .write_empty()?;
                }
                Ok(())
            })
            .unwrap();

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
        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );
        let arches = try_api!(
            project
                .repos
                .get(repo_name)
                .ok_or_else(|| unknown_repo(project_name, repo_name))
        );

        let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
        xml.create_element("directory")
            .write_inner_content(|writer| {
                for arch in arches.keys() {
                    writer
                        .create_element("entry")
                        .with_attribute(("name", arch.as_str()))
                        .write_empty()?;
                }
                Ok(())
            })
            .unwrap();

        ResponseTemplate::new(StatusCode::OK).set_body_xml(xml)
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

fn package_status_xml(
    xml: &mut XMLWriter,
    package_name: &str,
    status: &MockBuildStatus,
) -> quick_xml::Result<()> {
    use quick_xml::events::BytesText;
    let mut status_xml = xml.create_element("status").with_attributes([
        ("package", package_name),
        ("code", &status.code.to_string()),
    ]);
    if status.dirty {
        status_xml = status_xml.with_attribute(("dirty", "true"));
    }

    status_xml.write_inner_content(|writer| {
        writer
            .create_element("details")
            .write_text_content(BytesText::from_plain_str(status.details.as_str()))?;
        Ok(())
    })?;

    Ok(())
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
        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );

        for package_name in &package_filters {
            ensure!(
                project.packages.contains_key(package_name.as_ref()),
                unknown_package(package_name.clone().into_owned())
            );
        }

        let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
        xml.create_element("resultlist")
            // Using a random 'state' value for now, need to figure out how
            // these are computed.
            .with_attribute(("state", "3ff37f67d60b76bd0491a5243311ba81"))
            .write_inner_content(|writer| {
                for (repo_name, arches) in &project.repos {
                    for (arch, repo) in arches {
                        let result_xml = writer.create_element("result").with_attributes([
                            ("project", project_name),
                            ("repository", repo_name.as_str()),
                            ("arch", arch.as_str()),
                            ("code", repo.code.to_string().as_str()),
                            // Deprecated alias for 'code'.
                            ("state", repo.code.to_string().as_str()),
                        ]);

                        if package_filters.is_empty() {
                            result_xml
                                .write_inner_content(|writer| {
                                    for (package_name, package) in &repo.packages {
                                        package_status_xml(writer, package_name, &package.status)
                                            .unwrap();
                                    }
                                    Ok(())
                                })
                                .unwrap();
                        } else {
                            result_xml
                                .write_inner_content(|writer| {
                                    for package_name in &package_filters {
                                        if let Some(package) =
                                            repo.packages.get(package_name.as_ref())
                                        {
                                            package_status_xml(
                                                writer,
                                                package_name,
                                                &package.status,
                                            )
                                            .unwrap();
                                        }
                                    }
                                    Ok(())
                                })
                                .unwrap();
                        }
                    }
                }
                Ok(())
            })
            .unwrap();

        ResponseTemplate::new(200).set_body_xml(xml)
    }
}

pub(crate) struct BuildJobHistoryResponder {
    mock: ObsMock,
}

impl BuildJobHistoryResponder {
    pub fn new(mock: ObsMock) -> Self {
        Self { mock }
    }
}

impl Respond for BuildJobHistoryResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut package_names = vec![];
        let mut code_names = vec![];
        let mut limit = None;

        for (key, value) in request.url.query_pairs() {
            match key.as_ref() {
                "package" => package_names.push(value.into_owned()),
                "code" => code_names.push(value.into_owned()),
                "limit" if limit.is_some() => {
                    return ApiError::new(
                        StatusCode::OK,
                        "400".to_owned(),
                        "parameter 'limit' set multiple times".to_owned(),
                    )
                    .into_response();
                }
                "limit" => limit = Some(try_api!(parse_number_param(value))),
                _ => return unknown_parameter(&key).into_response(),
            }
        }

        let mut components = request.url.path_segments().unwrap();
        let arch = components.nth_back(1).unwrap();
        let repo_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let projects = self.mock.projects().read().unwrap();

        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );

        let arches = try_api!(
            project
                .repos
                .get(repo_name)
                .ok_or_else(|| unknown_repo(project_name, repo_name))
        );
        let arch = try_api!(arches.get(arch).ok_or_else(|| unknown_arch(
            project_name,
            repo_name,
            arch
        )));

        let mut codes = HashSet::new();
        for code_name in code_names {
            if let Ok(code) = MockPackageCode::from_str(&code_name) {
                codes.insert(code);
            }
        }

        let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
        xml.create_element("jobhistlist")
            .write_inner_content(|writer| {
                let mut entries_added = 0;

                for entry in &arch.jobhist {
                    if (!package_names.is_empty() && !package_names.contains(&entry.package))
                        || (!codes.is_empty() && !codes.contains(&entry.code))
                    {
                        continue;
                    }

                    writer
                        .create_element("jobhist")
                        .with_attributes([
                            ("package", entry.package.as_str()),
                            ("rev", &entry.rev),
                            ("srcmd5", &entry.srcmd5),
                            ("versrel", &entry.versrel),
                            ("bcnt", &entry.bcnt.to_string()),
                            (
                                "readytime",
                                &seconds_since_epoch(&entry.readytime).to_string(),
                            ),
                            (
                                "starttime",
                                &seconds_since_epoch(&entry.starttime).to_string(),
                            ),
                            ("endtime", &seconds_since_epoch(&entry.endtime).to_string()),
                            ("code", &entry.code.to_string()),
                            ("uri", &entry.uri),
                            ("workerid", &entry.workerid),
                            ("hostarch", &entry.hostarch),
                            ("reason", &entry.reason),
                            ("verifymd5", &entry.verifymd5),
                        ])
                        .write_empty()?;
                    entries_added += 1;

                    if let Some(limit) = limit {
                        if limit > 0 && entries_added >= limit {
                            break;
                        }
                    }
                }

                Ok(())
            })
            .unwrap();

        ResponseTemplate::new(StatusCode::OK).set_body_xml(xml)
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

        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name.to_owned())
        );

        let arches = try_api!(
            project
                .repos
                .get(repo_name)
                .ok_or_else(|| unknown_repo(project_name, repo_name))
        );
        let arch = try_api!(arches.get(arch).ok_or_else(|| unknown_arch(
            project_name,
            repo_name,
            arch
        )));

        let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
        xml.create_element("binarylist")
            .write_inner_content(|writer| {
                if let Some(package) = arch.packages.get(package_name) {
                    for (name, binary) in &package.binaries {
                        writer
                            .create_element("binary")
                            .with_attributes([
                                ("filename", name.as_str()),
                                ("size", &binary.contents.len().to_string()),
                                ("mtime", &seconds_since_epoch(&binary.mtime).to_string()),
                            ])
                            .write_empty()?;
                    }
                }
                Ok(())
            })
            .unwrap();

        ResponseTemplate::new(StatusCode::OK).set_body_xml(xml)
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

        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name.to_owned())
        );

        let arches = try_api!(
            project
                .repos
                .get(repo_name)
                .ok_or_else(|| unknown_repo(project_name, repo_name))
        );
        let arch = try_api!(arches.get(arch).ok_or_else(|| unknown_arch(
            project_name,
            repo_name,
            arch
        )));
        let package = arch.packages.get(package_name);

        let file = try_api!(
            package
                .and_then(|package| package.binaries.get(file_name))
                .ok_or_else(|| ApiError::new(
                    StatusCode::NOT_FOUND,
                    "404".to_owned(),
                    format!("{}: No such file or directory", file_name)
                ))
        );
        ResponseTemplate::new(StatusCode::OK)
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

        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name.to_owned())
        );

        let arches = try_api!(
            project
                .repos
                .get(repo_name)
                .ok_or_else(|| unknown_repo(project_name, repo_name))
        );
        let arch = try_api!(arches.get(arch).ok_or_else(|| unknown_arch(
            project_name,
            repo_name,
            arch
        )));

        let package = arch.packages.get(package_name);
        ResponseTemplate::new(StatusCode::OK).set_body_xml(package.map_or_else(
            || {
                let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
                package_status_xml(
                    &mut xml,
                    package_name,
                    &MockBuildStatus::new(MockPackageCode::Unknown),
                )
                .unwrap();
                xml
            },
            |package| {
                let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
                package_status_xml(&mut xml, package_name, &package.status).unwrap();
                xml
            },
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
            StatusCode::BAD_REQUEST,
            "400".to_owned(),
            "number is empty".to_owned(),
        ));
    }

    value.as_ref().parse().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
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
            StatusCode::BAD_REQUEST,
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
                            StatusCode::BAD_REQUEST,
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

        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name.to_owned())
        );

        let arches = try_api!(
            project
                .repos
                .get(repo_name)
                .ok_or_else(|| unknown_repo(project_name, repo_name))
        );
        let arch = try_api!(arches.get(arch).ok_or_else(|| unknown_arch(
            project_name,
            repo_name,
            arch
        )));
        let package = try_api!(arch.packages.get(package_name).ok_or_else(|| ApiError::new(
            StatusCode::BAD_REQUEST,
            "400".to_owned(),
            format!("remote error: {} no logfile", package_name)
        )));

        let log = if last_successful {
            &package.latest_successful_log
        } else {
            &package.latest_log
        };

        if entry_view {
            let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
            // XXX: Not sure what to do if no logs are present, for now just
            // return no file.
            xml.create_element("directory")
                .write_inner_content(|writer| {
                    if let Some(log) = log {
                        writer
                            .create_element("entry")
                            .with_attributes([
                                ("name", "_log"),
                                ("size", &log.contents.len().to_string()),
                                ("mtime", &seconds_since_epoch(&log.mtime).to_string()),
                            ])
                            .write_empty()?;
                    }
                    Ok(())
                })
                .unwrap();

            ResponseTemplate::new(StatusCode::OK).set_body_xml(xml)
        } else {
            let contents = log.as_ref().map_or("", |log| &log.contents);
            ensure!(
                start <= contents.len(),
                ApiError::new(
                    StatusCode::BAD_REQUEST,
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

            ResponseTemplate::new(StatusCode::OK).set_body_string(&contents[start..end])
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

        let project = try_api!(
            projects
                .get(project_name)
                .ok_or_else(|| unknown_project(project_name.to_owned()))
        );
        ensure!(
            project.packages.contains_key(package_name),
            unknown_package(package_name.to_owned())
        );

        let arches = try_api!(
            project
                .repos
                .get(repo_name)
                .ok_or_else(|| unknown_repo(project_name, repo_name))
        );
        let arch = try_api!(arches.get(arch).ok_or_else(|| unknown_arch(
            project_name,
            repo_name,
            arch
        )));

        let mut xml = XMLWriter::new_with_indent(Default::default(), b' ', 8);
        xml.create_element("buildhistory")
            .write_inner_content(|writer| {
                if let Some(package) = arch.packages.get(package_name) {
                    for entry in &package.history {
                        writer
                            .create_element("entry")
                            .with_attributes([
                                ("rev", entry.rev.as_str()),
                                ("srcmd5", &entry.srcmd5),
                                ("versrel", &entry.versrel),
                                ("bcnt", &entry.bcnt.to_string()),
                                ("time", &seconds_since_epoch(&entry.time).to_string()),
                                ("duration", &entry.duration.as_secs().to_string()),
                            ])
                            .write_empty()?;
                    }
                }
                Ok(())
            })
            .unwrap();

        ResponseTemplate::new(StatusCode::OK).set_body_xml(xml)
    }
}
