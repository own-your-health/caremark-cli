//! The household's own names for the members of a profile: `patients
//! label` (medical-common DESIGN.md §1). One sign-in sees a whole family,
//! so `auth login` cannot name them; a label is set per member afterwards.
//!
//! The labels live in the profile's settings in caremark's config, beside
//! every member the family read has listed. `patients label` checks its
//! `--patient` against those, so it makes no request and reads no secret.

use std::collections::BTreeMap;
use std::io::Write;

use pk_cli_config::profile::ProfileName;
use pk_cli_config::ConfigStore;
use pk_cli_core::CliError;
use pk_med_records::dto::{kind, relationship};
use pk_med_records::PatientContext;
use serde::{Deserialize, Serialize};

use crate::app::Config;
use crate::cli::{Cli, LabelArgs, BIN};
use crate::model::Person;

/// A profile's settings: each member the family read has listed, by
/// `Patient.id`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Members {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub members: BTreeMap<String, Member>,
}

/// What caremark keeps of a member between runs: never a name.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// The household's own name for the member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `subscriber` or `dependent`, as the family read gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_role: Option<String>,
    /// The member the profile signs in as.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signed_in: bool,
}

impl Members {
    /// The id of the member whose label is `q`, case aside.
    fn by_label(&self, q: &str) -> Option<&str> {
        self.members
            .iter()
            .find(|(_, m)| m.label.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(q)))
            .map(|(id, _)| id.as_str())
    }

    /// Whether `q` is a member's id, case aside.
    fn is_id(&self, q: &str) -> bool {
        self.members.keys().any(|id| id.eq_ignore_ascii_case(q))
    }
}

/// After a family read: `people` with the labels the profile holds for
/// them, and the members the read listed added to the profile's settings.
/// None is removed, so an answer that lists fewer people keeps every label;
/// the config is written only when something changed.
pub fn record(store: &ConfigStore, profile: &ProfileName, people: &mut [Person]) -> Result<(), CliError> {
    let mut config = store.load::<Config>()?;
    let mut members: Members = config.profiles.settings(profile)?;
    let mut changed = false;
    for p in people.iter_mut() {
        let role = p.role.map(str::to_string);
        let m = members.members.entry(p.id.clone()).or_insert_with(|| {
            changed = true;
            Member::default()
        });
        if m.member_role != role || m.signed_in != p.signed_in {
            m.member_role = role;
            m.signed_in = p.signed_in;
            changed = true;
        }
        p.label = m.label.clone();
    }
    if changed {
        config.profiles.set_settings(profile, &members)?;
        store.save(&config)?;
    }
    Ok(())
}

/// `patients label --profile <NAME> --patient <ID> (<WHO> | --clear)`.
pub fn run(
    cli: &Cli,
    store: &ConfigStore,
    profile: &ProfileName,
    args: &LabelArgs,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    let who = match (args.who.as_deref().map(str::trim), args.clear) {
        (Some(""), false) => {
            return Err(CliError::Usage(
                "the label is empty: give the household's name for the member, or --clear".into(),
            ))
        }
        (Some(w), false) => Some(w.to_string()),
        (None, true) => None,
        _ => return Err(CliError::Usage("give a label or --clear, one of the two".into())),
    };
    let mut config = store.load::<Config>()?;
    let mut members: Members = config.profiles.settings(profile)?;
    // No echo of --patient: it may be an id, which is a digest of short
    // identifiers (medical-common PRIVACY.md).
    let q = args.patient.trim();
    let id = match members.members.keys().find(|id| id.eq_ignore_ascii_case(q)) {
        Some(id) => id.clone(),
        None => match members.by_label(q) {
            Some(id) => id.to_string(),
            None => {
                return Err(CliError::NotFound(format!(
                    "--patient names no member of profile `{profile}` that caremark has listed"
                ))
                .with_remedy(format!(
                    "`{BIN} patients list --profile {profile}` reads the account's people; give an id it prints"
                )))
            }
        },
    };
    if let Some(w) = &who {
        if members.is_id(w) {
            return Err(CliError::Usage(
                "a label cannot be a member's id: --patient could not tell them apart".into(),
            ));
        }
        if members.by_label(w).is_some_and(|holder| holder != id) {
            return Err(CliError::Usage(
                "another member of this profile has that label; a label names one member".into(),
            ));
        }
    }
    let member = members
        .members
        .get_mut(&id)
        .expect("the id was found among the members");
    if member.label != who {
        member.label = who.clone();
        let member = member.clone();
        config.profiles.set_settings(profile, &members)?;
        store.save(&config)?;
        return print(cli, out, &id, &member);
    }
    let member = member.clone();
    print(cli, out, &id, &member)
}

/// The member as `patient-list/v1` holding it alone, or a line.
fn print(cli: &Cli, out: &mut dyn Write, id: &str, m: &Member) -> Result<(), CliError> {
    if cli.common.json {
        let context = PatientContext {
            id: id.to_string(),
            organization: crate::fhir::ORGANIZATION.into(),
            label: m.label.clone(),
            relationship: m.signed_in.then(|| relationship::SELF.into()),
            kind: Some(kind::MEMBER.into()),
            member_role: m.member_role.clone(),
            fhir_user: None,
            name: None,
            session: None,
        };
        let doc = serde_json::to_value(pk_med_records::patient_list(vec![context])).unwrap_or_default();
        let text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| doc.to_string());
        return writeln!(out, "{text}").map_err(|e| CliError::Other(format!("writing the output: {e}")));
    }
    let line = match &m.label {
        Some(l) => format!("{id} is labelled `{l}`"),
        None => format!("{id} has no label"),
    };
    writeln!(out, "{line}").map_err(|e| CliError::Other(format!("writing the output: {e}")))
}
