use anyhow::{Context, Result};
use clap::Parser;
use crates_index::Index;
use crates_io_api::SyncClient;
use git2::Repository;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about = "Clone the latest source repo of every crate on crates.io")]
struct Args {
    /// Output directory where repositories will be cloned
    #[arg(short, long, default_value = "repos")]
    output: PathBuf,

    /// Delay between API requests in milliseconds (default 1100 ms to follow crawler policy)
    #[arg(short = 'd', long, default_value_t = 1100)]
    delay_ms: u64,
}

fn main() -> Result<()> {
    // ─── Database setup ──────────────────────────────────────────────────────────
    let conn = Connection::open("bugbot.sqlite").context("failed to open bugbot.sqlite")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS crates (
            name        TEXT PRIMARY KEY,
            repository  TEXT,
            status      TEXT NOT NULL
        )",
        [],
    )
    .context("failed to create crates table")?;

    // ─── CLI + filesystem prep ───────────────────────────────────────────────────
    let args = Args::parse();
    std::fs::create_dir_all(&args.output).context("failed to create output directory")?;

    // ─── Fetch crates index ──────────────────────────────────────────────────────
    let mut index = Index::new_cargo_default().context("could not open crates.io index")?;
    index.update().context("could not update crates.io index")?;
    let crates: Vec<_> = index.crates().collect();
    println!("Found {} crates in the index", crates.len());

    // ─── crates.io API client ────────────────────────────────────────────────────
    let client = SyncClient::new(
        "crates_mirror/0.1.0 (https://github.com/yourname/crates_mirror)",
        Duration::from_millis(args.delay_ms),
    )
    .context("could not create crates.io API client")?;

    // ─── Main processing loop ────────────────────────────────────────────────────
    for krate in crates {
        let name = krate.name();
        let dest = args.output.join(name);

        // Skip if we have already cloned this crate successfully
        let already_cloned: bool = conn
            .query_row(
                "SELECT status FROM crates WHERE name = ?1",
                [name],
                |row| {
                    let s: String = row.get(0)?;
                    Ok(s == "cloned")
                },
            )
            .optional()
            .context("failed querying status")?
            .unwrap_or(false);
        if already_cloned || dest.exists() {
            continue;
        }

        match client.get_crate(name) {
            Ok(resp) => {
                if let Some(repo) = resp.crate_data.repository {
                    // Insert or update repository entry with pending status
                    conn.execute(
                        "INSERT INTO crates (name, repository, status)
                         VALUES (?1, ?2, 'pending')
                         ON CONFLICT(name) DO UPDATE SET repository = excluded.repository, status = 'pending'",
                        params![name, repo],
                    )
                    .ok();

                    match Repository::clone(&repo, &dest) {
                        Ok(_) => {
                            println!("✓ cloned {}", name);
                            conn.execute(
                                "UPDATE crates SET status = 'cloned' WHERE name = ?1",
                                params![name],
                            )
                            .ok();
                        }
                        Err(e) => {
                            eprintln!("✗ failed to clone {}: {}", name, e);
                            conn.execute(
                                "UPDATE crates SET status = 'failed' WHERE name = ?1",
                                params![name],
                            )
                            .ok();
                        }
                    }
                } else {
                    eprintln!("ℹ no repository URL for {}", name);
                    conn.execute(
                        "INSERT INTO crates (name, repository, status)
                         VALUES (?1, NULL, 'no_repo')
                         ON CONFLICT(name) DO UPDATE SET status = 'no_repo'",
                        params![name],
                    )
                    .ok();
                }
            }
            Err(e) => {
                eprintln!("✗ failed to fetch metadata for {}: {}", name, e);
                conn.execute(
                    "INSERT INTO crates (name, repository, status)
                     VALUES (?1, NULL, 'metadata_error')
                     ON CONFLICT(name) DO UPDATE SET status = 'metadata_error'",
                    params![name],
                )
                .ok();
            }
        }
    }

    Ok(())
}

