use std::collections::HashMap;
use std::io::BufReader;
use std::time::SystemTime;

use http_types::StatusCode;
use serde::{de::DeserializeOwned, Deserialize};
use wiremock::ResponseTemplate;
use wiremock::{Request, Respond};
use xml_builder::XMLElement;

use crate::{random_md5, MockEntry, MockRevision, MockRevisionOptions, ObsMock};

use super::*;

fn unknown_package(package: String) -> ApiError {
    ApiError::new(StatusCode::NotFound, "unknown_package".to_owned(), package)
}

fn source_file_not_found(name: &str) -> ApiError {
    ApiError::new(
        StatusCode::NotFound,
        "404".to_owned(),
        format!("{}: no such file", name),
    )
}

fn source_listing_xml(package_name: &str, rev_id: usize, rev: &MockRevision) -> XMLElement {
    let mut xml = XMLElement::new("directory");
    xml.add_attribute("name", package_name);
    xml.add_attribute("rev", &rev_id.to_string());
    xml.add_attribute("vrev", &rev.vrev.to_string());
    xml.add_attribute("srcmd5", &rev.options.srcmd5);

    for linkinfo in &rev.linkinfo {
        let mut link_xml = XMLElement::new("linkinfo");
        link_xml.add_attribute("project", &linkinfo.project);
        link_xml.add_attribute("package", &linkinfo.package);
        link_xml.add_attribute("baserev", &linkinfo.baserev);
        link_xml.add_attribute("srcmd5", &linkinfo.srcmd5);
        link_xml.add_attribute("xsrcmd5", &linkinfo.xsrcmd5);
        link_xml.add_attribute("lsrcmd5", &linkinfo.lsrcmd5);

        xml.add_child(link_xml).unwrap();
    }

    for (name, entry) in &rev.options.entries {
        let mut entry_xml = XMLElement::new("entry");
        entry_xml.add_attribute("name", name);
        entry_xml.add_attribute("md5", &entry.md5);
        entry_xml.add_attribute("size", &entry.contents.len().to_string());
        entry_xml.add_attribute(
            "mtime",
            &entry
                .mtime
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        );

        xml.add_child(entry_xml).unwrap();
    }

    xml
}

fn parse_xml_request<T: DeserializeOwned>(request: &Request) -> Result<T, ApiError> {
    quick_xml::de::from_reader(BufReader::new(&request.body[..]))
        .map_err(|e| ApiError::new(StatusCode::BadRequest, "400".to_string(), e.to_string()))
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

        let rev_id = if let Some(rev_arg) = find_query_param(request, "rev") {
            let index: usize = try_api!(rev_arg.parse().map_err(|_| ApiError::new(
                StatusCode::BadRequest,
                "400".to_owned(),
                format!("bad revision '{}'", rev_arg)
            )));
            ensure!(
                index <= package.revisions.len(),
                ApiError::new(
                    StatusCode::BadRequest,
                    "400".to_owned(),
                    "no such revision".to_owned(),
                )
            );

            index
        } else {
            package.revisions.len()
        };

        if rev_id == 0 {
            // OBS seems to have this weird zero revision that always has
            // the same md5 but no contents, so we just handle it in here.
            const ZERO_REV_SRCMD5: &str = "d41d8cd98f00b204e9800998ecf8427e";

            let mut xml = XMLElement::new("directory");
            xml.add_attribute("name", package_name);
            xml.add_attribute("srcmd5", ZERO_REV_SRCMD5);

            return ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml);
        }

        // -1 to skip the zero revision (see above).
        let rev = &package.revisions[rev_id - 1];
        ResponseTemplate::new(StatusCode::Ok).set_body_xml(source_listing_xml(
            package_name,
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

        match package.revisions.last() {
            Some(rev) => {
                let entry = try_api!(rev
                    .options
                    .entries
                    .get(file_name)
                    .ok_or_else(|| source_file_not_found(file_name)));
                let content_type = if file_name == "_meta" {
                    "application/xml"
                } else {
                    "application/octet-stream"
                };

                ResponseTemplate::new(200).set_body_raw(entry.contents.clone(), content_type)
            }
            None => {
                // For "revision 0", only "_meta" is valid.
                if file_name == "_meta" {
                    ResponseTemplate::new(200).set_body_raw(
                        MockEntry::new_metadata(project_name, package_name, SystemTime::UNIX_EPOCH)
                            .contents,
                        "application/xml",
                    )
                } else {
                    source_file_not_found(file_name).into_response()
                }
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

        let package = if file_name == "_meta" {
            // Creating _meta will create an empty package.
            project.packages.entry(package_name.to_owned()).or_default()
        } else {
            try_api!(project
                .packages
                .get_mut(package_name)
                .ok_or_else(|| unknown_package(package_name.to_owned())))
        };

        if file_name == "_meta" {
            // TODO: parse file, return errors if attributes don't match (the
            // API crate doesn't add these at all, so leaving this out for now
            // is relatively low-risk)

            // We don't actually *add* _meta changes, since that's added to
            // commits on-demand. It sort-of matches with OBS's behavior of
            // completely re-formatting / serializing the metadata & ignoring
            // any unknown tags and such.
            ResponseTemplate::new(StatusCode::Ok).set_body_status_xml("ok", "Ok".to_owned())
        } else {
            if matches!(rev.as_ref().map(AsRef::as_ref), Some("repository")) {
                package.pending_rev_entries.insert(
                    file_name.to_owned(),
                    MockEntry::new_with_contents(SystemTime::now(), request.body.clone()),
                );

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

impl Respond for PackageSourceCommandResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        try_api!(check_auth(self.mock.auth(), request));

        let mut components = request.url.path_segments().unwrap();
        let package_name = components.nth_back(0).unwrap();
        let project_name = components.nth_back(0).unwrap();

        let mut projects = self.mock.projects().write().unwrap();
        let project = try_api!(projects
            .get_mut(project_name)
            .ok_or_else(|| unknown_project(project_name.to_owned())));

        let package = try_api!(project
            .packages
            .get_mut(package_name)
            .ok_or_else(|| unknown_package(package_name.to_owned())));

        let cmd = try_api!(
            find_query_param(request, "cmd").ok_or_else(|| ApiError::new(
                StatusCode::BadRequest,
                "missing_parameter".to_string(),
                "POST request without given cmd parameter".to_string()
            ))
        );

        let comment = find_query_param(request, "comment");

        match cmd.as_ref() {
            "commitfilelist" => {
                let time = SystemTime::now();

                let mut entries = HashMap::new();

                let filelist: DirectoryRequest = try_api!(parse_xml_request(request));
                let mut missing = Vec::new();
                let last_rev = package.revisions.last();

                for req_entry in filelist.entries {
                    if let Some(entry) = package.pending_rev_entries.get(&req_entry.name) {
                        if entry.md5 == req_entry.md5 {
                            entries.insert(req_entry.name, entry.clone());
                            continue;
                        }
                    }

                    if let Some(last_rev) = last_rev {
                        if let Some(entry) = last_rev.options.entries.get(&req_entry.name) {
                            if entry.md5 == req_entry.md5 {
                                entries.insert(req_entry.name, entry.clone());
                                continue;
                            }
                        }
                    }

                    missing.push(req_entry);
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

                package.pending_rev_entries.clear();

                let options = MockRevisionOptions {
                    srcmd5: random_md5(),
                    // TODO: detect the source package version
                    version: None,
                    time,
                    user: self.mock.auth().username().to_owned(),
                    comment: comment.map(|c| c.into_owned()),
                    entries,
                };
                package.add_revision(options);

                let rev_id = package.revisions.len();
                let rev = package.revisions.last().unwrap();
                ResponseTemplate::new(StatusCode::Ok).set_body_xml(source_listing_xml(
                    package_name,
                    rev_id,
                    rev,
                ))
            }
            _ => ApiError::new(
                StatusCode::NotFound,
                "illegal_request".to_string(),
                "invalid_command".to_string(),
            )
            .into_response(),
        }
    }
}
