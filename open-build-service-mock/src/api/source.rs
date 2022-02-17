use std::time::SystemTime;

use wiremock::ResponseTemplate;
use wiremock::{Request, Respond};
use xml_builder::XMLElement;

use crate::ObsMock;

use super::*;

fn unknown_package(package: String) -> ApiError {
    ApiError::new(StatusCode::NotFound, "unknown_package".to_owned(), package)
}

pub(crate) struct PackageSourcesResponder {
    mock: ObsMock,
}

impl PackageSourcesResponder {
    pub fn new(mock: ObsMock) -> Self {
        PackageSourcesResponder { mock }
    }
}

impl Respond for PackageSourcesResponder {
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

        let rev_id = if let Some(rev_arg) =
            request
                .url
                .query_pairs()
                .find_map(|(key, value)| if key == "rev" { Some(value) } else { None })
        {
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

            if index == 0 {
                // OBS seems to have this weird zero revision that always has
                // the same md5 but no contents, so we just handle it in here.
                const ZERO_REV_SRCMD5: &str = "d41d8cd98f00b204e9800998ecf8427e";

                let mut xml = XMLElement::new("directory");
                xml.add_attribute("name", package_name);
                xml.add_attribute("srcmd5", ZERO_REV_SRCMD5);

                return ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml);
            }

            index
        } else {
            package.revisions.len()
        };

        // -1 to skip the zero revision (see above).
        let rev = &package.revisions[rev_id - 1];

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

        ResponseTemplate::new(StatusCode::Ok).set_body_xml(xml)
    }
}
