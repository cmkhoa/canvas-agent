//! Terminal interface for Canvas Academic Planner.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use canvas_agent::agent::Agent;
use canvas_agent::audit::AuditLog;
use canvas_agent::clock::{Clock, FixedClock, SystemClock, format_at};
use canvas_agent::config::{AppConfig, Overrides};
use canvas_agent::eval::run_suite;
use canvas_agent::llm::LlmProvider;
use canvas_agent::llm::openai::OpenAiClient;
use canvas_agent::mcp::LiveCanvasMcp;
use canvas_agent::planner::Planner;
use canvas_agent::policy::Policy;
use canvas_agent::session::Session;
use chrono_tz::Tz;
use clap::{Parser, Subcommand};

/// A read-only academic planning agent over canvas-mcp.
#[derive(Parser)]
#[command(name = "canvas-planner", version, about)]
struct Cli {
    /// Student time zone (IANA). Required unless STUDENT_TIMEZONE is set.
    #[arg(long, env = "STUDENT_TIMEZONE")]
    timezone: Option<String>,

    /// Deterministic clock: RFC 3339 with offset, or YYYY-MM-DD.
    #[arg(long, env = "CANVAS_AGENT_NOW")]
    now: Option<String>,

    /// Default planning horizon in days.
    #[arg(long, env = "CANVAS_AGENT_HORIZON_DAYS")]
    horizon_days: Option<u32>,

    /// Append-only JSONL audit log path.
    #[arg(long, env = "CANVAS_AGENT_AUDIT_LOG")]
    audit_log: Option<PathBuf>,

    /// Priority policy TOML.
    #[arg(long, env = "CANVAS_AGENT_POLICY")]
    policy: Option<PathBuf>,

    /// Command that starts canvas-mcp.
    #[arg(long, env = "CANVAS_MCP_COMMAND")]
    mcp_command: Option<String>,

    /// Extra canvas-mcp argument (repeatable). Overrides CANVAS_MCP_ARGS.
    #[arg(long = "mcp-arg")]
    mcp_arg: Vec<String>,

    /// Model id for the OpenAI-compatible endpoint.
    #[arg(long, env = "LLM_MODEL")]
    model: Option<String>,

    /// OpenAI-compatible base URL.
    #[arg(long, env = "LLM_BASE_URL")]
    base_url: Option<String>,

    /// Maximum model turns per user request.
    #[arg(long)]
    max_steps: Option<usize>,

    /// Maximum tool calls per user request.
    #[arg(long)]
    max_tool_calls: Option<usize>,

    /// Ask one question, print the answer, and exit.
    #[arg(long)]
    once: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the offline deterministic evaluation suite (no Canvas or model key needed).
    Eval {
        #[arg(long, default_value = "evals/scenarios.json")]
        scenarios: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    AppConfig::load_dotenv();
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    let cli = Cli::parse();
    if let Some(Command::Eval { scenarios }) = &cli.command {
        return match run_suite(scenarios).await {
            Ok(report) => {
                report.print();
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
        };
    }

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let overrides = Overrides {
        timezone: cli.timezone,
        now: cli.now,
        horizon_days: cli.horizon_days,
        audit_log: cli.audit_log,
        policy: cli.policy,
        mcp_command: cli.mcp_command,
        mcp_args: if cli.mcp_arg.is_empty() {
            None
        } else {
            Some(cli.mcp_arg)
        },
        model: cli.model,
        base_url: cli.base_url,
        max_steps: cli.max_steps,
        max_tool_calls: cli.max_tool_calls,
    };
    let config = AppConfig::load(overrides, true)?;
    let policy = Policy::load(&config.policy_path)?;
    if config.horizon_days > policy.max_horizon_days() {
        anyhow::bail!(
            "default horizon {} days exceeds the policy maximum of {} days",
            config.horizon_days,
            policy.max_horizon_days()
        );
    }
    let clock: Arc<dyn Clock> = match config.now_override {
        Some(at) => Arc::new(FixedClock::new(at)),
        None => Arc::new(SystemClock),
    };
    let tz: Tz = config.timezone;

    let mcp = LiveCanvasMcp::connect(&config.mcp).await.map_err(|error| {
        anyhow::anyhow!(
            "{error}. Is canvas-mcp installed and on PATH? Set CANVAS_MCP_COMMAND or --mcp-command."
        )
    })?;
    let llm = OpenAiClient::new(&config.llm)?;
    let audit = AuditLog::open(&config.audit_path)?;
    let planner = Planner::new(policy.clone(), tz);
    let session = Session::new(tz, config.horizon_days);

    let mut agent = Agent::new(
        Arc::new(llm) as Arc<dyn LlmProvider>,
        Arc::new(mcp),
        Arc::clone(&clock),
        planner,
        audit,
        session,
        config.max_tool_steps,
        config.max_tool_calls,
    );

    if let Some(question) = cli.once {
        run_and_print(&mut agent, &question).await?;
        return Ok(());
    }

    println!("Canvas Academic Planner — read-only");
    println!(
        "Now: {} · time zone: {} · horizon: {} days",
        format_at(clock.now(), tz),
        tz.name(),
        config.horizon_days
    );
    println!(
        "Model: {} · MCP: {} · audit: {}",
        config.llm.model,
        config.mcp.command,
        audit_display(agent.audit_path())
    );
    println!("Type /help for commands. Canvas writes are not supported.\n");

    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        print!("> ");
        io::stdout().flush()?;
        let Some(line) = lines.next() else {
            println!();
            break;
        };
        let line = line?;
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match input {
            "/quit" | "/exit" => break,
            "/help" => print_help(),
            "/now" => println!("{}", format_at(clock.now(), tz)),
            "/horizon" => println!("{} days", agent.session().horizon_days),
            "/sources" => print_sources(&agent, tz),
            "/clear" => {
                agent.clear_history();
                println!("Session history cleared. Timezone and horizon are unchanged.");
            }
            _ if input.starts_with("/horizon ") => {
                let value = input.trim_start_matches("/horizon ").trim();
                match value.parse::<u32>() {
                    Ok(days) => match agent.set_horizon(days) {
                        Ok(()) => println!("Planning horizon set to {days} days."),
                        Err(error) => eprintln!("error: {error}"),
                    },
                    Err(_) => eprintln!("usage: /horizon <1-{}>", policy.max_horizon_days()),
                }
            }
            _ if input.starts_with('/') => {
                eprintln!("unknown command. Type /help.");
            }
            _ => {
                run_and_print(&mut agent, input).await?;
            }
        }
    }
    Ok(())
}

async fn run_and_print(agent: &mut Agent, input: &str) -> anyhow::Result<()> {
    let result = agent.run_turn(input).await?;
    println!("{}", result.text);
    if result.degraded {
        eprintln!("note: the answer degraded to a deterministic fallback.");
    }
    println!();
    Ok(())
}

fn print_help() {
    println!(
        "Commands:\n\
         /help              show this help\n\
         /horizon [days]    show or set the planning horizon\n\
         /now               show the effective clock\n\
         /sources           show sources from the last answer\n\
         /clear             clear conversation history\n\
         /quit              exit\n\
         \n\
         Questions to try:\n\
         - What should I prioritize this week?\n\
         - What assignments am I most at risk of missing?\n\
         - Summarize my upcoming workload by course.\n\
         - What do I need to finish before Friday?"
    );
}

fn print_sources(agent: &Agent, tz: Tz) {
    let sources = agent.last_sources();
    if sources.is_empty() {
        println!("No Canvas sources from the last answer.");
        return;
    }
    for source in sources {
        println!("{}", source.render(tz));
    }
}

fn audit_display(path: &std::path::Path) -> String {
    path.display().to_string()
}
