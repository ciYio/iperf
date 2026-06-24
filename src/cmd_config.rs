use std::path::Path;

use crate::cli::ConfigArgs;
use crate::config::Config;

pub fn run(args: ConfigArgs) -> anyhow::Result<()> {
    let path = Path::new(&args.output);
    Config::generate_default_yaml(path)?;
    println!("Generated default config: {}", args.output);
    Ok(())
}
