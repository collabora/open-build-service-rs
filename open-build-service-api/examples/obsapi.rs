use anyhow::{Context, Result};
use clap::Parser;
use futures::prelude::*;
use open_build_service_api::{Client, PackageLogStreamOptions};
use oscrc::Oscrc;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use url::Url;

#[derive(Parser, Debug)]
struct PackageFull {
    project: String,
    package: String,
    repository: String,
    arch: String,
}

#[derive(Parser, Debug)]
struct Package {
    project: String,
    package: String,
}

#[derive(Parser, Debug)]
struct BuildResult {
    project: String,
    package: Option<String>,
}

async fn jobstatus(client: Client, opts: PackageFull) -> Result<()> {
    let p = client.project(opts.project).package(opts.package);
    println!("{:#?}", p.jobstatus(&opts.repository, &opts.arch).await);
    Ok(())
}

async fn status(client: Client, opts: PackageFull) -> Result<()> {
    let p = client.project(opts.project).package(opts.package);
    println!("{:#?}", p.status(&opts.repository, &opts.arch).await);
    Ok(())
}

async fn history(client: Client, opts: PackageFull) -> Result<()> {
    let p = client.project(opts.project).package(opts.package);
    println!("{:#?}", p.history(&opts.repository, &opts.arch).await);
    Ok(())
}

async fn log(client: Client, opts: PackageFull) -> Result<()> {
    let p = client.project(opts.project).package(opts.package);
    let log = p.log(&opts.repository, &opts.arch);

    let (size, mtime) = log.entry().await?;
    println!("Log: size: {size}, mtime: {mtime}");

    let mut stdout = tokio::io::stdout();

    let mut stream = log.stream(PackageLogStreamOptions::default())?;
    while let Some(chunk) = stream.try_next().await? {
        stdout.write_all(&chunk).await?;
    }

    Ok(())
}

async fn list(client: Client, opts: Package) -> Result<()> {
    let p = client.project(opts.project).package(opts.package);
    println!("{:#?}", p.list(None).await);
    Ok(())
}

async fn result(client: Client, opts: BuildResult) -> Result<()> {
    let p = client.project(opts.project);
    if let Some(package) = opts.package {
        let p = p.package(package);
        println!("{:#?}", p.result().await);
    } else {
        println!("{:#?}", p.result().await);
    }

    Ok(())
}

#[derive(Parser, Debug)]
enum Command {
    Jobstatus(PackageFull),
    History(PackageFull),
    Status(PackageFull),
    Log(PackageFull),
    List(Package),
    Result(BuildResult),
}

#[derive(Parser)]
struct Opts {
    #[arg(long, short)]
    apiurl: Option<Url>,
    #[arg(long, short, default_value = "/home/sjoerd/.oscrc")]
    config: PathBuf,
    #[arg(long, short, requires = "pass")]
    user: Option<String>,
    #[arg(long, short, requires = "user")]
    pass: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    let (url, user, pass) = match opts {
        Opts {
            apiurl: Some(url),
            user: Some(user),
            pass: Some(pass),
            ..
        } => (url, user, pass),
        _ => {
            let oscrc = Oscrc::from_path(&opts.config)
                .with_context(|| format!("Couldn't open {:?}", opts.config))?;
            let url = opts
                .apiurl
                .unwrap_or_else(|| oscrc.default_service().clone());
            let (user, pass) = if let Some(user) = opts.user {
                // If user is set pass should be set as well
                (user, opts.pass.unwrap())
            } else {
                oscrc.credentials(&url)?
            };
            (url, user, pass)
        }
    };

    let client = Client::new(url, user, pass);
    match opts.command {
        Command::Jobstatus(o) => jobstatus(client, o).await,
        Command::Status(o) => status(client, o).await,
        Command::History(o) => history(client, o).await,
        Command::List(o) => list(client, o).await,
        Command::Log(o) => log(client, o).await,
        Command::Result(o) => result(client, o).await,
    }
}
