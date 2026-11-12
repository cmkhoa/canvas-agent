//! Offline evaluation runner. Exits non-zero if any scenario fails.

use std::path::PathBuf;
use std::process::ExitCode;

use canvas_agent::eval::run_suite;
use clap::Parser;

/// Run the deterministic evaluation suite with a scripted model and mocked Canvas MCP.
#[derive(Parser)]
#[command(name = "canvas-eval", version, about)]
struct Cli {
    /// Scenario file.
    #[arg(long, default_value = "evals/scenarios.json")]
    scenarios: PathBuf,

    /// Print each scenario's full deterministic answer.
    #[arg(long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run_suite(&cli.scenarios).await {
        Ok(report) => {
            if cli.verbose {
                report.print_verbose();
            } else {
                report.print();
            }
            if report.all_passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("evaluation error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
