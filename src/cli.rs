use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "reflex",
    version,
    about = "The persistent brain of your codebase.",
    long_about = "Indexes symbols, issues and hardcoded values so any question about the code\nis answered from the index in milliseconds. Refreshes surgically when files change."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize the brain in the current directory (creates .reflex/)
    Init,
    /// Scan the repo and refresh the brain (only changed files are re-parsed)
    Index {
        /// Re-parse every file, even unchanged ones
        #[arg(long)]
        force: bool,
        /// Print one line per indexed file
        #[arg(long)]
        verbose: bool,
    },
    /// Watch the filesystem and refresh the brain as files change
    Watch {
        /// Quiet period in milliseconds before refreshing after a change
        #[arg(long, default_value_t = 300)]
        debounce: u64,
    },
    /// Brain statistics
    Stats,
    /// Search symbols by name substring, optionally --kind <kind>
    Symbols {
        filter: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// List classes
    Classes(FindArgs),
    /// List structs
    Structs(FindArgs),
    /// List enums and their variants
    Enums(FindArgs),
    /// List functions and methods
    Fns(FindArgs),
    /// List traits and interfaces
    Traits(FindArgs),
    /// List constants
    Consts(FindArgs),
    /// Show the outline (symbols + issues) of one file
    Outline {
        /// Path or path suffix, e.g. src/main.rs
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// List issues: syntax errors, TODO/FIXME, magic numbers, hardcoded strings
    Issues {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Syntax errors only
    Errors {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Hardcoded strings and magic numbers only
    Hardcoded {
        #[arg(long)]
        strings_only: bool,
        #[arg(long)]
        numbers_only: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Live regex search across file contents
    Grep {
        pattern: String,
        #[arg(long, default_value_t = 200)]
        max: usize,
    },
    /// Ask the brain anything (requires KIMI_API_KEY)
    Ask {
        /// The question, in quotes
        question: Vec<String>,
    },
    /// Start an MCP server (stdio) so AI agents can query the brain
    Mcp,
}

#[derive(Args)]
pub struct FindArgs {
    /// Name substring filter
    pub filter: Option<String>,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
}
