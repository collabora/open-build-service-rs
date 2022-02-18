use std::time::{Duration, SystemTime};

use futures::StreamExt;

use open_build_service_api::*;
use open_build_service_mock::*;

const DEFAULT_USERNAME: &str = "user";
const DEFAULT_PASSWORD: &str = "pass";

fn test_project() -> String {
    "test_project".to_owned()
}

fn test_repo() -> String {
    "test_repo".to_owned()
}

fn test_arch_1() -> String {
    "aarch64".to_owned()
}

fn test_arch_2() -> String {
    "x86_64".to_owned()
}

fn test_package_1() -> String {
    "test_package_1".to_owned()
}

fn test_package_2() -> String {
    "test_package_2".to_owned()
}

async fn start_mock() -> ObsMock {
    ObsMock::start(DEFAULT_USERNAME, DEFAULT_PASSWORD).await
}

fn create_authenticated_client(mock: ObsMock) -> Client {
    Client::new(
        mock.uri(),
        mock.auth().username().to_owned(),
        mock.auth().password().to_owned(),
    )
}

#[tokio::test]
async fn test_package_list() {
    let mock = start_mock().await;
    mock.add_project(test_project());

    let mtime = SystemTime::UNIX_EPOCH + Duration::new(10, 0);
    let srcmd5 = random_md5();
    mock.add_package_revision(
        &test_project(),
        test_package_1(),
        MockRevisionOptions {
            time: mtime.clone(),
            srcmd5: srcmd5.clone(),
            ..Default::default()
        },
    );

    let obs = create_authenticated_client(mock.clone());
    let dir = obs
        .project(test_project())
        .package(test_package_1())
        .list(None)
        .await
        .unwrap();

    assert_eq!(dir.name, test_package_1());
    assert_eq!(dir.rev, "1");
    assert_eq!(dir.vrev, "1");
    assert_eq!(dir.srcmd5, srcmd5);

    assert_eq!(dir.entries.len(), 1);
    assert_eq!(dir.linkinfo.len(), 0);

    let meta = &dir.entries[0];
    assert_eq!(meta.name, "_meta");
    assert_eq!(
        meta.mtime,
        mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    let srcmd5 = random_md5();
    let test_data = b"abc";

    mock.add_package_revision(
        &test_project(),
        test_package_1(),
        MockRevisionOptions {
            srcmd5: srcmd5.clone(),
            entries: [(
                "test".to_owned(),
                MockEntry::new_with_contents(SystemTime::now(), test_data.to_vec()),
            )]
            .into(),
            ..Default::default()
        },
    );

    let dir = obs
        .project(test_project())
        .package(test_package_1())
        .list(None)
        .await
        .unwrap();

    assert_eq!(dir.name, test_package_1());
    assert_eq!(dir.rev, "2");
    assert_eq!(dir.vrev, "2");
    assert_eq!(dir.srcmd5, srcmd5);

    assert_eq!(dir.entries.len(), 2);

    let test_entry = dir.entries.iter().find(|e| e.name == "test").unwrap();
    assert_eq!(test_entry.size, test_data.len() as u64);

    let dir = obs
        .project(test_project())
        .package(test_package_1())
        .list(Some("1"))
        .await
        .unwrap();

    assert_eq!(dir.rev, "1");
    assert_eq!(dir.entries.len(), 1);

    let branch_srcmd5 = random_md5();
    let branch_xsrcmd5 = random_md5();

    mock.branch(
        test_project(),
        test_package_1(),
        &test_project(),
        test_package_2(),
        MockBranchOptions {
            srcmd5: branch_srcmd5.clone(),
            xsrcmd5: branch_xsrcmd5.clone(),
            ..Default::default()
        },
    );

    let dir = obs
        .project(test_project())
        .package(test_package_2())
        .list(None)
        .await
        .unwrap();

    assert_eq!(dir.rev, "1");
    assert_eq!(dir.vrev, "1");
    assert_eq!(dir.srcmd5, branch_srcmd5);
    assert_eq!(dir.entries.len(), 2);
    assert_eq!(dir.linkinfo.len(), 1);

    let linkinfo = &dir.linkinfo[0];
    assert_eq!(linkinfo.project, test_project());
    assert_eq!(linkinfo.package, test_package_1());
    assert_eq!(linkinfo.srcmd5, srcmd5);
    assert_eq!(linkinfo.lsrcmd5, branch_srcmd5);
    assert_eq!(linkinfo.xsrcmd5, branch_xsrcmd5);
}

fn get_results_by_arch(mut results: ResultList) -> (ResultListResult, ResultListResult) {
    assert_eq!(results.results.len(), 2);

    // Sort by the arch, so we know arch 1 is first and arch 2 is second.
    results.results.sort_by_key(|result| result.arch.clone());

    let mut it = results.results.into_iter();
    let a = it.next().unwrap();
    let b = it.next().unwrap();

    assert_eq!(a.arch, test_arch_1());
    assert_eq!(b.arch, test_arch_2());

    (a, b)
}

#[tokio::test]
async fn test_build_repo_listing() {
    let mock = start_mock().await;

    mock.add_project(test_project());
    mock.add_or_update_repository(
        &test_project(),
        test_repo(),
        test_arch_1(),
        MockRepositoryCode::Building,
    );
    mock.add_or_update_repository(
        &test_project(),
        test_repo(),
        test_arch_2(),
        MockRepositoryCode::Broken,
    );

    let obs = create_authenticated_client(mock.clone());

    let repositories = obs.project(test_project()).repositories().await.unwrap();
    assert_eq!(&repositories[..], &[test_repo()]);

    let mut arches = obs
        .project(test_project())
        .arches(&test_repo())
        .await
        .unwrap();
    arches.sort();
    assert_eq!(&arches[..], &[test_arch_1(), test_arch_2()]);
}

#[tokio::test]
async fn test_build_results() {
    let mock = start_mock().await;

    mock.add_project(test_project());
    mock.add_or_update_repository(
        &test_project(),
        test_repo(),
        test_arch_1(),
        MockRepositoryCode::Building,
    );
    mock.add_or_update_repository(
        &test_project(),
        test_repo(),
        test_arch_2(),
        MockRepositoryCode::Broken,
    );

    mock.set_package_build_status(
        &test_project(),
        &test_repo(),
        &test_arch_1(),
        test_package_1(),
        MockBuildStatus::new(MockPackageCode::Building),
    );

    mock.set_package_build_status(
        &test_project(),
        &test_repo(),
        &test_arch_2(),
        test_package_2(),
        MockBuildStatus {
            code: MockPackageCode::Broken,
            dirty: true,
        },
    );

    let obs = create_authenticated_client(mock.clone());

    let results = obs.project(test_project()).result().await.unwrap();
    let (arch1_repo, arch2_repo) = get_results_by_arch(results);

    assert_eq!(arch1_repo.project, test_project());
    assert_eq!(arch1_repo.repository, test_repo());
    assert_eq!(arch1_repo.code, RepositoryCode::Building);
    assert_eq!(arch1_repo.statuses.len(), 1);

    let package1 = &arch1_repo.statuses[0];
    assert_eq!(package1.package, test_package_1());
    assert_eq!(package1.code, PackageCode::Building);
    assert!(!package1.dirty);

    assert_eq!(arch2_repo.project, test_project());
    assert_eq!(arch2_repo.repository, test_repo());
    assert_eq!(arch2_repo.code, RepositoryCode::Broken);
    assert_eq!(arch2_repo.statuses.len(), 1);

    let package2 = &arch2_repo.statuses[0];
    assert_eq!(package2.package, test_package_2());
    assert_eq!(package2.code, PackageCode::Broken);
    assert!(package2.dirty);

    mock.set_package_build_status(
        &test_project(),
        &test_repo(),
        &test_arch_1(),
        test_package_2(),
        MockBuildStatus::new(MockPackageCode::Broken),
    );

    let results = obs.project(test_project()).result().await.unwrap();
    let (arch1_repo, _) = get_results_by_arch(results);

    let package2_arch2 = arch1_repo
        .statuses
        .iter()
        .filter(|status| status.package == test_package_2())
        .next()
        .unwrap();
    assert_eq!(package2_arch2.package, test_package_2());
    assert_eq!(package2_arch2.code, PackageCode::Broken);

    let results = obs
        .project(test_project())
        .package(test_package_2())
        .result()
        .await
        .unwrap();
    let (arch1_repo, arch2_repo) = get_results_by_arch(results);

    assert_eq!(arch1_repo.statuses.len(), 1);
    assert_eq!(arch2_repo.statuses.len(), 1);

    assert_eq!(arch1_repo.statuses[0].package, test_package_2());
    assert_eq!(arch2_repo.statuses[0].package, test_package_2());
}

#[tokio::test]
async fn test_build_status() {
    let mock = start_mock().await;

    mock.add_project(test_project());
    mock.add_or_update_repository(
        &test_project(),
        test_repo(),
        test_arch_1(),
        MockRepositoryCode::Building,
    );
    mock.set_package_build_status(
        &test_project(),
        &test_repo(),
        &test_arch_1(),
        test_package_1(),
        MockBuildStatus::new(MockPackageCode::Building),
    );

    let obs = create_authenticated_client(mock.clone());

    let status = obs
        .project(test_project())
        .package(test_package_1())
        .status(&test_repo(), &test_arch_1())
        .await
        .unwrap();

    assert_eq!(status.package, test_package_1());
    assert_eq!(status.code, PackageCode::Building);
    assert!(!status.dirty);

    mock.set_package_build_status(
        &test_project(),
        &test_repo(),
        &test_arch_1(),
        test_package_1(),
        MockBuildStatus {
            code: MockPackageCode::Unknown,
            dirty: true,
        },
    );

    let status = obs
        .project(test_project())
        .package(test_package_1())
        .status(&test_repo(), &test_arch_1())
        .await
        .unwrap();

    assert_eq!(status.package, test_package_1());
    assert_eq!(status.code, PackageCode::Unknown);
    assert!(status.dirty);
}

#[tokio::test]
async fn test_build_logs() {
    let log = MockBuildLog {
        contents: "some log text".to_owned(),
        mtime: SystemTime::UNIX_EPOCH,
        chunk_size: Some(5),
    };

    let mock = start_mock().await;

    mock.add_project(test_project());
    mock.add_or_update_repository(
        &test_project(),
        test_repo(),
        test_arch_1(),
        MockRepositoryCode::Building,
    );
    mock.add_completed_build_log(
        &test_project(),
        &test_repo(),
        &test_arch_1(),
        test_package_1(),
        log.clone(),
        false,
    );

    let obs = create_authenticated_client(mock.clone());

    let (size, mtime) = obs
        .project(test_project())
        .package(test_package_1())
        .log(&test_repo(), &test_arch_1())
        .entry()
        .await
        .unwrap();

    assert_eq!(size, log.contents.len());
    assert_eq!(mtime, 0);

    let mut stream = obs
        .project(test_project())
        .package(test_package_1())
        .log(&test_repo(), &test_arch_1())
        .stream(Default::default())
        .unwrap();

    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b"some ");
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b"log t");
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b"ext");
    assert!(stream.next().await.is_none());

    let mut stream = obs
        .project(test_project())
        .package(test_package_1())
        .log(&test_repo(), &test_arch_1())
        .stream(PackageLogStreamOptions {
            offset: Some(4),
            end: Some(11),
            ..PackageLogStreamOptions::default()
        })
        .unwrap();

    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b" log ");
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b"te");
    assert!(stream.next().await.is_none());
}
