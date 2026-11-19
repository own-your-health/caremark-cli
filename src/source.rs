//! Which profile a command signs in as.
//!
//! - Every command that reads the member site names its profile:
//!   `--profile <NAME>` or `CAREMARK_PROFILE`. A missing profile is exit 2;
//!   a lone profile is never picked.
//! - A profile must be configured (`caremark profiles add <NAME>`):
//!   [`resolve_profile`] is `pk_cli_config::profile::resolve`, so a missing,
//!   malformed or unknown name is exit 2 listing the configured ones.
//! - The commands that need no sign-in (`info`, `config`, `profiles`,
//!   `auth reset`, `self-update`, `completions`) take no `--profile` on
//!   their command line, and do not read the variable.

use clap::parser::ValueSource;
use clap::ArgMatches;
use pk_cli_config::profile::{self, ProfileName, Profiles};
use pk_cli_core::CliError;

use crate::cli::{Cli, BIN};

pub const PROFILE_ENV: &str = "CAREMARK_PROFILE";

/// The profile a command signs in as: `given` (the flag, or clap's value
/// from `CAREMARK_PROFILE`), which must be configured.
pub fn resolve_profile(given: Option<&str>, configured: &Profiles) -> Result<ProfileName, CliError> {
    profile::resolve(given.filter(|g| !g.is_empty()), PROFILE_ENV, configured, BIN)
}

/// Whether `--profile` was given on the command line rather than through
/// its variable, wherever in the line it stood.
pub fn profile_flag_given(matches: &ArgMatches) -> bool {
    let mut m = matches;
    let mut on_line = m.value_source("profile") == Some(ValueSource::CommandLine);
    // A global flag given after a subcommand is that subcommand's.
    while let Some((_, sub)) = m.subcommand() {
        m = sub;
        on_line |= m.value_source("profile") == Some(ValueSource::CommandLine);
    }
    on_line
}

/// The profile of a command that reads the member site.
pub fn site_profile(cli: &Cli, configured: &Profiles) -> Result<ProfileName, CliError> {
    resolve_profile(cli.profile.as_deref(), configured)
}

/// For a command that needs no sign-in: `--profile` on its command line is
/// exit 2, and `CAREMARK_PROFILE` is not read.
pub fn no_profile(flag_given: bool, command: &str) -> Result<(), CliError> {
    if flag_given {
        return Err(CliError::Usage(format!(
            "`{BIN} {command}` needs no sign-in and takes no --profile"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, FromArgMatches};

    fn parse(args: &[&str]) -> (Cli, bool) {
        let matches = Cli::command().try_get_matches_from(args).unwrap();
        let cli = Cli::from_arg_matches(&matches).unwrap();
        (cli, profile_flag_given(&matches))
    }

    fn configured(names: &[&str]) -> Profiles {
        let mut p = Profiles::new();
        for n in names {
            p.add(ProfileName::parse(n).unwrap());
        }
        p
    }

    /// Before or after the subcommand, the flag is the command line's.
    #[test]
    fn a_flag_counts_wherever_it_is_given() {
        assert!(parse(&["caremark", "--profile", "adult1", "auth", "status"]).1);
        assert!(parse(&["caremark", "auth", "status", "--profile", "adult1"]).1);
        assert!(!parse(&["caremark", "auth", "status"]).1);
    }

    /// Malformed or not configured is exit 2 naming the configured ones; a
    /// lone profile is never picked for a command that names none.
    #[test]
    fn a_profile_must_be_configured() {
        let one = configured(&["adult1"]);
        for name in ["Bad Name", "adult3", "-x", "Adult1"] {
            let (cli, _) = parse(&["caremark", &format!("--profile={name}"), "auth", "status"]);
            assert_eq!(site_profile(&cli, &one).unwrap_err().exit_code(), 2, "{name}");
        }
        let (cli, _) = parse(&["caremark", "--profile", "adult3", "auth", "status"]);
        assert!(site_profile(&cli, &one).unwrap_err().to_string().contains("adult1"));
        let (mut cli, _) = parse(&["caremark", "auth", "status"]);
        cli.profile = None;
        let e = site_profile(&cli, &one).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.to_string().contains("adult1"), "{e}");
        let e = site_profile(&cli, &Profiles::new()).unwrap_err();
        assert!(e.remedy().unwrap().contains("caremark profiles add"), "{e}");
        let (cli, _) = parse(&["caremark", "auth", "status", "--profile", "adult1"]);
        assert_eq!(site_profile(&cli, &one).unwrap().as_str(), "adult1");
    }

    #[test]
    fn commands_without_a_sign_in_take_no_profile_flag() {
        assert_eq!(no_profile(true, "info").unwrap_err().exit_code(), 2);
        no_profile(false, "info").unwrap();
    }
}
