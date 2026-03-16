use clap::{Arg, ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Auto,
    Json,
    Table,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum AuthServiceArg {
    Mail,
    Calendar,
    Drive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum GuideTopicArg {
    All,
    Account,
    Auth,
    Mail,
    Calendar,
    Drive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum McpClientArg {
    Claude,
    ClaudeDesktop,
    Codex,
    Gemini,
    Warp,
    Zed,
    Cursor,
    Antigravity,
    Windsurf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum McpTransportArg {
    Stdio,
    Http,
}

impl McpTransportArg {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

const HELP_TEMPLATE: &str = "\
{before-help}{about-with-newline}\
Usage:\n    {usage}\n\
\n\
{all-args}{after-help}\
";

#[derive(Debug, Parser)]
#[command(
    name = "zocli",
    version,
    about = "CLI for Zoho Mail, Calendar, and WorkDrive"
)]
pub struct Cli {
    #[arg(
        long,
        value_enum,
        global = true,
        default_value_t = OutputFormat::Auto,
        value_name = "FORMAT",
        help = "Output format"
    )]
    pub format: OutputFormat,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(hide = true)]
    Guide {
        #[arg(long, value_enum, default_value_t = GuideTopicArg::All)]
        topic: GuideTopicArg,
    },
    /// Add an account. If alias is omitted, it is derived from email.
    Add {
        #[arg(value_name = "EMAIL")]
        email: String,
        #[arg(value_name = "ALIAS")]
        name: Option<String>,
        /// Zoho datacenter (com, eu, in, com.au, jp, zohocloud.ca, sa, uk)
        #[arg(long, default_value = "com", value_name = "DC")]
        datacenter: String,
        /// Advanced override: Zoho account ID. When omitted, zocli auto-discovers it after login.
        #[arg(long, value_name = "ID")]
        account_id: Option<String>,
        /// Zoho organization ID (for WorkDrive team operations)
        #[arg(long, value_name = "ORG_ID")]
        org_id: Option<String>,
        /// Advanced override: OAuth2 client ID. When omitted, zocli uses the shared/default OAuth app.
        #[arg(long, value_name = "CLIENT_ID")]
        client_id: Option<String>,
        /// Advanced override: OAuth2 client secret.
        #[arg(long, value_name = "SECRET")]
        client_secret: Option<String>,
    },
    /// Show all configured accounts.
    Accounts,
    /// Set an account as current.
    Use {
        #[arg(value_name = "ALIAS")]
        name: String,
    },
    /// Show current account.
    Whoami,
    /// Show auth status of an account.
    Status {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },
    /// Authenticate with Zoho OAuth2.
    Login {
        #[arg(value_name = "SERVICE")]
        service: Option<AuthServiceArg>,
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(long, hide = true)]
        code: Option<String>,
        #[arg(long, hide = true)]
        login_hint: Option<String>,
    },
    /// Revoke credentials for one or all services.
    Logout {
        #[arg(value_name = "SERVICE")]
        service: Option<AuthServiceArg>,
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },
    #[command(hide = true)]
    Account {
        #[command(subcommand)]
        action: AccountCommand,
    },
    #[command(hide = true)]
    Auth {
        #[command(subcommand)]
        action: AuthCommand,
    },
    /// Zoho WorkDrive files and folders.
    Drive {
        #[command(subcommand)]
        action: DriveCommand,
    },
    /// Zoho Calendar events.
    Calendar {
        #[command(subcommand)]
        action: CalendarCommand,
    },
    /// Zoho Mail messages and folders.
    Mail {
        #[command(subcommand)]
        action: MailCommand,
    },
    /// Run MCP server via stdio or install into supported clients.
    Mcp {
        #[arg(
            long,
            value_enum,
            default_value_t = McpTransportArg::Stdio,
            value_name = "TRANSPORT",
            help = "MCP server transport"
        )]
        transport: McpTransportArg,
        #[arg(
            long,
            default_value = "127.0.0.1:8787",
            value_name = "ADDRESS",
            help = "Address for HTTP transport"
        )]
        listen: String,
        #[arg(
            long,
            value_name = "URL",
            help = "Canonical public URL for MCP HTTP auth discovery"
        )]
        public_url: Option<String>,
        #[command(subcommand)]
        action: Option<McpCommand>,
    },
    /// Update zocli from GitHub Releases.
    Update {
        #[arg(
            long,
            default_value = "latest",
            value_name = "VERSION",
            help = "Version without v prefix, or latest"
        )]
        version: String,
        #[arg(
            long,
            default_value_t = false,
            help = "Only check if an update is available"
        )]
        check: bool,
        #[arg(long, hide = true, value_name = "URL")]
        base_url: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// Install zocli MCP server into locally available clients.
    Install {
        #[arg(long = "client", value_enum, value_name = "CLIENT")]
        client: Vec<McpClientArg>,
        #[arg(
            long,
            value_enum,
            default_value_t = McpTransportArg::Stdio,
            value_name = "TRANSPORT",
            help = "Which transport to register in the client"
        )]
        transport: McpTransportArg,
        #[arg(
            long,
            value_name = "URL",
            help = "MCP HTTP server URL, if HTTP transport is selected"
        )]
        url: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    Add {
        name: String,
        email: String,
        #[arg(long = "use", default_value_t = false)]
        use_as_current: bool,
        #[arg(long, default_value = "com", value_name = "DC")]
        datacenter: String,
        #[arg(long, value_name = "ID")]
        account_id: Option<String>,
        #[arg(long, value_name = "ORG_ID")]
        org_id: Option<String>,
        #[arg(long, value_name = "CLIENT_ID")]
        client_id: Option<String>,
        #[arg(long, value_name = "SECRET")]
        client_secret: Option<String>,
    },
    List,
    Show {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },
    Validate {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },
    Use {
        name: String,
    },
    Current,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    Status {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },
    Login {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(long, value_enum, value_name = "SERVICE")]
        service: Option<AuthServiceArg>,
        #[arg(long, hide = true)]
        code: Option<String>,
        #[arg(long, hide = true)]
        login_hint: Option<String>,
    },
    Logout {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(long, value_enum, value_name = "SERVICE")]
        service: Option<AuthServiceArg>,
    },
}

#[derive(Debug, Subcommand)]
pub enum DriveCommand {
    /// List files in a WorkDrive folder.
    List {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        /// WorkDrive team ID or folder ID.
        #[arg(value_name = "FOLDER_ID")]
        folder_id: Option<String>,
        #[arg(long, default_value_t = 50, value_name = "N")]
        limit: usize,
        #[arg(long, default_value_t = 0, value_name = "OFFSET")]
        offset: u64,
    },
    /// Show WorkDrive info (teams, quotas).
    Info {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },
    /// Upload a local file to WorkDrive.
    Upload {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(value_name = "FILE")]
        source: PathBuf,
        /// Target folder ID in WorkDrive.
        #[arg(value_name = "FOLDER_ID")]
        folder_id: String,
        #[arg(long, default_value_t = false)]
        overwrite: bool,
    },
    /// Download a file from WorkDrive.
    Download {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        /// File resource ID in WorkDrive.
        #[arg(value_name = "FILE_ID")]
        file_id: String,
        #[arg(long, value_name = "OUTPUT")]
        output: PathBuf,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MailCommand {
    /// List mail folders.
    Folders {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },
    /// List messages. Defaults to Inbox.
    List {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(long, value_name = "FOLDER_ID")]
        folder_id: Option<String>,
        #[arg(long, default_value_t = false)]
        unread: bool,
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
    },
    /// Search messages.
    Search {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(value_name = "QUERY")]
        query: String,
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
    },
    /// Read a message by ID.
    Read {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(value_name = "MESSAGE_ID")]
        message_id: String,
        #[arg(long, value_name = "FOLDER_ID")]
        folder_id: String,
    },
    /// Send a new message.
    Send {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(value_name = "TO")]
        to: String,
        #[arg(value_name = "SUBJECT")]
        subject: String,
        #[arg(value_name = "BODY")]
        body: Option<String>,
        #[arg(long, value_name = "EMAIL")]
        cc: Vec<String>,
        #[arg(long, value_name = "EMAIL")]
        bcc: Vec<String>,
        #[arg(long, value_name = "HTML")]
        html: Option<String>,
        #[arg(long = "attachment", value_name = "FILE")]
        attachments: Vec<String>,
    },
    /// Reply to a message.
    Reply {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(value_name = "MESSAGE_ID")]
        message_id: String,
        #[arg(long, value_name = "FOLDER_ID")]
        folder_id: String,
        #[arg(value_name = "BODY")]
        body: Option<String>,
        #[arg(long, value_name = "EMAIL")]
        cc: Vec<String>,
        #[arg(long, value_name = "HTML")]
        html: Option<String>,
    },
    /// Forward a message.
    Forward {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(value_name = "MESSAGE_ID")]
        message_id: String,
        #[arg(long, value_name = "FOLDER_ID")]
        folder_id: String,
        #[arg(value_name = "TO")]
        to: String,
        #[arg(value_name = "BODY")]
        body: Option<String>,
        #[arg(long, value_name = "EMAIL")]
        cc: Vec<String>,
        #[arg(long, value_name = "EMAIL")]
        bcc: Vec<String>,
        #[arg(long, value_name = "HTML")]
        html: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum CalendarCommand {
    /// List available calendars.
    Calendars {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },
    /// List events in a date range.
    Events {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(long, value_name = "CALENDAR_UID")]
        calendar: Option<String>,
        #[arg(value_name = "FROM")]
        from: Option<String>,
        #[arg(value_name = "TO")]
        to: Option<String>,
        #[arg(long, default_value_t = 50, value_name = "N")]
        limit: usize,
    },
    /// Create a calendar event.
    Create {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(long, value_name = "CALENDAR_UID")]
        calendar: Option<String>,
        #[arg(value_name = "TITLE")]
        title: String,
        #[arg(value_name = "START")]
        start: String,
        #[arg(value_name = "END")]
        end: String,
        #[arg(long, value_name = "DESCRIPTION")]
        description: Option<String>,
        #[arg(long, value_name = "LOCATION")]
        location: Option<String>,
    },
    /// Delete a calendar event by UID.
    Delete {
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        #[arg(long, value_name = "CALENDAR_UID")]
        calendar: Option<String>,
        #[arg(value_name = "EVENT_UID")]
        event_uid: String,
        /// ETag for conditional delete (required by Zoho).
        #[arg(long, value_name = "ETAG")]
        etag: Option<String>,
    },
}

pub fn parse_cli() -> Cli {
    let command = build_cli_command();
    let matches = command.get_matches();
    Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit())
}

pub fn build_cli_command() -> clap::Command {
    localize_help(Cli::command(), true)
}

fn localize_help(mut command: clap::Command, is_root: bool) -> clap::Command {
    command = command
        .help_template(HELP_TEMPLATE)
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .subcommand_help_heading("Commands")
        .subcommand_value_name("COMMAND")
        .next_help_heading("Options")
        .mut_args(|arg| {
            if arg.get_help_heading().is_none() {
                let heading = if arg.is_positional() {
                    "Arguments"
                } else {
                    "Options"
                };
                arg.help_heading(heading)
            } else {
                arg
            }
        });

    command = command.arg(
        Arg::new("help")
            .short('h')
            .long("help")
            .action(ArgAction::Help)
            .help("Show help")
            .help_heading("Options"),
    );

    if is_root {
        command = command.disable_version_flag(true).arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help("Show version")
                .help_heading("Options"),
        );
    }

    command.mut_subcommands(|subcommand| localize_help(subcommand, false))
}
