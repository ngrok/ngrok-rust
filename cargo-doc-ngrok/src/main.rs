use std::{
    io,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
};

use axum::BoxError;
use clap::{
    Args,
    Parser,
    Subcommand,
};
use futures::TryStreamExt;
use hyper::service::service_fn;
use hyper_util::{
    rt::TokioExecutor,
    server,
};
use ngrok::prelude::*;
use watchexec::{
    action::{
        Action,
        Outcome,
    },
    command::Command,
    config::{
        InitConfig,
        RuntimeConfig,
    },
    error::CriticalError,
    handler::PrintDebug,
    signal::source::MainSignal,
    Watchexec,
};

#[derive(Parser, Debug)]
struct Cargo {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    DocNgrok(DocNgrok),
}

#[derive(Debug, Args)]
struct DocNgrok {
    #[arg(short)]
    package: Option<String>,

    #[arg(long, short)]
    domain: Option<String>,

    #[arg(long, short)]
    watch: bool,

    #[arg(last = true)]
    doc_args: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let Cmd::DocNgrok(args) = Cargo::parse().cmd;

    std::process::Command::new("cargo")
        .arg("doc")
        .args(args.doc_args.iter())
        .stderr(Stdio::inherit())
        .stdout(Stdio::inherit())
        .spawn()?
        .wait()?;

    let meta = cargo_metadata::MetadataCommand::new().exec()?;

    let default_package = args
        .package
        .or(meta.root_package().map(|p| p.name.clone()))
        .ok_or("No default package found. You must provide one with -p")?;
    let root_dir = meta.workspace_root;
    let target_dir = meta.target_directory;
    let doc_dir = target_dir.join("doc");

    let sess = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?;

    let mut listen_cfg = sess.http_endpoint();
    if let Some(domain) = args.domain {
        listen_cfg.domain(domain);
    }

    let mut listener = listen_cfg.listen().await?;

    let service = service_fn(move |req| {
        let stat = hyper_staticfile::Static::new(&doc_dir);
        stat.serve(req)
    });

    println!(
        "serving docs on: {}/{}/",
        listener.url(),
        default_package.replace('-', "_")
    );

    let server = async move {
        let (dropref, waiter) = awaitdrop::awaitdrop();

        // Continuously accept new connections.
        while let Some(conn) = listener.try_next().await? {
            let service = service.clone();
            let dropref = dropref.clone();
            // Spawn a task to handle the connection. That way we can multiple connections
            // concurrently.
            tokio::spawn(async move {
                if let Err(err) = server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection(conn, service)
                    .await
                {
                    eprintln!("failed to serve connection: {err:#}");
                }
                drop(dropref);
            });
        }

        // Wait until all children have finished, not just the listener.
        drop(dropref);
        waiter.await;

        Ok::<(), BoxError>(())
    };

    if args.watch {
        let we = make_watcher(args.doc_args, root_dir, target_dir)?;

        we.main().await??;
    } else {
        server.await?;
    }

    Ok(())
}

fn make_watcher(
    args: Vec<String>,
    root_dir: impl Into<PathBuf>,
    target_dir: impl Into<PathBuf>,
) -> Result<Arc<Watchexec>, CriticalError> {
    let target_dir = target_dir.into();
    let root_dir = root_dir.into();
    let mut init = InitConfig::default();
    init.on_error(PrintDebug(std::io::stderr()));

    let mut runtime = RuntimeConfig::default();
    runtime.pathset([root_dir]);
    runtime.command(Command::Exec {
        prog: "cargo".into(),
        args: [String::from("doc")].into_iter().chain(args).collect(),
    });
    runtime.on_action({
        move |action: Action| {
            let target_dir = target_dir.clone();
            async move {
                let sigs = action
                    .events
                    .iter()
                    .flat_map(|event| event.signals())
                    .collect::<Vec<_>>();
                if sigs.iter().any(|sig| sig == &MainSignal::Interrupt) {
                    action.outcome(Outcome::Exit);
                } else if action
                    .events
                    .iter()
                    .any(|e| e.paths().any(|(p, _)| !p.starts_with(&target_dir)))
                {
                    action.outcome(Outcome::if_running(
                        Outcome::both(Outcome::Stop, Outcome::Start),
                        Outcome::Start,
                    ));
                }

                Result::<_, io::Error>::Ok(())
            }
        }
    });
    Watchexec::new(init, runtime)
}
