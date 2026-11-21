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

/// Whether `q` (a `--patient` value) is the id `id`: case aside, as
/// every command reads `--patient`.
pub fn is_the_id(q: &str, id: &str) -> bool {
    q.eq_ignore_ascii_case(id)
}

/// Whether `q` (a `--patient` value) is the label `label`: case aside.
pub fn is_the_label(q: &str, label: Option<&str>) -> bool {
    label.is_some_and(|l| l.eq_ignore_ascii_case(q))
}

impl Members {
    /// The id of the member whose label is `q`, case aside.
    fn by_label(&self, q: &str) -> Option<&str> {
        self.members
            .iter()
            .find(|(_, m)| is_the_label(q, m.label.as_deref()))
            .map(|(id, _)| id.as_str())
    }

    /// Whether `q` is a member's id, case aside.
    fn is_id(&self, q: &str) -> bool {
        self.members.keys().any(|id| is_the_id(q, id))
    }
}

/// After a family read: `people` with the labels the profile holds for
/// them, and the members the read listed added to the profile's settings.
/// None is removed, so an answer that lists fewer people keeps every label.
/// The config is written only when a member is new or their role changed,
/// and then from a fresh read of it, changing no label: a `patients label`
/// saved since the first read keeps its label.
pub fn record(store: &ConfigStore, profile: &ProfileName, people: &mut [Person]) -> Result<(), CliError> {
    let members: Members = store.load::<Config>()?.profiles.settings(profile)?;
    let stale = |p: &Person, m: Option<&Member>| {
        m.is_none_or(|m| m.member_role.as_deref() != p.role || m.signed_in != p.signed_in)
    };
    let changed = people.iter().any(|p| stale(p, members.members.get(&p.id)));
    for p in people.iter_mut() {
        p.label = members.members.get(&p.id).and_then(|m| m.label.clone());
    }
    if !changed {
        return Ok(());
    }
    let mut config = store.load::<Config>()?;
    let mut fresh: Members = config.profiles.settings(profile)?;
    for p in people.iter() {
        let m = fresh.members.entry(p.id.clone()).or_default();
        m.member_role = p.role.map(str::to_string);
        m.signed_in = p.signed_in;
    }
    config.profiles.set_settings(profile, &fresh)?;
    store.save(&config)
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
    let id = match members.members.keys().find(|id| is_the_id(q, id)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str, role: Option<&'static str>) -> Person {
        Person {
            id: id.into(),
            key: "001".into(),
            first_name: None,
            last_name: None,
            birth_date: None,
            role,
            member_id: None,
            signed_in: false,
            label: None,
        }
    }

    #[test]
    fn a_family_read_adds_people_and_keeps_their_labels() {
        let dir = std::env::temp_dir().join(format!("caremark-labels-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"profiles":{"adult1":{}}}"#).unwrap();
        let store = ConfigStore::new(BIN).with_override(Some(path.clone()));
        let profile = ProfileName::parse("adult1").unwrap();
        let mut read = vec![person("p1", None), person("p2", None)];
        record(&store, &profile, &mut read).unwrap();
        // A label set after the read; the next read changes a role.
        let mut config = store.load::<Config>().unwrap();
        let mut members: Members = config.profiles.settings(&profile).unwrap();
        members.members.get_mut("p1").unwrap().label = Some("alex".into());
        config.profiles.set_settings(&profile, &members).unwrap();
        store.save(&config).unwrap();
        let mut again = vec![person("p1", Some("subscriber"))];
        record(&store, &profile, &mut again).unwrap();
        assert_eq!(again[0].label.as_deref(), Some("alex"));
        let kept: Members = store.load::<Config>().unwrap().profiles.settings(&profile).unwrap();
        assert_eq!(kept.members["p1"].label.as_deref(), Some("alex"));
        assert_eq!(kept.members["p1"].member_role.as_deref(), Some("subscriber"));
        // Nobody is removed by a read that lists fewer people.
        assert!(kept.members.contains_key("p2"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
