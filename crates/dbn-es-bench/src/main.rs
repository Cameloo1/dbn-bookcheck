mod acquisition;
mod analysis_command;
mod decode_command;
mod sample;

use std::path::PathBuf;

use acquisition::{acquire, load_config, quote, verify};
use analysis_command::{run_sweeps, run_validation};
use clap::{Parser, Subcommand};
use decode_command::run_stats;
use sample::generate_sample;

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error(transparent)]
    Acquisition(#[from] acquisition::AcquisitionError),
    #[error(transparent)]
    Decode(#[from] decode_command::DecodeCommandError),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Analysis(#[from] analysis_command::AnalysisError),
    #[error(transparent)]
    Sample(#[from] sample::SampleError),
}

#[derive(Debug, Parser)]
#[command(name = "dbn-es-bench", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Quote, acquire, and verify the bounded market-data input set.
    Data {
        #[command(subcommand)]
        command: DataCommand,
    },
    /// Decode files and report record, timestamp, and sequence diagnostics.
    Decode {
        #[command(subcommand)]
        command: DecodeCommand,
    },
    /// Reconstruct books, validate against MBP-10, and detect liquidity sweeps.
    Analyze {
        #[command(subcommand)]
        command: AnalyzeCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DataCommand {
    /// Generate a small deterministic synthetic dataset for tests and demos.
    Sample {
        /// Directory for the four DBN files and their manifest.
        #[arg(long, default_value = "data/sample")]
        output_dir: PathBuf,
    },
    /// Quote the configured request without downloading data.
    Quote {
        /// Path to the committed acquisition configuration.
        #[arg(long, default_value = "config/live-session.json")]
        config: PathBuf,
    },
    /// Re-quote, enforce the spend ledger, and download the configured data.
    Acquire {
        /// Path to the committed acquisition configuration.
        #[arg(long, default_value = "config/live-session.json")]
        config: PathBuf,
    },
    /// Verify the data manifest, checksums, record counts, and spend cap.
    Verify {
        /// Path to the committed acquisition configuration.
        #[arg(long, default_value = "config/live-session.json")]
        config: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum DecodeCommand {
    /// Stream every manifest file and verify its declared schema and record count.
    Stats {
        /// Path to the generated data manifest.
        #[arg(long, default_value = "data/manifest.json")]
        manifest: PathBuf,
        /// Destination for the deterministic decode report.
        #[arg(long, default_value = "data/decode-stats.json")]
        output: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum AnalyzeCommand {
    /// Validate reconstructed MBO top-of-book against aligned MBP-10 updates.
    Validate {
        /// Path to the generated aligned-data manifest.
        #[arg(long, default_value = "data/manifest.json")]
        manifest: PathBuf,
        /// Stable Markdown validation report.
        #[arg(long, default_value = "docs/book-validation.md")]
        output: PathBuf,
        /// Local machine-readable validation evidence.
        #[arg(long, default_value = "data/book-validation.json")]
        json_output: PathBuf,
        /// ES tick size in DBN fixed-price units.
        #[arg(long, default_value_t = 250_000_000)]
        tick_size: i64,
    },
    /// Detect liquidity sweeps from MBO trades and reconstructed resting depth.
    Sweeps {
        /// Path to the generated data manifest.
        #[arg(long, default_value = "data/manifest.json")]
        manifest: PathBuf,
        /// Committed four-parameter detector configuration.
        #[arg(long, default_value = "config/sweep.json")]
        config: PathBuf,
        /// JSON Lines event output.
        #[arg(long, default_value = "out/sweeps.jsonl")]
        output: PathBuf,
        /// Local machine-readable summary.
        #[arg(long, default_value = "data/sweep-summary.json")]
        summary: PathBuf,
        /// Stable Markdown detector report.
        #[arg(long, default_value = "docs/sweep-detection.md")]
        report: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let cli = Cli::parse();
    match cli.command {
        Command::Data { command } => match command {
            DataCommand::Sample { output_dir } => {
                let manifest = generate_sample(&output_dir)?;
                println!("{}", serde_json::to_string_pretty(&manifest)?);
            }
            DataCommand::Quote { config } => {
                let config = load_config(&config)?;
                let report = quote(&config).await?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            DataCommand::Acquire { config } => {
                let config = load_config(&config)?;
                let manifest = acquire(&config).await?;
                println!("{}", serde_json::to_string_pretty(&manifest)?);
            }
            DataCommand::Verify { config } => {
                let config = load_config(&config)?;
                let report = verify(&config)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        },
        Command::Decode { command } => match command {
            DecodeCommand::Stats { manifest, output } => {
                let report = run_stats(&manifest, &output)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        },
        Command::Analyze { command } => match command {
            AnalyzeCommand::Validate {
                manifest,
                output,
                json_output,
                tick_size,
            } => {
                let report = run_validation(&manifest, &output, &json_output, tick_size)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            AnalyzeCommand::Sweeps {
                manifest,
                config,
                output,
                summary,
                report,
            } => {
                let result = run_sweeps(&manifest, &config, &output, &summary, &report)?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        },
    }
    Ok(())
}
