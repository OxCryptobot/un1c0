use clap::{Parser, Subcommand};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use un1c0::agentic::{
    built_in_registry, DeterministicPlanner, EventJournal, Plan, Planner, Policy, RunOptions,
    Runtime, Workspace,
};

#[derive(Debug, Parser)]
#[command(
    name = "un1c0-agent",
    version,
    about = "Local-first, capability-scoped agent runtime"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate a deterministic starter plan for a goal.
    Plan {
        /// Goal statement to turn into a plan.
        goal: String,
    },
    /// Execute a JSON plan inside a scoped workspace.
    Run {
        /// Path to a JSON plan file.
        plan: PathBuf,
        /// Workspace root. It is created if missing.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// JSONL event journal path.
        #[arg(long)]
        journal: Option<PathBuf>,
        /// Comma-separated action IDs that are explicitly approved.
        #[arg(long, value_delimiter = ',')]
        approve: Vec<String>,
        /// Enable workspace writes in the policy. Individual writes still need approval.
        #[arg(long)]
        allow_writes: bool,
    },
    /// Print the built-in tool manifest.
    Tools,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    match args.command {
        Command::Plan { goal } => {
            let plan = DeterministicPlanner.plan(&goal)?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
        Command::Run {
            plan,
            workspace,
            journal,
            approve,
            allow_writes,
        } => {
            let plan: Plan = serde_json::from_str(&fs::read_to_string(&plan)?)?;
            let workspace = Workspace::new(&workspace)?;
            let journal_path =
                journal.unwrap_or_else(|| workspace.root().join(".un1c0/events.jsonl"));
            let policy = if allow_writes {
                Policy::developer()
            } else {
                Policy::restricted()
            };
            let runtime = Runtime::new(
                workspace,
                built_in_registry(),
                policy,
                EventJournal::new(journal_path),
            );
            let options = RunOptions {
                approved_actions: approve.into_iter().collect::<BTreeSet<_>>(),
            };
            let report = runtime.run(&plan, &options)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.status != un1c0::agentic::ActionStatus::Succeeded {
                std::process::exit(2);
            }
        }
        Command::Tools => {
            println!(
                "{}",
                serde_json::to_string_pretty(&built_in_registry().specs())?
            );
        }
    }
    Ok(())
}
