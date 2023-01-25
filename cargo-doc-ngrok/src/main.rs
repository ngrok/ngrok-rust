use std::{
    error::Error,
    io,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
};

use clap::{
    Args,
    Parser,
    Subcommand,
};
use hyper::service::make_service_fn;
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
async fn main() -> Result<(), Box<dyn Error>> {
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

    let mut tunnel_cfg = sess.http_endpoint();
    if let Some(domain) = args.domain {
        tunnel_cfg = tunnel_cfg.domain(domain);
    }

    let tunnel = tunnel_cfg.listen().await?;

    println!(
        "serving docs on: {}/{}/",
        tunnel.url(),
        default_package.replace('-', "_")
    );

    let srv = hyper::server::Server::builder(tunnel).serve(make_service_fn(move |_| {
        let stat = hyper_staticfile::Static::new(&doc_dir);
        async move { Result::<_, String>::Ok(stat) }
    }));

    if args.watch {
        let we = make_watcher(args.doc_args, root_dir, target_dir)?;
        tokio::spawn(srv);

        we.main().await??;
    } else {
        srv.await?;
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
        args: [String::from("doc")]
            .into_iter()
            .chain(args.into_iter())
            .collect(),
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
