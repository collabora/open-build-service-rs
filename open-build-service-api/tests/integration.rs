use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};

use futures::{StreamExt, TryStreamExt};

use open_build_service_api::*;
use open_build_service_mock::*;

const DEFAULT_USERNAME: &str = "user";
const DEFAULT_PASSWORD: &str = "pass";

const TEST_PROJECT: &str = "test_project";
const TEST_REPO: &str = "test_repo";
const TEST_ARCH_1: &str = "aarch64";
const TEST_ARCH_2: &str = "x86_64";
const TEST_PACKAGE_1: &str = "test_package_1";
const TEST_PACKAGE_2: &str = "test_package_2";

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
async fn test_project_list() {
    let mock = start_mock().await;
    mock.add_project(TEST_PROJECT.to_owned());

    let obs = create_authenticated_client(mock.clone());
    let project = obs.project(TEST_PROJECT.to_owned());

    let dir = project.list_packages().await.unwrap();
    assert_eq!(dir.entries.len(), 0);

    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_2.to_owned(),
        MockPackageOptions::default(),
    );

    let mut dir = project.list_packages().await.unwrap();
    assert_eq!(dir.entries.len(), 2);

    dir.entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(dir.entries[0].name, TEST_PACKAGE_1);
    assert_eq!(dir.entries[1].name, TEST_PACKAGE_2);
}

#[tokio::test]
async fn test_project_meta() {
    let mock = start_mock().await;
    mock.add_project(TEST_PROJECT.to_owned());

    let obs = create_authenticated_client(mock.clone());
    let project = obs.project(TEST_PROJECT.to_owned());

    let meta = project.meta().await.unwrap();
    assert_eq!(meta.name, TEST_PROJECT);
    assert_eq!(meta.repositories.len(), 0);

    mock.set_project_modes(TEST_PROJECT, MockRebuildMode::Direct, MockBlockMode::Never);
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Unknown,
    );

    let meta = project.meta().await.unwrap();
    assert_eq!(meta.name, TEST_PROJECT);
    assert_eq!(meta.repositories.len(), 1);
    assert_eq!(meta.repositories[0].name, TEST_REPO);
    assert_eq!(meta.repositories[0].rebuild, RebuildMode::Direct);
    assert_eq!(meta.repositories[0].block, BlockMode::Never);
    assert_eq!(meta.repositories[0].arches.len(), 1);
    assert_eq!(meta.repositories[0].arches[0], TEST_ARCH_1);
}

#[tokio::test]
async fn test_project_package_delete() {
    let mock = start_mock().await;
    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );

    let obs = create_authenticated_client(mock.clone());

    let project = obs.project(TEST_PROJECT.to_owned());
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());

    package_1.list(None).await.unwrap();

    package_1.delete().await.unwrap();

    let err = package_1.list(None).await.unwrap_err();
    assert!(matches!(
        err,
        Error::ApiError(ApiError { code, .. }) if code == "unknown_package"
    ));

    project.meta().await.unwrap();

    project.delete().await.unwrap();

    let err = project.meta().await.unwrap_err();
    assert!(matches!(
        err,
        Error::ApiError(ApiError { code, .. }) if code == "unknown_project"
    ));
}

#[tokio::test]
async fn test_source_history() {
    let srcmd5 = random_md5();
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

    let mock = start_mock().await;
    mock.add_project(TEST_PROJECT.to_owned());

    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );

    let obs = create_authenticated_client(mock.clone());
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());

    let revisions = package_1.revisions().await.unwrap();
    assert_eq!(revisions.revisions.len(), 0);

    mock.add_package_revision(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockRevisionOptions {
            comment: None,
            srcmd5: srcmd5.clone(),
            time,
            user: ADMIN_USER.to_owned(),
            version: Some("version".to_owned()),
        },
        HashMap::new(),
    );

    let revisions = package_1.revisions().await.unwrap();
    assert_eq!(revisions.revisions.len(), 1);

    let rev = &revisions.revisions[0];
    assert_eq!(rev.comment, None);
    assert_eq!(rev.rev, "1");
    assert_eq!(rev.vrev, "1");
    assert_eq!(rev.version, "version");
    assert_eq!(rev.srcmd5, srcmd5);
    assert_eq!(rev.time, 10);
    assert_eq!(rev.user, ADMIN_USER);
}

#[tokio::test]
async fn test_source_list() {
    let mock = start_mock().await;
    mock.add_project(TEST_PROJECT.to_owned());

    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );

    let obs = create_authenticated_client(mock.clone());
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());
    let package_2 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_2.to_owned());

    let dir = package_1.list(None).await.unwrap();

    assert_eq!(dir.name, TEST_PACKAGE_1);
    assert!(dir.rev.is_none());
    assert!(dir.vrev.is_none());

    let meta_dir = package_1.list_meta(None).await.unwrap();

    assert_eq!(meta_dir.name, TEST_PACKAGE_1);
    assert_eq!(meta_dir.rev.unwrap(), "1");
    assert!(meta_dir.vrev.is_none());
    let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
    let srcmd5 = random_md5();
    mock.add_package_revision(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockRevisionOptions {
            time: mtime,
            srcmd5: srcmd5.clone(),
            ..Default::default()
        },
        HashMap::new(),
    );

    let dir = package_1.list(None).await.unwrap();

    assert_eq!(dir.name, TEST_PACKAGE_1);
    assert_eq!(dir.rev.unwrap(), "1");
    assert_eq!(dir.vrev.unwrap(), "1");
    assert_eq!(dir.srcmd5, srcmd5);

    assert_eq!(dir.entries.len(), 0);
    assert_eq!(dir.linkinfo.len(), 0);

    assert_eq!(meta_dir.entries.len(), 1);
    assert_eq!(meta_dir.linkinfo.len(), 0);

    let meta = &meta_dir.entries[0];
    assert_eq!(meta.name, "_meta");

    let test_data = b"abc";
    let test_key = mock.add_package_files(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockSourceFile {
            path: "test".to_owned(),
            contents: test_data.to_vec(),
        },
    );

    let srcmd5 = random_md5();
    mock.add_package_revision(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockRevisionOptions {
            srcmd5: srcmd5.clone(),
            ..Default::default()
        },
        [(
            "test".to_owned(),
            MockEntry::from_key(&test_key, SystemTime::now()),
        )]
        .into(),
    );

    let dir = package_1.list(None).await.unwrap();

    assert_eq!(dir.name, TEST_PACKAGE_1);
    assert_eq!(dir.rev.unwrap(), "2");
    assert_eq!(dir.vrev.unwrap(), "2");
    assert_eq!(dir.srcmd5, srcmd5);

    assert_eq!(dir.entries.len(), 1);

    let test_entry = &dir.entries[0];
    assert_eq!(test_entry.size, test_data.len() as u64);

    let dir = package_1.list(Some("1")).await.unwrap();

    assert_eq!(dir.rev.unwrap(), "1");
    assert_eq!(dir.entries.len(), 0);

    let branch_srcmd5 = random_md5();
    let branch_xsrcmd5 = random_md5();

    mock.branch(
        TEST_PROJECT.to_owned(),
        TEST_PACKAGE_1.to_owned(),
        TEST_PROJECT,
        TEST_PACKAGE_2.to_owned(),
        MockBranchOptions {
            srcmd5: branch_srcmd5.clone(),
            xsrcmd5: branch_xsrcmd5.clone(),
            ..Default::default()
        },
    );

    let dir = package_2.list(None).await.unwrap();

    assert_eq!(dir.rev.unwrap(), "1");
    assert_eq!(dir.vrev.unwrap(), "1");
    assert_eq!(dir.srcmd5, branch_srcmd5);
    assert_eq!(dir.entries.len(), 1);
    assert_eq!(dir.linkinfo.len(), 1);

    let linkinfo = &dir.linkinfo[0];
    assert_eq!(linkinfo.project, TEST_PROJECT);
    assert_eq!(linkinfo.package, TEST_PACKAGE_1);
    assert_eq!(linkinfo.srcmd5, srcmd5);
    assert_eq!(linkinfo.lsrcmd5, branch_srcmd5);
    assert_eq!(linkinfo.xsrcmd5, branch_xsrcmd5);
}

#[tokio::test]
async fn test_source_meta() {
    let mock = start_mock().await;
    mock.add_project(TEST_PROJECT.to_owned());

    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );

    let client = create_authenticated_client(mock.clone());

    let package_1 = client
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());

    let meta = package_1.meta().await.unwrap();
    assert_eq!(meta.project, TEST_PROJECT);
    assert_eq!(meta.name, TEST_PACKAGE_1);
    assert_eq!(meta.build.disabled.len(), 0);

    mock.set_package_metadata(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockPackageOptions {
            disabled: vec![
                MockPackageDisabledBuild {
                    arch: Some(TEST_ARCH_1.to_owned()),
                    ..Default::default()
                },
                MockPackageDisabledBuild {
                    repository: Some(TEST_REPO.to_owned()),
                    arch: Some(TEST_ARCH_2.to_owned()),
                },
            ],
            ..Default::default()
        },
    );

    let meta = package_1.meta().await.unwrap();
    assert_eq!(meta.project, TEST_PROJECT);
    assert_eq!(meta.name, TEST_PACKAGE_1);
    assert_eq!(meta.build.disabled.len(), 2);

    assert_eq!(meta.build.disabled[0].repository.as_deref(), None);
    assert_eq!(meta.build.disabled[0].arch.as_deref(), Some(TEST_ARCH_1));
    assert_eq!(
        meta.build.disabled[1].repository.as_deref(),
        Some(TEST_REPO)
    );
    assert_eq!(meta.build.disabled[1].arch.as_deref(), Some(TEST_ARCH_2));
}

#[tokio::test]
async fn test_source_get() {
    let test_file = "test";
    let test_contents = b"some file contents here";

    let mock = start_mock().await;
    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );

    let test_key = mock.add_package_files(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockSourceFile {
            path: test_file.to_owned(),
            contents: test_contents.to_vec(),
        },
    );

    mock.add_package_revision(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockRevisionOptions::default(),
        [(
            test_file.to_owned(),
            MockEntry::from_key(&test_key, SystemTime::now()),
        )]
        .into(),
    );

    let obs = create_authenticated_client(mock);

    let mut data = Vec::new();
    obs.project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned())
        .source_file(test_file)
        .await
        .unwrap()
        .try_for_each(|chunk| {
            data.extend_from_slice(&chunk);
            futures::future::ready(Ok(()))
        })
        .await
        .unwrap();
    assert_eq!(&data[..], test_contents);
}

#[tokio::test]
async fn test_commits() {
    let test_file = "test";
    let test_contents = b"some file contents here";
    let test_entry = CommitEntry::from_contents(test_file.to_owned(), test_contents);

    let file_list = CommitFileList::new().entry(test_entry.clone());

    let mock = start_mock().await;

    mock.add_project(TEST_PROJECT.to_owned());

    let obs = create_authenticated_client(mock);
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());

    package_1.create().await.unwrap();

    let commit_result = package_1
        .commit(&file_list, &CommitOptions::default())
        .await
        .unwrap();
    if let CommitResult::MissingEntries(missing) = commit_result {
        assert_eq!(missing.entries.len(), 1);
        assert_eq!(missing.entries[0].name, test_entry.name);
        assert_eq!(missing.entries[0].md5, test_entry.md5);
    } else {
        panic!("Expected missing entries, got {:?}", commit_result);
    }

    package_1
        .upload_for_commit(test_file, test_contents.to_vec())
        .await
        .unwrap();

    let commit_result = package_1
        .commit(
            &file_list,
            &CommitOptions {
                comment: Some("test comment".to_owned()),
            },
        )
        .await
        .unwrap();
    if let CommitResult::Success(directory) = commit_result {
        assert_eq!(directory.entries.len(), 1);
        assert_eq!(directory.entries[0].name, test_entry.name);
        assert_eq!(directory.entries[0].md5, test_entry.md5);
    } else {
        panic!("Expected missing entries, got {:?}", commit_result);
    }

    let directory = package_1.list(None).await.unwrap();
    assert_eq!(directory.entries.len(), 1);
    assert_eq!(directory.entries[0].name, test_entry.name);
    assert_eq!(directory.entries[0].md5, test_entry.md5);

    let revisions = package_1.revisions().await.unwrap();
    assert_eq!(
        revisions.revisions.last().unwrap().comment.as_deref(),
        Some("test comment")
    );
}

#[tokio::test]
async fn test_branch() {
    let test_file = "test";
    let test_contents = b"some file contents here";

    let test_project_branched_1 = format!("home:{}:branches:{}", DEFAULT_USERNAME, TEST_PROJECT);
    let test_project_branched_2 = format!("{}:branch", TEST_PROJECT);

    let mock = start_mock().await;
    mock.add_project(TEST_PROJECT.to_owned());
    mock.set_project_modes(
        TEST_PROJECT,
        MockRebuildMode::Transitive,
        MockBlockMode::All,
    );
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Finished,
    );

    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );

    let test_key = mock.add_package_files(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockSourceFile {
            path: test_file.to_owned(),
            contents: test_contents.to_vec(),
        },
    );

    mock.add_package_revision(
        TEST_PROJECT,
        TEST_PACKAGE_1,
        MockRevisionOptions::default(),
        [(
            test_file.to_owned(),
            MockEntry::from_key(&test_key, SystemTime::now()),
        )]
        .into(),
    );

    let obs = create_authenticated_client(mock);
    let branched_project_1 = obs.project(test_project_branched_1.clone());
    let branched_project_2 = obs.project(test_project_branched_2.clone());
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());
    let package_2 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_2.to_owned());
    let branched_package_1_1 = obs
        .project(test_project_branched_1.clone())
        .package(TEST_PACKAGE_1.to_owned());
    let branched_package_1_2 = obs
        .project(test_project_branched_1.clone())
        .package(TEST_PACKAGE_2.to_owned());
    let branched_package_2_1 = obs
        .project(test_project_branched_2.clone())
        .package(TEST_PACKAGE_1.to_owned());

    let status = package_1.branch(&BranchOptions::default()).await.unwrap();
    assert_eq!(status.source_project, TEST_PROJECT);
    assert_eq!(status.source_package, TEST_PACKAGE_1);
    assert_eq!(status.target_project, test_project_branched_1);
    assert_eq!(status.target_package, TEST_PACKAGE_1);

    let dir = branched_package_1_1.list(None).await.unwrap();
    assert_eq!(dir.linkinfo.len(), 1);
    assert_eq!(dir.linkinfo[0].project, TEST_PROJECT);
    assert_eq!(dir.linkinfo[0].package, TEST_PACKAGE_1);
    assert!(!dir.linkinfo[0].missingok);

    let meta = branched_project_1.meta().await.unwrap();
    assert_eq!(meta.repositories.len(), 1);
    assert_eq!(meta.repositories[0].name, TEST_REPO);
    assert_eq!(meta.repositories[0].rebuild, RebuildMode::Transitive);
    assert_eq!(meta.repositories[0].block, BlockMode::All);

    let err = package_1
        .branch(&BranchOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::ApiError(ApiError { code,.. }) if code == "double_branch_package"
    ));

    let status = package_1
        .branch(&BranchOptions {
            force: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(status.source_project, TEST_PROJECT);
    assert_eq!(status.source_package, TEST_PACKAGE_1);
    assert_eq!(status.target_project, test_project_branched_1);
    assert_eq!(status.target_package, TEST_PACKAGE_1);

    let status = package_1
        .branch(&BranchOptions {
            target_project: Some(test_project_branched_2.clone()),
            add_repositories_rebuild: Some(RebuildMode::Local),
            add_repositories_block: Some(BlockMode::Never),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(status.source_project, TEST_PROJECT);
    assert_eq!(status.source_package, TEST_PACKAGE_1);
    assert_eq!(status.target_project, test_project_branched_2);
    assert_eq!(status.target_package, TEST_PACKAGE_1);

    let dir = branched_package_2_1.list(None).await.unwrap();
    assert_eq!(dir.linkinfo.len(), 1);
    assert_eq!(dir.linkinfo[0].project, TEST_PROJECT);
    assert_eq!(dir.linkinfo[0].package, TEST_PACKAGE_1);
    assert!(!dir.linkinfo[0].missingok);

    let meta = branched_project_2.meta().await.unwrap();
    assert_eq!(meta.repositories.len(), 1);
    assert_eq!(meta.repositories[0].name, TEST_REPO);
    assert_eq!(meta.repositories[0].rebuild, RebuildMode::Local);
    assert_eq!(meta.repositories[0].block, BlockMode::Never);

    let status = package_2
        .branch(&BranchOptions {
            missingok: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(status.source_project, TEST_PROJECT);
    assert_eq!(status.source_package, TEST_PACKAGE_2);
    assert_eq!(status.target_project, test_project_branched_1);
    assert_eq!(status.target_package, TEST_PACKAGE_2);

    let dir = branched_package_1_2.list(None).await.unwrap();
    assert_eq!(dir.linkinfo.len(), 1);
    assert_eq!(dir.linkinfo[0].project, TEST_PROJECT);
    assert_eq!(dir.linkinfo[0].package, TEST_PACKAGE_2);
    assert!(dir.linkinfo[0].missingok);
}

fn get_results_by_arch(mut results: ResultList) -> (ResultListResult, ResultListResult) {
    assert_eq!(results.results.len(), 2);

    // Sort by the arch, so we know arch 1 is first and arch 2 is second.
    results.results.sort_by_key(|result| result.arch.clone());

    let mut it = results.results.into_iter();
    let a = it.next().unwrap();
    let b = it.next().unwrap();

    assert_eq!(a.arch, TEST_ARCH_1);
    assert_eq!(b.arch, TEST_ARCH_2);

    (a, b)
}

#[tokio::test]
async fn test_build_repo_listing() {
    let mock = start_mock().await;

    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Building,
    );
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_2.to_owned(),
        MockRepositoryCode::Broken,
    );

    let obs = create_authenticated_client(mock.clone());
    let project = obs.project(TEST_PROJECT.to_owned());

    let repositories = project.repositories().await.unwrap();
    assert_eq!(&repositories[..], &[TEST_REPO]);

    let mut arches = project.arches(TEST_REPO).await.unwrap();
    arches.sort();
    assert_eq!(&arches[..], &[TEST_ARCH_1, TEST_ARCH_2]);
}

#[tokio::test]
async fn test_build_jobhist() {
    let mock = start_mock().await;

    let rev_1_1 = "1";
    let srcmd5_1_1 = random_md5();
    let versrel_1_1 = "1.2-3";
    let readytime_1_1 = 123;
    let starttime_1_1 = 456;
    let endtime_1_1 = 789;
    let uri_1_1 = "http://127.0.0.1:9011";
    let worker_1_1 = "worker:1";
    let reason_1_1 = "reason 1";
    let verifymd5_1_1 = random_md5();

    let rev_1_2 = "2";
    let srcmd5_1_2 = random_md5();
    let versrel_1_2 = "3.2-1";
    let readytime_1_2 = 321;
    let starttime_1_2 = 654;
    let endtime_1_2 = 987;
    let uri_1_2 = "http://127.0.0.1:9012";
    let worker_1_2 = "worker:2";
    let reason_1_2 = "reason 2";
    let verifymd5_1_2 = random_md5();

    let rev_2_1 = "2";
    let srcmd5_2_1 = random_md5();
    let versrel_2_1 = "1.3-2";
    let readytime_2_1 = 132;
    let starttime_2_1 = 465;
    let endtime_2_1 = 798;
    let uri_2_1 = "http://127.0.0.1:9021";
    let worker_2_1 = "worker:3";
    let reason_2_1 = "reason 3";
    let verifymd5_2_1 = random_md5();

    let rev_2_2 = "2";
    let srcmd5_2_2 = random_md5();
    let versrel_2_2 = "3.1-2";
    let readytime_2_2 = 312;
    let starttime_2_2 = 645;
    let endtime_2_2 = 978;
    let uri_2_2 = "http://127.0.0.1:9022";
    let worker_2_2 = "worker:4";
    let reason_2_2 = "reason 4";
    let verifymd5_2_2 = random_md5();

    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Building,
    );
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_2.to_owned(),
        MockRepositoryCode::Building,
    );
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_2.to_owned(),
        MockPackageOptions::default(),
    );

    mock.add_job_history(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        MockJobHistoryEntry {
            package: TEST_PACKAGE_1.to_owned(),
            rev: rev_1_1.to_owned(),
            srcmd5: srcmd5_1_1.clone(),
            versrel: versrel_1_1.to_owned(),
            bcnt: 1,
            readytime: SystemTime::UNIX_EPOCH + Duration::from_secs(readytime_1_1),
            starttime: SystemTime::UNIX_EPOCH + Duration::from_secs(starttime_1_1),
            endtime: SystemTime::UNIX_EPOCH + Duration::from_secs(endtime_1_1),
            code: MockPackageCode::Failed,
            uri: uri_1_1.to_owned(),
            workerid: worker_1_1.to_owned(),
            hostarch: TEST_ARCH_1.to_string(),
            reason: reason_1_1.to_owned(),
            verifymd5: verifymd5_1_1.clone(),
        },
    );

    mock.add_job_history(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_2,
        MockJobHistoryEntry {
            package: TEST_PACKAGE_1.to_owned(),
            rev: rev_1_2.to_owned(),
            srcmd5: srcmd5_1_2.clone(),
            versrel: versrel_1_2.to_owned(),
            bcnt: 12,
            readytime: SystemTime::UNIX_EPOCH + Duration::from_secs(readytime_1_2),
            starttime: SystemTime::UNIX_EPOCH + Duration::from_secs(starttime_1_2),
            endtime: SystemTime::UNIX_EPOCH + Duration::from_secs(endtime_1_2),
            code: MockPackageCode::Succeeded,
            uri: uri_1_2.to_owned(),
            workerid: worker_1_2.to_owned(),
            hostarch: TEST_ARCH_2.to_string(),
            reason: reason_1_2.to_owned(),
            verifymd5: verifymd5_1_2.clone(),
        },
    );

    mock.add_job_history(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        MockJobHistoryEntry {
            package: TEST_PACKAGE_2.to_owned(),
            rev: rev_2_1.to_owned(),
            srcmd5: srcmd5_2_1.clone(),
            versrel: versrel_2_1.to_owned(),
            bcnt: 21,
            readytime: SystemTime::UNIX_EPOCH + Duration::from_secs(readytime_2_1),
            starttime: SystemTime::UNIX_EPOCH + Duration::from_secs(starttime_2_1),
            endtime: SystemTime::UNIX_EPOCH + Duration::from_secs(endtime_2_1),
            code: MockPackageCode::Succeeded,
            uri: uri_2_1.to_owned(),
            workerid: worker_2_1.to_owned(),
            hostarch: TEST_ARCH_2.to_string(),
            reason: reason_2_1.to_owned(),
            verifymd5: verifymd5_2_1.clone(),
        },
    );

    mock.add_job_history(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        MockJobHistoryEntry {
            package: TEST_PACKAGE_2.to_owned(),
            rev: rev_2_2.to_owned(),
            srcmd5: srcmd5_2_2.clone(),
            versrel: versrel_2_2.to_owned(),
            bcnt: 22,
            readytime: SystemTime::UNIX_EPOCH + Duration::from_secs(readytime_2_2),
            starttime: SystemTime::UNIX_EPOCH + Duration::from_secs(starttime_2_2),
            endtime: SystemTime::UNIX_EPOCH + Duration::from_secs(endtime_2_2),
            code: MockPackageCode::Failed,
            uri: uri_2_2.to_owned(),
            workerid: worker_2_2.to_owned(),
            hostarch: TEST_ARCH_1.to_string(),
            reason: reason_2_2.to_owned(),
            verifymd5: verifymd5_2_2.clone(),
        },
    );

    let obs = create_authenticated_client(mock.clone());
    let project = obs.project(TEST_PROJECT.to_owned());

    let mut jobhist = project
        .jobhistory(TEST_REPO, TEST_ARCH_1, &JobHistoryFilters::empty())
        .await
        .unwrap();

    assert_eq!(jobhist.jobhist.len(), 3);
    jobhist.jobhist.sort_by(|a, b| a.package.cmp(&b.package));

    assert_eq!(jobhist.jobhist[0].package, TEST_PACKAGE_1);
    assert_eq!(jobhist.jobhist[0].rev, rev_1_1);
    assert_eq!(jobhist.jobhist[0].srcmd5, srcmd5_1_1);
    assert_eq!(jobhist.jobhist[0].versrel, versrel_1_1);
    assert_eq!(jobhist.jobhist[0].bcnt, "1");
    assert_eq!(jobhist.jobhist[0].readytime, readytime_1_1);
    assert_eq!(jobhist.jobhist[0].starttime, starttime_1_1);
    assert_eq!(jobhist.jobhist[0].endtime, endtime_1_1);
    assert_eq!(jobhist.jobhist[0].code, PackageCode::Failed);
    assert_eq!(jobhist.jobhist[0].uri, uri_1_1);
    assert_eq!(jobhist.jobhist[0].workerid, worker_1_1);
    assert_eq!(jobhist.jobhist[0].hostarch, TEST_ARCH_1);
    assert_eq!(jobhist.jobhist[0].reason, reason_1_1);
    assert_eq!(jobhist.jobhist[0].verifymd5, verifymd5_1_1);

    assert_eq!(jobhist.jobhist[1].package, TEST_PACKAGE_2);
    assert_eq!(jobhist.jobhist[1].rev, rev_2_1);
    assert_eq!(jobhist.jobhist[1].srcmd5, srcmd5_2_1);
    assert_eq!(jobhist.jobhist[1].versrel, versrel_2_1);
    assert_eq!(jobhist.jobhist[1].bcnt, "21");
    assert_eq!(jobhist.jobhist[1].readytime, readytime_2_1);
    assert_eq!(jobhist.jobhist[1].starttime, starttime_2_1);
    assert_eq!(jobhist.jobhist[1].endtime, endtime_2_1);
    assert_eq!(jobhist.jobhist[1].code, PackageCode::Succeeded);
    assert_eq!(jobhist.jobhist[1].uri, uri_2_1);
    assert_eq!(jobhist.jobhist[1].workerid, worker_2_1);
    assert_eq!(jobhist.jobhist[1].hostarch, TEST_ARCH_2);
    assert_eq!(jobhist.jobhist[1].reason, reason_2_1);
    assert_eq!(jobhist.jobhist[1].verifymd5, verifymd5_2_1);

    assert_eq!(jobhist.jobhist[2].package, TEST_PACKAGE_2);
    assert_eq!(jobhist.jobhist[2].rev, rev_2_2);
    assert_eq!(jobhist.jobhist[2].srcmd5, srcmd5_2_2);
    assert_eq!(jobhist.jobhist[2].versrel, versrel_2_2);
    assert_eq!(jobhist.jobhist[2].bcnt, "22");
    assert_eq!(jobhist.jobhist[2].readytime, readytime_2_2);
    assert_eq!(jobhist.jobhist[2].starttime, starttime_2_2);
    assert_eq!(jobhist.jobhist[2].endtime, endtime_2_2);
    assert_eq!(jobhist.jobhist[2].code, PackageCode::Failed);
    assert_eq!(jobhist.jobhist[2].uri, uri_2_2);
    assert_eq!(jobhist.jobhist[2].workerid, worker_2_2);
    assert_eq!(jobhist.jobhist[2].hostarch, TEST_ARCH_1);
    assert_eq!(jobhist.jobhist[2].reason, reason_2_2);
    assert_eq!(jobhist.jobhist[2].verifymd5, verifymd5_2_2);

    let jobhist = project
        .jobhistory(
            TEST_REPO,
            TEST_ARCH_1,
            &JobHistoryFilters::only_package(TEST_PACKAGE_2.to_owned()),
        )
        .await
        .unwrap();

    assert_eq!(jobhist.jobhist.len(), 2);

    assert_eq!(jobhist.jobhist[0].package, TEST_PACKAGE_2);
    assert_eq!(jobhist.jobhist[0].rev, rev_2_1);
    assert_eq!(jobhist.jobhist[1].package, TEST_PACKAGE_2);
    assert_eq!(jobhist.jobhist[1].rev, rev_2_2);

    let jobhist = project
        .jobhistory(
            TEST_REPO,
            TEST_ARCH_1,
            &JobHistoryFilters::empty()
                .package(TEST_PACKAGE_2.to_owned())
                .limit(Some(1)),
        )
        .await
        .unwrap();

    assert_eq!(jobhist.jobhist.len(), 1);

    assert_eq!(jobhist.jobhist[0].package, TEST_PACKAGE_2);
    assert_eq!(jobhist.jobhist[0].rev, rev_2_1);

    let mut jobhist = project
        .jobhistory(
            TEST_REPO,
            TEST_ARCH_1,
            &JobHistoryFilters::empty().code(PackageCode::Failed),
        )
        .await
        .unwrap();

    assert_eq!(jobhist.jobhist.len(), 2);
    jobhist.jobhist.sort_by(|a, b| a.package.cmp(&b.package));

    assert_eq!(jobhist.jobhist[0].package, TEST_PACKAGE_1);
    assert_eq!(jobhist.jobhist[0].rev, rev_1_1);
    assert_eq!(jobhist.jobhist[1].package, TEST_PACKAGE_2);
    assert_eq!(jobhist.jobhist[1].rev, rev_2_2);

    let jobhist = project
        .jobhistory(
            TEST_REPO,
            TEST_ARCH_1,
            &JobHistoryFilters::empty()
                .package(TEST_PACKAGE_1.to_owned())
                .code(PackageCode::Failed),
        )
        .await
        .unwrap();

    assert_eq!(jobhist.jobhist.len(), 1);

    assert_eq!(jobhist.jobhist[0].package, TEST_PACKAGE_1);
    assert_eq!(jobhist.jobhist[0].rev, rev_1_1);

    let jobhist = project
        .jobhistory(
            TEST_REPO,
            TEST_ARCH_1,
            &JobHistoryFilters::only_package(TEST_PACKAGE_1.to_owned())
                .code(PackageCode::Succeeded),
        )
        .await
        .unwrap();

    assert_eq!(jobhist.jobhist.len(), 0);

    let jobhist = project
        .jobhistory(TEST_REPO, TEST_ARCH_2, &JobHistoryFilters::empty())
        .await
        .unwrap();

    assert_eq!(jobhist.jobhist.len(), 1);

    assert_eq!(jobhist.jobhist[0].package, TEST_PACKAGE_1);
    assert_eq!(jobhist.jobhist[0].rev, rev_1_2);
    assert_eq!(jobhist.jobhist[0].srcmd5, srcmd5_1_2);
    assert_eq!(jobhist.jobhist[0].versrel, versrel_1_2);
    assert_eq!(jobhist.jobhist[0].bcnt, "12");
    assert_eq!(jobhist.jobhist[0].readytime, readytime_1_2);
    assert_eq!(jobhist.jobhist[0].starttime, starttime_1_2);
    assert_eq!(jobhist.jobhist[0].endtime, endtime_1_2);
    assert_eq!(jobhist.jobhist[0].code, PackageCode::Succeeded);
    assert_eq!(jobhist.jobhist[0].uri, uri_1_2);
    assert_eq!(jobhist.jobhist[0].workerid, worker_1_2);
    assert_eq!(jobhist.jobhist[0].hostarch, TEST_ARCH_2);
    assert_eq!(jobhist.jobhist[0].reason, reason_1_2);
    assert_eq!(jobhist.jobhist[0].verifymd5, verifymd5_1_2);
}

#[tokio::test]
async fn test_build_results() {
    let details = "some details";

    let mock = start_mock().await;

    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Building,
    );
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_2.to_owned(),
        MockRepositoryCode::Broken,
    );

    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_2.to_owned(),
        MockPackageOptions::default(),
    );

    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        MockBuildStatus::new(MockPackageCode::Building),
    );

    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_2,
        TEST_PACKAGE_2.to_owned(),
        MockBuildStatus {
            code: MockPackageCode::Broken,
            details: details.to_owned(),
            dirty: true,
        },
    );

    let obs = create_authenticated_client(mock.clone());
    let project = obs.project(TEST_PROJECT.to_owned());
    let package_2 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_2.to_owned());

    let results = project.result().await.unwrap();
    let (arch1_repo, arch2_repo) = get_results_by_arch(results);

    assert_eq!(arch1_repo.project, TEST_PROJECT);
    assert_eq!(arch1_repo.repository, TEST_REPO);
    assert_eq!(arch1_repo.code, RepositoryCode::Building);
    assert_eq!(arch1_repo.statuses.len(), 1);

    let package1_status = &arch1_repo.statuses[0];
    assert_eq!(package1_status.package, TEST_PACKAGE_1);
    assert_eq!(package1_status.code, PackageCode::Building);
    assert!(!package1_status.dirty);

    assert_eq!(arch2_repo.project, TEST_PROJECT);
    assert_eq!(arch2_repo.repository, TEST_REPO);
    assert_eq!(arch2_repo.code, RepositoryCode::Broken);
    assert_eq!(arch2_repo.statuses.len(), 1);

    let package2_status = &arch2_repo.statuses[0];
    assert_eq!(package2_status.package, TEST_PACKAGE_2);
    assert_eq!(package2_status.code, PackageCode::Broken);
    assert_eq!(package2_status.details.as_ref().unwrap(), details);
    assert!(package2_status.dirty);

    let results = package_2.result().await.unwrap();
    let (arch1_repo, arch2_repo) = get_results_by_arch(results);

    assert_eq!(arch1_repo.statuses.len(), 0);
    assert_eq!(arch2_repo.statuses.len(), 1);

    let package2_status = &arch2_repo.statuses[0];
    assert_eq!(package2_status.package, TEST_PACKAGE_2);
    assert_eq!(package2_status.code, PackageCode::Broken);
    assert!(package2_status.dirty);

    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_2.to_owned(),
        MockBuildStatus::new(MockPackageCode::Broken),
    );

    let results = project.result().await.unwrap();
    let (arch1_repo, _) = get_results_by_arch(results);

    let package2_arch2 = arch1_repo
        .statuses
        .iter()
        .find(|status| status.package == TEST_PACKAGE_2)
        .unwrap();
    assert_eq!(package2_arch2.package, TEST_PACKAGE_2);
    assert_eq!(package2_arch2.code, PackageCode::Broken);

    let results = package_2.result().await.unwrap();
    let (arch1_repo, arch2_repo) = get_results_by_arch(results);

    assert_eq!(arch1_repo.statuses.len(), 1);
    assert_eq!(arch2_repo.statuses.len(), 1);

    assert_eq!(arch1_repo.statuses[0].package, TEST_PACKAGE_2);
    assert_eq!(arch2_repo.statuses[0].package, TEST_PACKAGE_2);
}

#[tokio::test]
async fn test_build_binaries() {
    let test_file = "test";
    let test_contents = b"abc 123";
    let test_mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

    let mock = start_mock().await;

    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Finished,
    );

    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );
    mock.set_package_binaries(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        HashMap::new(),
    );

    let obs = create_authenticated_client(mock.clone());
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());

    let binaries = package_1.binaries(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(binaries.binaries.len(), 0);

    mock.set_package_binaries(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        [(
            test_file.to_owned(),
            MockBinary {
                contents: test_contents.to_vec(),
                mtime: test_mtime,
            },
        )]
        .into(),
    );

    let binaries = package_1.binaries(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(binaries.binaries.len(), 1);

    assert_eq!(binaries.binaries[0].filename, test_file);
    assert_eq!(binaries.binaries[0].size, test_contents.len() as u64);
    assert_eq!(
        binaries.binaries[0].mtime,
        test_mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    let mut data = Vec::new();
    package_1
        .binary_file(TEST_REPO, TEST_ARCH_1, test_file)
        .await
        .unwrap()
        .try_for_each(|chunk| {
            data.extend_from_slice(&chunk);
            futures::future::ready(Ok(()))
        })
        .await
        .unwrap();

    assert_eq!(&data[..], test_contents);
}

#[tokio::test]
async fn test_build_status() {
    let details = "details";

    let mock = start_mock().await;

    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Building,
    );
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );
    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        MockBuildStatus::new(MockPackageCode::Building),
    );

    let obs = create_authenticated_client(mock.clone());
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());

    let status = package_1.status(TEST_REPO, TEST_ARCH_1).await.unwrap();

    assert_eq!(status.package, TEST_PACKAGE_1);
    assert_eq!(status.code, PackageCode::Building);
    assert!(!status.dirty);

    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        MockBuildStatus {
            code: MockPackageCode::Unknown,
            details: details.to_owned(),
            dirty: true,
        },
    );

    let status = package_1.status(TEST_REPO, TEST_ARCH_1).await.unwrap();

    assert_eq!(status.package, TEST_PACKAGE_1);
    assert_eq!(status.code, PackageCode::Unknown);
    assert_eq!(status.details.unwrap(), details);
    assert!(status.dirty);
}

#[tokio::test]
async fn test_build_rebuild() {
    let mock = start_mock().await;

    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_2.to_owned(),
        MockPackageOptions::default(),
    );

    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Building,
    );

    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        MockBuildStatus::new(MockPackageCode::Blocked),
    );
    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_2.to_owned(),
        MockBuildStatus::new(MockPackageCode::Blocked),
    );
    mock.set_package_build_status_for_rebuilds(
        TEST_PROJECT,
        MockBuildStatus::new(MockPackageCode::Building),
    );

    let obs = create_authenticated_client(mock.clone());
    let project = obs.project(TEST_PROJECT.to_owned());
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());
    let package_2 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_2.to_owned());

    let status = package_1.status(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(status.code, PackageCode::Blocked);

    package_1.rebuild().await.unwrap();

    let status = package_1.status(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(status.code, PackageCode::Building);

    let status = package_2.status(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(status.code, PackageCode::Blocked);

    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        MockBuildStatus::new(MockPackageCode::Blocked),
    );
    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_2.to_owned(),
        MockBuildStatus::new(MockPackageCode::Blocked),
    );

    project.rebuild(&RebuildFilters::empty()).await.unwrap();

    let status = package_1.status(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(status.code, PackageCode::Building);

    let status = package_2.status(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(status.code, PackageCode::Building);

    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        MockBuildStatus::new(MockPackageCode::Blocked),
    );
    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_2.to_owned(),
        MockBuildStatus::new(MockPackageCode::Blocked),
    );

    project
        .rebuild(&RebuildFilters::only_package(TEST_PACKAGE_2.to_owned()))
        .await
        .unwrap();

    let status = package_1.status(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(status.code, PackageCode::Blocked);

    let status = package_2.status(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(status.code, PackageCode::Building);
}

#[tokio::test]
async fn test_build_history() {
    let mock = start_mock().await;

    let rev = "6";
    let srcmd5 = random_md5();
    let versrel = "0.0.1-1";
    let bcnt = 2;
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
    let duration = Duration::from_secs(15);

    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );

    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Finished,
    );
    mock.set_package_build_status(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        MockBuildStatus::new(MockPackageCode::Finished),
    );

    let client = create_authenticated_client(mock.clone());
    let package_1 = client
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());

    let history = package_1.history(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(history.entries.len(), 0);

    mock.add_build_history(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        MockBuildHistoryEntry {
            rev: rev.to_owned(),
            srcmd5: srcmd5.clone(),
            versrel: versrel.to_owned(),
            bcnt,
            time,
            duration,
        },
    );

    let history = package_1.history(TEST_REPO, TEST_ARCH_1).await.unwrap();
    assert_eq!(history.entries.len(), 1);

    assert_eq!(history.entries[0].rev, rev);
    assert_eq!(history.entries[0].srcmd5, srcmd5);
    assert_eq!(history.entries[0].versrel, versrel);
    assert_eq!(history.entries[0].bcnt, bcnt.to_string());
    assert_eq!(
        history.entries[0].time,
        time.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    );
    assert_eq!(history.entries[0].duration, duration.as_secs().to_string());
}

#[tokio::test]
async fn test_build_logs() {
    let log = MockBuildLog {
        contents: "some log text".to_owned(),
        mtime: SystemTime::UNIX_EPOCH,
        chunk_size: Some(5),
    };

    let mock = start_mock().await;

    mock.add_project(TEST_PROJECT.to_owned());
    mock.add_or_update_repository(
        TEST_PROJECT,
        TEST_REPO.to_owned(),
        TEST_ARCH_1.to_owned(),
        MockRepositoryCode::Building,
    );
    mock.add_new_package(
        TEST_PROJECT,
        TEST_PACKAGE_1.to_owned(),
        MockPackageOptions::default(),
    );
    mock.add_completed_build_log(
        TEST_PROJECT,
        TEST_REPO,
        TEST_ARCH_1,
        TEST_PACKAGE_1.to_owned(),
        log.clone(),
        false,
    );

    let obs = create_authenticated_client(mock.clone());
    let package_1 = obs
        .project(TEST_PROJECT.to_owned())
        .package(TEST_PACKAGE_1.to_owned());

    let (size, mtime) = package_1.log(TEST_REPO, TEST_ARCH_1).entry().await.unwrap();

    assert_eq!(size, log.contents.len());
    assert_eq!(mtime, 0);

    let mut stream = package_1
        .log(TEST_REPO, TEST_ARCH_1)
        .stream(Default::default())
        .unwrap();

    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b"some ");
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b"log t");
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b"ext");
    assert!(stream.next().await.is_none());

    let mut stream = package_1
        .log(TEST_REPO, TEST_ARCH_1)
        .stream(PackageLogStreamOptions {
            offset: Some(4),
            end: Some(11),
        })
        .unwrap();

    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b" log ");
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk.as_ref(), b"te");
    assert!(stream.next().await.is_none());
}
