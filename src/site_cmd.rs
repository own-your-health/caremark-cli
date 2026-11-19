//! The commands that read the member site (`--profile <NAME>`):
//! `patients list`, `rx list`, `costs`, `records export` and `api`.
//!
//! [`run`] takes an [`Env`]: where sessions are kept and which site is
//! read. The binary's is the vault and www.caremark.com a second apart; the
//! tests' is an in-memory vault and a loopback server. Every document goes
//! to `out`; the session is written back once the command has done what it
//! was asked, never after an error or an incomplete export.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::time::Duration;

use pk_cli_config::profile::ProfileName;
use pk_cli_core::{dates, output, CliError, Money, Paged, RangeArgs};
use pk_med_records::dto::{kind, method, relationship};
use pk_med_records::{
    create_export_file, Finished, Header, Organization, PatientContext, RawResource, RecordsExport, RecordsWriter,
    Source,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::app::now_rfc3339;
use crate::cents::Cents;
use crate::cli::*;
use crate::client::{self, experience, Client};
use crate::fhir;
use crate::model::{prescription_id, Fill, Person, Prescription};
use crate::portal::{self, CostedFill};
use crate::session::SessionStore;
use crate::site::SiteKeys;

/// How many years back an export without `--since` reads at most. It stops
/// earlier, at the first year in which no one on the account had a fill.
pub const MAX_YEARS: i64 = 10;

/// Where the site commands keep sessions and what they read.
pub struct Env {
    pub sessions: SessionStore,
    /// The site's origin, or a test's loopback server.
    pub base: String,
    /// The least time between one answer and the next request.
    pub pace: Duration,
    /// The site's public keys, when known (the tests'); else read from
    /// env.js.
    pub keys: Option<SiteKeys>,
    /// Today, `YYYY-MM-DD`, for the tests; else the local date.
    pub today: Option<String>,
}

impl Env {
    /// The binary's: the vault, www.caremark.com, one request a second.
    pub fn for_binary() -> Result<Env, CliError> {
        Ok(Env {
            sessions: SessionStore::open()?,
            base: client::BASE.into(),
            pace: client::PACE,
            keys: None,
            today: None,
        })
    }
}

struct Ctx<'a> {
    cli: &'a Cli,
    client: Client,
    /// `YYYY-MM-DD`.
    today: String,
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
}

fn put(out: &mut dyn Write, v: &Value) -> Result<(), CliError> {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    writeln!(out, "{text}").map_err(|e| CliError::Other(format!("writing the output: {e}")))
}

/// The checks a command's arguments get before the session is read: a
/// refused command reads nothing.
pub fn check_args(cli: &Cli) -> Result<(), CliError> {
    match &cli.command {
        Command::Rx(RxCmd::List(args)) => range(&args.range).map(|_| ()),
        Command::Costs(args) => range(&args.range).map(|_| ()),
        Command::Records(RecordsCmd::Export(args)) => {
            if args.ndjson && cli.common.json {
                return Err(CliError::Usage("--ndjson is the output; drop --json".into()));
            }
            if !args.ndjson && args.output.is_none() {
                return Err(CliError::Usage(format!(
                    "say where the records go: --ndjson for stdout (`| medrec import --from {BIN}`) or -o <DIR>"
                )));
            }
            args.since.as_deref().map(iso).transpose()?;
            Ok(())
        }
        Command::Api(args) => client::api_request(args.method, &args.path, args.data.as_deref()).map(|_| ()),
        _ => Ok(()),
    }
}

/// One member-site command as `profile`, its documents to `out`. The exit
/// code is not 0 only for an export whose stream ended incomplete (the
/// trailer carries the error).
pub fn run(cli: &Cli, env: &Env, profile: &ProfileName, out: &mut dyn Write) -> Result<i32, CliError> {
    check_args(cli)?;
    let session = env.sessions.load(profile)?;
    let read_header = session.cookie_header.expose().to_string();
    let mut client = Client::new(&session, profile, &env.base, env.pace)?;
    if let Some(keys) = &env.keys {
        client = client.with_keys(keys.clone());
    }
    let today = env
        .today
        .clone()
        .unwrap_or_else(|| dates::fmt_iso(dates::today_local()));
    let ctx = Ctx { cli, client, today };
    let code = match &cli.command {
        Command::Patients(PatientsCmd::List) => patients(&ctx, out).map(|()| 0),
        Command::Rx(RxCmd::List(args)) => rx_list(&ctx, args, out).map(|()| 0),
        Command::Costs(args) => costs(&ctx, args, out).map(|()| 0),
        Command::Records(RecordsCmd::Export(args)) => export(&ctx, args, out),
        Command::Api(args) => {
            let request = client::api_request(args.method, &args.path, args.data.as_deref())?;
            ctx.client.api(&request).and_then(|doc| put(out, &doc)).map(|()| 0)
        }
        _ => Err(CliError::Other("not a member-site command".into())),
    }?;
    // Only after the command did what it was asked: an export that ended
    // incomplete may have ended on a refused session.
    if code == 0 {
        write_back(&ctx.client, &env.sessions, profile, &read_header);
    }
    Ok(code)
}

/// [`run`] for a command line, resolving its profile as the binary does:
/// for the in-process tests.
pub fn run_args(cli: &Cli, env: &Env, out: &mut dyn Write) -> Result<i32, CliError> {
    check_args(cli)?;
    let store = pk_cli_config::ConfigStore::new(BIN).with_override(cli.config.clone());
    let profiles = store.load::<crate::app::Config>()?.profiles;
    let profile = crate::source::site_profile(cli, &profiles)?;
    run(cli, env, &profile, out)
}

/// After a command's reads: the cookies the answers set, written back to
/// the profile's session, unless a newer sign-in replaced it meanwhile. A
/// failure is a warning; the output is complete and the stored session
/// still works.
fn write_back(client: &Client, store: &SessionStore, profile: &ProfileName, read_header: &str) {
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

/// A `YYYY-MM-DD` date, zero-padded, so it compares as a string.
pub fn iso(d: &str) -> Result<String, CliError> {
    Ok(dates::fmt_iso(dates::parse_iso(d.trim())?))
}

/// A list command's checked range: `since` and `until` as `YYYY-MM-DD`.
fn range(r: &RangeArgs) -> Result<(Option<String>, Option<String>), CliError> {
    r.validate()?;
    Ok((
        r.since.as_deref().map(iso).transpose()?,
        r.until.as_deref().map(iso).transpose()?,
    ))
}

// ---------------------------------------------------------------- reads

/// The people on the account, the signed-in member first.
fn people(ctx: &Ctx) -> Result<Vec<Person>, CliError> {
    let answer = ctx
        .client
        .experience(experience("member-family"), &json!({ "data": {} }))?;
    let people = portal::people(&answer);
    if people.is_empty() {
        return Err(CliError::NotFound(
            "the member site lists no one on this account (member-family); the site may have changed (docs/api.md)"
                .into(),
        ));
    }
    Ok(people)
}

/// The person `q` names: an id from `patients list`, or a first name.
/// Without `q`, the only person on the account.
fn pick<'a>(people: &'a [Person], q: Option<&str>) -> Result<&'a Person, CliError> {
    let Some(q) = q.map(str::trim).filter(|q| !q.is_empty()) else {
        return match people {
            [one] => Ok(one),
            many => Err(CliError::Usage(format!(
                "pass --patient <ID>: there are {} people on this account ({}); `{BIN} patients list` names them",
                many.len(),
                many.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ")
            ))),
        };
    };
    if let Some(p) = people.iter().find(|p| p.id == q) {
        return Ok(p);
    }
    let by_name: Vec<&Person> = people
        .iter()
        .filter(|p| p.first_name.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(q)))
        .collect();
    match by_name.as_slice() {
        [one] => Ok(one),
        [] => Err(CliError::NotFound(format!("no one on this account is `{q}`"))
            .with_remedy(format!("`{BIN} patients list` shows their ids"))),
        _ => Err(CliError::Usage(format!(
            "`{q}` names more than one person on this account; pass the id from `{BIN} patients list`"
        ))),
    }
}

/// A year's read: `[start, end]`, `YYYY-MM-DD`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Window {
    start: String,
    end: String,
}

/// The windows from `until` back, a year each (sharing their boundary day,
/// which the fills' ids dedupe): down to `since`, or [`MAX_YEARS`] back.
fn windows(since: Option<&str>, until: &str) -> Vec<Window> {
    let (y, m, d) = dates::parse_iso(until).expect("an ISO date");
    let mut out = Vec::new();
    for back in 0..MAX_YEARS {
        let end = shift((y - back, m, d));
        let start = shift((y - back - 1, m, d));
        match since {
            Some(s) if start.as_str() <= s => {
                out.push(Window {
                    start: s.to_string(),
                    end,
                });
                break;
            }
            _ => out.push(Window { start, end }),
        }
    }
    out
}

/// A day in another year: February 29th is the 28th in a common year.
fn shift((y, m, d): dates::Civil) -> String {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let d = if m == 2 && d == 29 && !leap { 28 } else { d };
    dates::fmt_iso((y, m, d))
}

/// One window's financial summary: every person's costed fills.
fn financial(ctx: &Ctx, w: &Window) -> Result<Vec<CostedFill>, CliError> {
    let body = json!({ "data": {
        "startDate": w.start,
        "endDate": w.end,
        "idType": "PBM_QL_PARTICIPANT_ID_TYPE",
        "profileId": "",
    }});
    let answer = ctx.client.experience(experience("financial-summary"), &body)?;
    portal::check_status(&answer, "data.getFinancialSummary", "financial summary")?;
    Ok(portal::financial(&answer))
}

/// One window's claims of `person`.
fn claims(ctx: &Ctx, person: &Person, w: &Window) -> Result<Vec<Fill>, CliError> {
    let body = json!({ "data": {
        "idType": "PBM_QL_PARTICIPANT_ID_TYPE",
        "personCode": person.key,
        "allClaims": true,
        "dedupRxClaims": true,
        "claimStartDate": w.start,
        "claimEndDate": w.end,
    }});
    let answer = ctx.client.experience(experience("claim-history"), &body)?;
    portal::check_status(&answer, "data.getClaimV2", "claim history")?;
    Ok(portal::claims(&answer, person))
}

/// `who`'s fills from `since` (or as far back as the account has fills)
/// to `until`, newest first, each once. A fill with no date is kept only
/// without `since`.
fn fills(ctx: &Ctx, who: &[&Person], since: Option<&str>, until: Option<&str>) -> Result<Vec<Fill>, CliError> {
    let until = until.unwrap_or(&ctx.today).to_string();
    let mut seen = HashSet::new();
    let mut all = Vec::new();
    for w in windows(since, &until) {
        let costed = financial(ctx, &w)?;
        let mut any = costed.iter().any(|c| who.iter().any(|p| p.key == c.person));
        for p in who {
            let mut found = claims(ctx, p, &w)?;
            any |= !found.is_empty();
            portal::add_costs(&mut found, p, &costed);
            for f in found {
                if seen.insert(f.id.clone()) {
                    all.push(f);
                }
            }
        }
        if since.is_none() && !any {
            break;
        }
    }
    all.retain(|f| match f.filled.as_deref() {
        Some(d) => since.is_none_or(|s| d >= s) && d <= until.as_str(),
        None => since.is_none(),
    });
    all.sort_by(|a, b| b.filled.cmp(&a.filled).then_with(|| a.id.cmp(&b.id)));
    Ok(all)
}

/// The prescriptions `fills` (newest first) are of: one per prescription
/// number, its drug and prescriber the latest fill's.
fn prescriptions(fills: &[Fill]) -> Vec<Prescription> {
    let mut out: Vec<Prescription> = Vec::new();
    for f in fills {
        let Some(rx) = &f.rx_number else {
            continue;
        };
        if out.iter().any(|p| p.patient == f.patient && &p.rx_number == rx) {
            continue;
        }
        out.push(Prescription {
            id: prescription_id(&f.patient, rx),
            patient: f.patient.clone(),
            rx_number: rx.clone(),
            drug: f.drug.clone(),
            prescriber: fills
                .iter()
                .filter(|g| g.patient == f.patient && g.rx_number.as_ref() == Some(rx))
                .map(|g| &g.prescriber)
                .find(|p| !p.is_empty())
                .cloned()
                .unwrap_or_default(),
            refills_authorized: fills
                .iter()
                .filter(|g| g.patient == f.patient && g.rx_number.as_ref() == Some(rx))
                .find_map(|g| g.refills_authorized),
            refills_remaining: None,
            status: None,
            last_filled: f.filled.clone(),
        });
    }
    out
}

// ------------------------------------------------------------- patients

/// The person as the stream header and `patients list` name them.
pub fn context(p: &Person) -> PatientContext {
    PatientContext {
        id: p.id.clone(),
        organization: fhir::ORGANIZATION.into(),
        label: None,
        relationship: p.signed_in.then(|| relationship::SELF.into()),
        kind: Some(kind::MEMBER.into()),
        member_role: p.role.map(str::to_string),
        fhir_user: None,
        name: p.name(),
        session: None,
    }
}

const PATIENT_COLUMNS: &[&str] = &["name", "id", "member_role", "relationship"];

fn patients(ctx: &Ctx, out: &mut dyn Write) -> Result<(), CliError> {
    let rows: Vec<PatientContext> = people(ctx)?.iter().map(context).collect();
    if ctx.json() {
        return put(
            out,
            &serde_json::to_value(pk_med_records::patient_list(rows)).unwrap_or_default(),
        );
    }
    let items: Vec<Value> = rows
        .iter()
        .map(|r| serde_json::to_value(r).unwrap_or_default())
        .collect();
    output::table(&output::table_view(&items, PATIENT_COLUMNS));
    Ok(())
}

// ------------------------------------------------------------------- rx

/// An `rx-list/v1` item: a prescription and its latest fill.
#[derive(Serialize)]
struct RxRow {
    patient: String,
    rx_number: Option<String>,
    drug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ndc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_filled: Option<String>,
    fills: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    days_supply: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pharmacy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prescriber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refills_authorized: Option<u32>,
}

const RX_COLUMNS: &[&str] = &[
    "last_filled",
    "drug",
    "quantity",
    "days_supply",
    "pharmacy",
    "fills",
    "rx_number",
];

/// Whom a list command reads: `--patient`'s person, or everyone.
fn who<'a>(people: &'a [Person], patient: Option<&str>) -> Result<Vec<&'a Person>, CliError> {
    match patient {
        Some(q) => Ok(vec![pick(people, Some(q))?]),
        None => Ok(people.iter().collect()),
    }
}

/// A list command's `since`: a year before `until` (or today) by
/// default, as the site's own pages read.
fn list_since(ctx: &Ctx, since: Option<String>, until: Option<&str>) -> String {
    since.unwrap_or_else(|| {
        let (y, m, d) = dates::parse_iso(until.unwrap_or(&ctx.today)).expect("an ISO date");
        shift((y - 1, m, d))
    })
}

fn rx_list(ctx: &Ctx, args: &RxListArgs, out: &mut dyn Write) -> Result<(), CliError> {
    let (since, until) = range(&args.range)?;
    let since = list_since(ctx, since, until.as_deref());
    let everyone = people(ctx)?;
    let who = who(&everyone, args.patient.as_deref())?;
    let fills = fills(ctx, &who, Some(&since), until.as_deref())?;
    let mut rows: Vec<RxRow> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for f in fills.iter().filter(|f| !f.reversed) {
        let key = (f.patient.clone(), f.rx_number.clone().unwrap_or_else(|| f.drug.key()));
        if seen.contains(&key) {
            continue;
        }
        let same: Vec<&Fill> = fills
            .iter()
            .filter(|g| {
                !g.reversed && g.patient == key.0 && g.rx_number.clone().unwrap_or_else(|| g.drug.key()) == key.1
            })
            .collect();
        seen.push(key);
        rows.push(RxRow {
            patient: f.patient.clone(),
            rx_number: f.rx_number.clone(),
            drug: f.drug.label(),
            ndc: f.drug.ndc.clone(),
            last_filled: f.filled.clone(),
            fills: same.len(),
            quantity: f.quantity.clone(),
            days_supply: f.days_supply,
            pharmacy: f.pharmacy.name.clone(),
            prescriber: same.iter().find_map(|g| g.prescriber.name.clone()),
            refills_authorized: same.iter().find_map(|g| g.refills_authorized),
        });
    }
    if let Some(n) = args.range.limit {
        rows.truncate(n as usize);
    }
    let items: Vec<Value> = rows
        .iter()
        .map(|r| serde_json::to_value(r).unwrap_or_default())
        .collect();
    if ctx.json() {
        return put(out, &serde_json::to_value(Paged::new("rx", items)).unwrap_or_default());
    }
    if items.is_empty() {
        ctx.note(&format!("no fills since {since}"));
    } else {
        output::table(&output::table_view(&items, RX_COLUMNS));
    }
    Ok(())
}

// ---------------------------------------------------------------- costs

/// A `cost-list/v1` item: one fill and what it cost.
#[derive(Serialize)]
struct CostRow {
    filled: Option<String>,
    patient: String,
    drug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rx_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pharmacy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_paid: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    you_paid: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deductible: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discount: Option<Money>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    reversed: bool,
}

/// The sums over the fills that were not reversed, each over the fills
/// that gave the amount.
#[derive(Serialize)]
struct CostTotals {
    fills: usize,
    total: Money,
    plan_paid: Money,
    you_paid: Money,
    deductible: Money,
    discount: Money,
}

const COST_COLUMNS: &[&str] = &[
    "filled",
    "drug",
    "pharmacy",
    "total",
    "plan_paid",
    "you_paid",
    "deductible",
];

fn costs(ctx: &Ctx, args: &CostsArgs, out: &mut dyn Write) -> Result<(), CliError> {
    let (since, until) = range(&args.range)?;
    let since = list_since(ctx, since, until.as_deref());
    let everyone = people(ctx)?;
    let who = who(&everyone, args.patient.as_deref())?;
    let mut fills = fills(ctx, &who, Some(&since), until.as_deref())?;
    if let Some(n) = args.range.limit {
        fills.truncate(n as usize);
    }
    let counted: Vec<&Fill> = fills.iter().filter(|f| !f.reversed).collect();
    let sum = |pick: fn(&Fill) -> Option<Cents>| counted.iter().filter_map(|f| pick(f)).sum::<Cents>().money();
    let totals = CostTotals {
        fills: counted.len(),
        total: sum(|f| f.amounts.total),
        plan_paid: sum(|f| f.amounts.plan_paid),
        you_paid: sum(|f| f.amounts.patient_paid),
        deductible: sum(|f| f.amounts.deductible),
        discount: sum(|f| f.amounts.discount),
    };
    let rows: Vec<Value> = fills
        .iter()
        .map(|f| {
            let row = CostRow {
                filled: f.filled.clone(),
                patient: f.patient.clone(),
                drug: f.drug.label(),
                rx_number: f.rx_number.clone(),
                pharmacy: f.pharmacy.name.clone(),
                total: f.amounts.total.map(Cents::money),
                plan_paid: f.amounts.plan_paid.map(Cents::money),
                you_paid: f.amounts.patient_paid.map(Cents::money),
                deductible: f.amounts.deductible.map(Cents::money),
                discount: f.amounts.discount.map(Cents::money),
                reversed: f.reversed,
            };
            serde_json::to_value(row).unwrap_or_default()
        })
        .collect();
    if ctx.json() {
        let doc = json!({
            "schema": "cost-list/v1",
            "since": since,
            "until": until,
            "items": rows,
            "totals": totals,
        });
        return put(out, &doc);
    }
    if rows.is_empty() {
        ctx.note(&format!("no fills since {since}"));
        return Ok(());
    }
    output::table(&output::table_view(&rows, COST_COLUMNS));
    println!(
        "\n{} fills: total {}, plan paid {}, you paid {}, deductible {}",
        totals.fills, totals.total.amount, totals.plan_paid.amount, totals.you_paid.amount, totals.deductible.amount
    );
    Ok(())
}

// -------------------------------------------------------------- records

pub fn organization() -> Organization {
    Organization {
        id: fhir::ORGANIZATION.into(),
        name: fhir::ORGANIZATION_NAME.into(),
        fhir_base: client::BASE.into(),
        vendor: Some(BIN.into()),
        location: None,
        aliases: vec!["Caremark".into(), "CVS Caremark Pharmacy Benefits".into()],
    }
}

/// The stream's header for `person`.
pub fn header(person: &Person, since: Option<&str>) -> Header {
    let mut h = Header::new(
        Source::new(BIN, env!("CARGO_PKG_VERSION")),
        organization(),
        context(person),
        now_rfc3339(),
    );
    h.method = Some(method::PORTAL.into());
    if let Some(s) = since {
        h.since = Some(s.into());
        // Exported whatever the date: the payer, the person, the plan and
        // the prescriptions the fills are of.
        h.since_unbounded = vec![
            "Coverage".into(),
            "MedicationRequest".into(),
            "Organization".into(),
            "Patient".into(),
        ];
    }
    h
}

/// The exit code of a stream that ended incomplete: its error's.
fn incomplete_exit(finished: &Finished) -> i32 {
    finished.trailer.error.as_ref().and_then(|e| e.exit_code).unwrap_or(1)
}

/// `records export`: one person's payer, Patient and Coverage, each
/// prescription (MedicationRequest), then each fill as a
/// MedicationDispense and its pharmacy ExplanationOfBenefit. The fills are
/// read first: a read that fails before the stream starts is the
/// command's error; one that fails after ends the stream incomplete, the
/// error in its trailer.
fn export(ctx: &Ctx, args: &ExportArgs, out: &mut dyn Write) -> Result<i32, CliError> {
    let since = args.since.as_deref().map(iso).transpose()?;
    let since = since.as_deref();
    let everyone = people(ctx)?;
    let person = pick(&everyone, args.patient.as_deref())?;
    let read = fills(ctx, &[person], since, None);
    let mut file = match &args.output {
        Some(dir) => Some(create_export_file(dir)?),
        None => None,
    };
    let finished = {
        let sink: &mut dyn Write = match file.as_mut() {
            Some(f) => f.file(),
            None => out,
        };
        let mut w = RecordsWriter::start(sink, &header(person, since))?;
        let written = read.and_then(|fills| {
            let coverage = fhir::coverage_id(person);
            for r in [fhir::organization(), fhir::patient(person), fhir::coverage(person)] {
                w.resource(&RawResource::from_value(&r)?)?;
            }
            let rxs = prescriptions(&fills);
            for rx in &rxs {
                w.resource(&RawResource::from_value(&fhir::medication_request(rx))?)?;
            }
            for f in &fills {
                let rx = f.rx_number.as_ref().map(|n| prescription_id(&f.patient, n));
                w.resource(&RawResource::from_value(&fhir::medication_dispense(f, rx.as_deref()))?)?;
                w.resource(&RawResource::from_value(&fhir::explanation_of_benefit(f, &coverage))?)?;
            }
            Ok(())
        });
        match written {
            Ok(()) => w.finish(now_rfc3339(), None)?,
            Err(e) => w.finish(now_rfc3339(), Some(&e))?,
        }
    };
    if let Some(file) = file {
        let path = file.commit(&finished)?;
        let summary = RecordsExport::new(path.display().to_string(), &finished);
        if ctx.json() {
            put(out, &serde_json::to_value(&summary).unwrap_or_default())?;
        } else {
            let counts: BTreeMap<&String, &u64> = summary.counts.iter().collect();
            ctx.note(&format!(
                "wrote {} ({} bytes{}): {}",
                summary.path,
                summary.bytes,
                if summary.complete { "" } else { ", incomplete" },
                counts
                    .iter()
                    .map(|(t, n)| format!("{n} {t}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(if finished.trailer.complete {
        0
    } else {
        incomplete_exit(&finished)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_go_back_a_year_at_a_time() {
        let w = windows(Some("2025-03-01"), "2026-11-19");
        assert_eq!(
            w,
            [
                Window {
                    start: "2025-11-19".into(),
                    end: "2026-11-19".into()
                },
                Window {
                    start: "2025-03-01".into(),
                    end: "2025-11-19".into()
                },
            ]
        );
        assert_eq!(windows(Some("2026-01-01"), "2026-11-19").len(), 1);
        assert_eq!(windows(None, "2026-11-19").len(), MAX_YEARS as usize);
        assert_eq!(windows(None, "2028-02-29")[0].start, "2027-02-28");
    }
}
