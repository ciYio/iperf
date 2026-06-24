mod error;
mod config;
mod backend;
mod benchmark;
mod metrics;
mod report;
mod cli;
mod cmd_run;
mod cmd_config;
mod download;
mod hub;

use clap::Parser;
use cli::{Cli, Commands, HubCommands};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = match option_env!("IPERF_COMMIT") {
    Some(v) => v,
    None => "unknown",
};
pub const BUILT: &str = match option_env!("IPERF_BUILT") {
    Some(v) => v,
    None => "unknown",
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    backend::init_backends();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => cmd_run::run(args).await?,
        Commands::Config(args) => cmd_config::run(args)?,
        Commands::Hub(args) => match args.command {
            HubCommands::Download(dl_args) => cmd_hub_download(dl_args).await?,
            HubCommands::Serve(srv_args) => {
                hub::serve(&srv_args.local_dir, &srv_args.addr).await?;
            }
        },
    }

    Ok(())
}

async fn cmd_hub_download(args: cli::HubDownloadArgs) -> anyhow::Result<()> {
    use std::path::PathBuf;
    use download::{Downloader, HubDownloader};

    let dest_dir = args.local_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("./models/{}", args.model_id.replace('/', "_"))));

    if let Some(ref source) = args.source {
        let mut dl = HubDownloader::new(
            source,
            &args.model_id,
            &dest_dir,
            args.http_proxy.as_deref(),
        );
        dl.offset = args.offset;
        dl.count = args.count;
        dl.download_all().await?;
    } else {
        let mut dl = Downloader::new(
            &args.model_id,
            &args.revision,
            &dest_dir,
            args.http_proxy.as_deref(),
        );
        dl.offset = args.offset;
        dl.count = args.count;
        dl.download_all().await?;
    }

    println!("Download complete: {}", dest_dir.display());
    Ok(())
}
