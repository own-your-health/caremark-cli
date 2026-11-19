//! The command tree (piekstra-cli/1, health-records/v1).

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use pk_cli_auth::LoginArgs;
use pk_cli_browser::BrowserArgs;
use pk_cli_core::{CommonArgs, RangeArgs};
use pk_cli_selfupdate::SelfUpdateArgs;

pub const BIN: &str = "caremark";

/// Unofficial read-only CLI for CVS Caremark: prescription fills and
/// pharmacy claims, read from the member site (caremark.com). Not
/// affiliated with or endorsed by CVS Caremark.
#[derive(Parser, Debug)]
#[command(name = BIN, version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Override the config file location.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Read the member site as this profile: one configured with `caremark
    /// profiles add <NAME>` and signed in with `caremark auth login
    /// --browser --profile <NAME>`. There is no default profile.
    #[arg(long, global = true, env = "CAREMARK_PROFILE", value_name = "NAME")]
    pub profile: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// The member-site sign-in (caremark.com), one per --profile.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// Non-secret settings.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// The member-site logins: one profile per account (profile-list/v1).
    #[command(subcommand)]
    Profiles(ProfilesCmd),
    /// The people on the account: the signed-in member and the family the
    /// site shows (patient-list/v1).
    #[command(subcommand)]
    Patients(PatientsCmd),
    /// Prescriptions, each once with its latest fill (rx-list/v1).
    #[command(subcommand)]
    Rx(RxCmd),
    /// Each fill and what it cost: the drug's total cost, what the plan
    /// paid, what you paid and what went to the deductible, with their sums
    /// (cost-list/v1).
    Costs(CostsArgs),
    /// One person's fills and pharmacy claims as records-stream/v1, for
    /// `medrec import --from caremark`.
    #[command(subcommand)]
    Records(RecordsCmd),
    /// One read of the member site as --profile <NAME>, printed as the JSON
    /// it answers: `GET <PATH>`, or `POST <PATH> --data <JSON>` to the
    /// site's experience API (a read the site sends as POST). Any other
    /// request is refused before one is sent.
    Api(ApiArgs),
    /// Update to the latest release from GitHub.
    SelfUpdate(SelfUpdateArgs),
    /// Print a shell completion script.
    Completions { shell: Shell },
    /// Machine-readable capability discovery (cli-info/v1).
    Info,
}

/// `caremark api <METHOD> <PATH> [--data JSON]`, read-only.
#[derive(Args, Debug, Clone)]
pub struct ApiArgs {
    /// GET, or POST to an experience API path (any case).
    #[arg(value_enum, ignore_case = true)]
    pub method: ApiMethod,
    /// A path on https://www.caremark.com (leading /), or a URL on it.
    pub path: String,
    /// For POST: the request body as JSON.
    #[arg(long)]
    pub data: Option<String>,
}

/// The methods `caremark api` sends: it reads only.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiMethod {
    #[value(name = "GET")]
    Get,
    #[value(name = "POST")]
    Post,
}

#[derive(Subcommand, Debug)]
pub enum AuthCmd {
    /// Sign in to the member site as --profile <NAME> and keep the session
    /// in caremark's vault: encrypted files under ~/.config/caremark/vault/,
    /// sealed with a key held in the OS keychain (auth-login/v1).
    ///
    /// --browser opens a Chrome window on caremark.com in the profile's own
    /// browser profile, where you sign in; --stdin reads the Cookie header
    /// of a www.caremark.com request, copied from a signed-in browser's
    /// DevTools, with --user-agent from the same request. One member-site
    /// read checks the session before it is stored, unless --no-verify.
    Login(LoginCmd),
    /// Whether the profile's member-site session is stored and when it was
    /// captured (auth-status/v1). Sends no request.
    Status,
    /// Remove the profile's stored session, then the browser profile its
    /// --browser sign-in keeps (auth-logout/v1).
    Logout,
    /// Remove every profile's stored session from the vault and the vault's
    /// key from the keychain (auth-reset/v1): the way out when the key is
    /// lost or damaged. Asks first; without a terminal it needs --force
    /// (exit 6).
    Reset {
        /// Do not ask.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProfilesCmd {
    /// The configured profiles, by name (profile-list/v1).
    #[command(visible_alias = "ls")]
    List,
    /// Add a profile: 1 to 32 lowercase letters, digits, `-` or `_`,
    /// starting with a letter or a digit (profile-add/v1).
    Add { name: String },
    /// Remove a profile with its stored session and the browser profile its
    /// --browser sign-in keeps (profile-remove/v1). Asks first; without a
    /// terminal it needs --force (exit 6).
    Remove {
        name: String,
        /// Do not ask.
        #[arg(long)]
        force: bool,
    },
}

// The family's login flags with `--browser` beside them (DESIGN.md §2.2).
// A plain comment: a doc comment here would replace `auth login`'s about.
#[derive(Args, Debug)]
pub struct LoginCmd {
    #[command(flatten)]
    pub login: LoginArgs,
    #[command(flatten)]
    pub browser: BrowserArgs,
    /// With --stdin or --from-env: the User-Agent of the browser the Cookie
    /// header was copied from. The check and every later read send it.
    #[arg(long, value_name = "UA")]
    pub user_agent: Option<String>,
    /// With --browser: how long the window waits for the sign-in, in
    /// minutes (1 to 120).
    #[arg(long, value_name = "MINUTES", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=120))]
    pub wait: u64,
    /// With --browser: print, on stderr, the endpoints the site's pages
    /// called on caremark.com while the window was open (method, path with
    /// ids hidden, status, content type; never a header, body or query),
    /// and keep the window open at least this many seconds, so the pages
    /// can be used. For mapping the site.
    #[arg(long, value_name = "SECONDS", hide = true)]
    pub log_requests: Option<u64>,
}

#[derive(Subcommand, Debug)]
pub enum PatientsCmd {
    /// List the people (patient-list/v1).
    #[command(visible_alias = "ls")]
    List,
}

#[derive(Subcommand, Debug)]
pub enum RxCmd {
    /// Prescriptions filled in the range (default: the last year), newest
    /// fill first (rx-list/v1).
    #[command(visible_alias = "ls")]
    List(RxListArgs),
}

#[derive(Args, Debug)]
pub struct RxListArgs {
    /// Only this person: an id from `caremark patients list`, or a first
    /// name. Default: everyone on the account.
    #[arg(long, value_name = "ID")]
    pub patient: Option<String>,
    #[command(flatten)]
    pub range: RangeArgs,
}

#[derive(Args, Debug)]
pub struct CostsArgs {
    /// Only this person: an id from `caremark patients list`, or a first
    /// name. Default: everyone on the account.
    #[arg(long, value_name = "ID")]
    pub patient: Option<String>,
    /// Fills in this range (default: the last year).
    #[command(flatten)]
    pub range: RangeArgs,
}

#[derive(Subcommand, Debug)]
pub enum RecordsCmd {
    /// One person's Patient, Coverage, prescriptions (MedicationRequest),
    /// fills (MedicationDispense) and pharmacy claims
    /// (ExplanationOfBenefit) as records-stream/v1.
    Export(ExportArgs),
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("destination").required(true).args(["ndjson", "output"])))]
pub struct ExportArgs {
    /// The person: an id from `caremark patients list`, or a first name.
    /// May be omitted when the account has one person.
    #[arg(long, value_name = "ID")]
    pub patient: Option<String>,
    /// Only fills on or after this day (YYYY-MM-DD); without it, the last
    /// 10 years. An export always runs to today (health-records/v1 has no
    /// --until): use `rx list` or `costs` for a closed range.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub since: Option<String>,
    /// Write the stream to stdout (`| medrec import --from caremark`). The
    /// stream is the output, so --json with it is a usage error. One of
    /// --ndjson and -o is required.
    #[arg(long)]
    pub ndjson: bool,
    /// Write the stream to DIR/records.ndjson instead
    /// (records.incomplete.ndjson when it ends incomplete), and print
    /// records-export/v1.
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Print the config file path.
    Path,
    /// Show the effective configuration.
    Show,
}
