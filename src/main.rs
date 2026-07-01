mod error;
mod config;
mod backend;
mod benchmark;
mod metrics;
mod report;
mod cli;
mod cmd_run;
mod cmd_watch;
mod cmd_config;
mod download;
mod hub;
mod watch;

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

// Full version string for --version flag
pub fn version_string() -> String {
    format!("{} (commit: {}, built at: {})", VERSION, COMMIT, BUILT)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Handle --version before clap parsing
    if std::env::args().nth(1).as_deref() == Some("--version") ||
       std::env::args().nth(1).as_deref() == Some("-V") {
        println!("{}", version_string());
        return Ok(());
    }

    backend::init_backends();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => cmd_run::run(args).await?,
        Commands::Watch(args) => cmd_watch::run(args).await?,
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

    // Parse role parameter (e.g. "1/4", "2/4")
    let (offset, count) = if let Some(ref role) = args.role {
        let parts: Vec<&str> = role.split('/').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid role format: {}, expected 'N/M' (e.g. '1/4')", role);
        }
        let current: usize = parts[0].parse().map_err(|_| anyhow::anyhow!("Invalid role number: {}", parts[0]))?;
        let total: usize = parts[1].parse().map_err(|_| anyhow::anyhow!("Invalid role total: {}", parts[1]))?;
        if current == 0 || current > total {
            anyhow::bail!("Role {} out of range 1..={}", current, total);
        }
        // We'll calculate actual offset/count after getting file list
        // For now, return special values that will be handled later
        (current, total)
    } else {
        (0, 0)  // Not using role
    };

    if let Some(ref source) = args.source {
        let mut dl = HubDownloader::new(
            source,
            &args.model_id,
            &dest_dir,
            args.http_proxy.as_deref(),
        );
        if args.role.is_some() {
            dl.role = Some((offset, count));
        } else {
            dl.offset = args.offset;
            dl.count = args.count;
        }
        if args.check_only {
            dl.target = args.target.clone();
            let result = dl.check_files().await?;
            result.print_summary();
            if !result.is_ok() {
                std::process::exit(1);
            }
        } else {
            dl.download_all().await?;
            println!("Download complete: {}", dest_dir.display());
        }
    } else {
        let mut dl = Downloader::new(
            &args.model_id,
            &args.revision,
            &dest_dir,
            args.http_proxy.as_deref(),
        );
        if args.role.is_some() {
            dl.role = Some((offset, count));
        } else {
            dl.offset = args.offset;
            dl.count = args.count;
        }

        if args.check_only {
            dl.target = args.target.clone();
            let result = dl.check_files().await?;
            result.print_summary();
            if !result.is_ok() {
                std::process::exit(1);
            }
        } else {
            dl.download_all().await?;
            println!("Download complete: {}", dest_dir.display());
        }
    }

    Ok(())
}
