use anyhow::{Context, Result};
use clap::Parser;
use crates_index::Index;
use crates_io_api::SyncClient;
use git2::Repository;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about = "Clone the latest source repo of every crate on crates.io")]
struct Args {
    /// Output directory where repositories will be cloned
    #[arg(short, long, default_value = "repos")]
    output: PathBuf,

    /// Delay between API requests in milliseconds (default 1100 ms to follow crawler policy)
    #[arg(short = 'd', long, default_value_t = 1100)]
    delay_ms: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    std::fs::create_dir_all(&args.output).context("failed to create output directory")?;

    // Use the same local index cache as Cargo
    let mut index = Index::new_cargo_default().context("could not open crates.io index")?;
    index.update().context("could not update crates.io index")?;

    let crates: Vec<_> = index.crates().collect();
    println!("Found {} crates in the index", crates.len());

    // Respect the crates.io crawler policy: identify ourselves & keep ≤1 request/sec
    let client = SyncClient::new(
        "bugbot9000/0.1.0 (https://github.com/timschmidt/bugbot9000)",
        Duration::from_millis(args.delay_ms),
    )
    .context("could not create crates.io API client")?;

    for krate in crates {
        let name = krate.name();
        let dest = args.output.join(name);
        if dest.exists() {
            continue;
        }

        match client.get_crate(name) {
            Ok(resp) => {
                if let Some(repo) = resp.crate_data.repository {
                    // Accept both HTTPS and git+ssh style URLs
                    match Repository::clone(&repo, &dest) {
                        Ok(_) => println!("✓ cloned {}", name),
                        Err(e) => eprintln!("✗ failed to clone {}: {e}", name),
                    }
                } else {
                    eprintln!("ℹ no repository URL for {}", name);
                }
            }
            Err(e) => eprintln!("✗ failed to fetch metadata for {}: {e}", name),
        }
    }

    Ok(())
}
