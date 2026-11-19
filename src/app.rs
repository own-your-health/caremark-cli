//! The commands.

use clap::{CommandFactory, FromArgMatches};
use pk_cli_config::profile::{ProfileName, Profiles};
use pk_cli_config::ConfigStore;
use pk_cli_core::info::{AuthInfo, CliInfo};
use pk_cli_core::{output, CliError};
use pk_cli_selfupdate::Updater;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cli::*;
use crate::client::{self, Client};
use crate::session::SessionStore;
use crate::source;

const REPO: &str = "piekstra/caremark-cli";
/// `info`'s profiles: health-records/v1.
pub const PROFILES: &[&str] = &[pk_med_records::PROFILE];

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// The member-site logins, one per account (`caremark profiles add
    /// <NAME>`).
    #[serde(default, skip_serializing_if = "Profiles::is_empty")]
    pub profiles: Profiles,
}

struct Ctx<'a> {
    cli: &'a Cli,
    store: ConfigStore,
}

impl Ctx<'_> {
    fn json(&self) -> bool {
        self.cli.common.json
    }

    fn note(&self, msg: &str) {
        if !self.cli.common.quiet {
            eprintln!("{msg}");
        }
    }

    fn profiles(&self) -> Result<Profiles, CliError> {
        Ok(self.store.load::<Config>()?.profiles)
    }
}

/// Now as RFC 3339 UTC, to the second.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    pk_cli_core::dates::fmt_rfc3339(secs)
}

pub fn main() {
    // A reader that closes the pipe early (`caremark api ... | head`) ends
    // the process quietly, as it does any Unix tool, instead of a panic in
    // println!.
    #[cfg(unix)]
    // SAFETY: called first, before any thread starts; it only resets the
    // disposition Rust's runtime set for SIGPIPE back to the default.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let flag = source::profile_flag_given(&matches);
    if let Err(e) = run(&cli, flag) {
        std::process::exit(output::fail(&e, cli.common.json));
    }
}

/// `profile_flag`: whether `--profile` was on the command line (not only in
/// `CAREMARK_PROFILE`).
pub fn run(cli: &Cli, profile_flag: bool) -> Result<(), CliError> {
    let ctx = Ctx {
        cli,
        store: ConfigStore::new(BIN).with_override(cli.config.clone()),
    };
    match &cli.command {
        Command::Auth(AuthCmd::Reset { force }) => {
            source::no_profile(profile_flag, "auth reset")?;
            crate::auth::reset(*force, cli.common.interactive(), cli.common.json, cli.common.quiet)
        }
        Command::Auth(cmd) => auth(&ctx, cmd),
        Command::Config(cmd) => {
            source::no_profile(profile_flag, "config")?;
            config(&ctx, cmd)
        }
        Command::Profiles(cmd) => {
            source::no_profile(profile_flag, "profiles")?;
            profiles_cmd(&ctx, cmd)
        }
        Command::Api(args) => api(&ctx, args),
        Command::SelfUpdate(args) => {
            source::no_profile(profile_flag, "self-update")?;
            Updater {
                repo: REPO.into(),
                binary: BIN.into(),
                target: env!("BUILD_TARGET").into(),
                current: env!("CARGO_PKG_VERSION").into(),
            }
            .run(args, cli.common.json, cli.common.quiet)
        }
        Command::Completions { shell } => {
            source::no_profile(profile_flag, "completions")?;
            clap_complete::generate(*shell, &mut Cli::command(), BIN, &mut std::io::stdout());
            Ok(())
        }
        Command::Info => {
            source::no_profile(profile_flag, "info")?;
            output::json(&info());
            Ok(())
        }
    }
}

/// `info --json` (cli-info/v1).
pub fn info() -> serde_json::Value {
    let info = CliInfo::new(
        BIN,
        env!("CARGO_PKG_VERSION"),
        &format!("https://github.com/{REPO}"),
        // Every record command signs in (medical-common DESIGN.md §1.1),
        // so a records store passes --profile.
        AuthInfo {
            required: true,
            method: "browser-session".into(),
            login_hint: Some(format!("{BIN} auth login --browser --profile <NAME>")),
        },
        &[
            "profiles",
            "api",
            // The login commands take --profile.
            pk_med_records::LOGIN_PROFILES,
        ],
    )
    .with_profiles(PROFILES);
    serde_json::to_value(&info).expect("CliInfo serializes")
}

// ------------------------------------------------------------------ auth

fn auth(ctx: &Ctx, cmd: &AuthCmd) -> Result<(), CliError> {
    let quiet = ctx.cli.common.quiet;
    // Flags are checked before the profile: a refused combination is exit 2
    // whatever the profile.
    let profile = source::site_profile(ctx.cli, &ctx.profiles()?)?;
    match cmd {
        AuthCmd::Login(args) => crate::auth::login(args, &profile, ctx.json(), quiet),
        AuthCmd::Status => crate::auth::status(&profile, ctx.json(), quiet),
        AuthCmd::Logout => crate::auth::logout(&profile, ctx.json(), quiet),
        AuthCmd::Reset { .. } => unreachable!("run() handles auth reset"),
    }
}

// -------------------------------------------------------------- profiles

/// `profiles list|add|remove` (cli-common DESIGN.md §2.5): the logins, by
/// name, in the config. Removing one removes what caremark keeps for it
/// first.
fn profiles_cmd(ctx: &Ctx, cmd: &ProfilesCmd) -> Result<(), CliError> {
    match cmd {
        ProfilesCmd::List => {
            let cfg: Config = ctx.store.load()?;
            let list = cfg.profiles.list();
            if ctx.json() {
                output::json(&serde_json::to_value(&list).unwrap_or_default());
            } else if list.profiles.is_empty() {
                ctx.note(&format!(
                    "no profiles: `{BIN} profiles add <NAME>`, one per caremark.com account"
                ));
            } else {
                for p in &list.profiles {
                    println!("{}", p.name);
                }
            }
            Ok(())
        }
        ProfilesCmd::Add { name } => {
            let name = ProfileName::parse(name)?;
            let mut cfg: Config = ctx.store.load()?;
            let added = cfg.profiles.add(name.clone());
            if added {
                ctx.store.save(&cfg)?;
            }
            output::emit(
                ctx.json(),
                "profile-add",
                json!({ "profile": name.as_str(), "added": added }),
                |_| {
                    ctx.note(&if added {
                        format!("added profile {name}; sign in with `{BIN} auth login --browser --profile {name}`")
                    } else {
                        format!("profile {name} is already configured")
                    })
                },
            );
            Ok(())
        }
        ProfilesCmd::Remove { name, force } => {
            let name = ProfileName::parse(name)?;
            // Whether it can ask, before the config or the vault is read.
            pk_cli_core::confirm::require_confirmable(*force, ctx.cli.common.interactive(), "remove a profile")?;
            let cfg: Config = ctx.store.load()?;
            if !cfg.profiles.contains(&name) {
                return Err(CliError::NotFound(format!("no profile `{name}`"))
                    .with_remedy(format!("`{BIN} profiles list` shows the configured ones")));
            }
            pk_cli_core::confirm::confirm(
                *force,
                &format!("Remove profile {name}, its stored session and its browser profile?"),
            )?;
            let done = crate::auth::forget_login(&name)?;
            let mut cfg: Config = ctx.store.load()?;
            cfg.profiles.remove(&name);
            ctx.store.save(&cfg)?;
            let mut v = json!({ "profile": name.as_str(), "session_removed": done.session_removed });
            if let Some(removed) = done.browser_profile_removed {
                v["browser_profile_removed"] = removed.into();
            }
            output::emit(ctx.json(), "profile-remove", v, |_| {
                ctx.note(&format!(
                    "removed profile {name}: {}",
                    if done.session_removed {
                        "its session was removed"
                    } else {
                        "no session was stored"
                    }
                ))
            });
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- config

fn config(ctx: &Ctx, cmd: &ConfigCmd) -> Result<(), CliError> {
    match cmd {
        ConfigCmd::Path => {
            let path = ctx.store.path()?;
            if ctx.json() {
                output::json(&json!({ "schema": "config-path/v1", "path": path.display().to_string() }));
            } else {
                println!("{}", path.display());
            }
            Ok(())
        }
        ConfigCmd::Show => {
            let cfg: Config = ctx.store.load()?;
            let mut v = serde_json::to_value(&cfg).unwrap_or_default();
            if ctx.json() {
                v["schema"] = "config/v1".into();
                output::json(&v);
            } else if v.as_object().is_some_and(|o| o.is_empty()) {
                ctx.note(&format!("no settings (`{BIN} profiles add <NAME>` adds a login)"));
            } else {
                output::render(&v);
            }
            Ok(())
        }
    }
}

// ------------------------------------------------------------------- api

/// A signed-in member-site client for `profile`, and the store to write
/// its updated session back to.
pub(crate) fn site_client(profile: &ProfileName) -> Result<(Client, SessionStore), CliError> {
    let store = SessionStore::open()?;
    let session = store.load(profile)?;
    let client = Client::new(&session, profile, client::BASE, client::PACE)?;
    Ok((client, store))
}

/// After a command's reads: the cookies the answers set, written back to
/// the profile's session, unless a newer sign-in replaced it meanwhile. A
/// failure is a warning; the output is complete and the stored session
/// still works.
pub(crate) fn write_back(client: &Client, store: &SessionStore, profile: &ProfileName, read_header: &str) {
    let Some(updated) = client.updated_cookie_header() else {
        return;
    };
    let result = store.get(profile).and_then(|current| match current {
        Some(current) if current.cookie_header.expose() == read_header => {
            store.put(profile, &current.with_cookie_header(updated))
        }
        _ => Ok(()),
    });
    if let Err(e) = result {
        eprintln!(
            "warning: the session's updated cookies were not stored: {}",
            e.message()
        );
    }
}

fn api(ctx: &Ctx, args: &ApiArgs) -> Result<(), CliError> {
    // The request is checked before the profile, the vault or the site.
    let request = client::api_request(args.method, &args.path, args.data.as_deref())?;
    let profile = source::site_profile(ctx.cli, &ctx.profiles()?)?;
    let (client, store) = site_client(&profile)?;
    let read_header = store.load(&profile)?.cookie_header.expose().to_string();
    let answer = client.api(&request)?;
    write_back(&client, &store, &profile, &read_header);
    output::json(&answer);
    Ok(())
}
