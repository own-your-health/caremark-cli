//! Every member-site command end to end, in-process, against a loopback
//! server that answers with the synthetic fixtures in `tests/fixtures/site/`
//! (their README has the household). Each runs from a command line through
//! `--profile adult1`, as the binary does (`site_cmd::run_args`), with a
//! temp config configuring the profile and the session in an in-memory
//! vault (`Vault::new(MemoryKeys::new(), dir)`): no keychain item is read
//! or written on any platform, and nothing is contacted beyond 127.0.0.1.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use caremark::cli::Cli;
use caremark::session::{Session, SessionStore};
use caremark::site_cmd::{self, Env};
use clap::{CommandFactory, FromArgMatches};
use pk_cli_auth::vault::{MemoryKeys, Vault};
use pk_cli_config::profile::ProfileName;
use pk_cli_secrets::Secret;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/site");
const COOKIES: &str = "CVS_ONESITE_TOKEN=dummy-token; visitor=dummy";
const AGENT: &str = "Mozilla/5.0 (synthetic test browser)";
const TODAY: &str = "2026-11-19";
const JSON: &str = "application/json";

const FAMILY: &str = "e496ecd3-4067-4499-a1cd-27e353397cb4";
const CLAIMS: &str = "19378b96-0bc0-43c1-a37c-9308e71a22f1";
const FINANCIAL: &str = "13ed331c-3e9c-4fdf-bab2-6396a12a3623";

fn tmp(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "caremark-site-e2e-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap()).unwrap()
}

/// One request the server saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

/// An answer the server gives instead of the fixtures': to the `nth`
/// request (counting from 0) for an experience id, or for every one when
/// `nth` is `None`.
#[derive(Clone)]
struct Fault {
    experience: &'static str,
    nth: Option<usize>,
    status: u16,
    content_type: &'static str,
    body: String,
}

#[derive(Default)]
struct State {
    seen: Vec<Seen>,
    faults: Vec<Fault>,
    set_cookie: Option<String>,
}

struct Server {
    base: String,
    state: Arc<Mutex<State>>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                std::thread::spawn(move || serve(stream, &state));
            }
        });
        Server { base, state }
    }

    fn fault(&self, f: Fault) {
        self.state.lock().unwrap().faults.push(f);
    }

    fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let seen = Seen {
        method,
        path,
        headers,
        body,
    };
    let (status, content_type, text, cookie) = {
        let mut st = state.lock().unwrap();
        st.seen.push(seen.clone());
        let id = seen.headers.get("x-experienceid").cloned().unwrap_or_default();
        let nth = st
            .seen
            .iter()
            .filter(|s| s.headers.get("x-experienceid") == Some(&id))
            .count()
            .saturating_sub(1);
        let fault = st
            .faults
            .iter()
            .find(|f| f.experience == id && f.nth.is_none_or(|n| n == nth))
            .cloned();
        let (status, content_type, text) = match fault {
            Some(f) => (f.status, f.content_type, f.body),
            None => answer(&seen),
        };
        (status, content_type, text, st.set_cookie.clone())
    };
    let mut out = stream;
    let mut head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        text.len()
    );
    if let Some(c) = cookie {
        head.push_str(&format!("Set-Cookie: {c}\r\n"));
    }
    head.push_str("\r\n");
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(text.as_bytes());
}

/// Whether `date` (any of the site's forms) is in `[start, end]`.
fn within(date: &str, start: &str, end: &str) -> bool {
    let d = &date[..10];
    d >= start && d <= end
}

/// The fixtures' answer to a request, the dated reads cut to the request's
/// window as the site does.
fn answer(seen: &Seen) -> (u16, &'static str, String) {
    if seen.method == "GET" && seen.path == "/memberportal/env.js" {
        let js = std::fs::read_to_string(format!("{FIXTURES}/env.js")).unwrap();
        return (200, "application/javascript", js);
    }
    let id = seen.headers.get("x-experienceid").map(String::as_str).unwrap_or("");
    assert_eq!(seen.method, "POST", "an experience read is a POST");
    assert_eq!(seen.path, format!("/api/client/experience/v2/load/{id}"));
    assert_eq!(
        seen.headers.get("x-api-key").map(String::as_str),
        Some("dummy-experience-key")
    );
    assert_eq!(seen.headers.get("x-channel-type").map(String::as_str), Some("WEB"));
    // The stored session's cookies, or the jar a Set-Cookie rotated.
    let cookie = seen.headers.get("cookie").map(String::as_str).unwrap_or("");
    assert!(
        cookie.contains("CVS_ONESITE_TOKEN=") && cookie.contains("visitor=dummy"),
        "{cookie}"
    );
    assert_eq!(seen.headers.get("user-agent").map(String::as_str), Some(AGENT));
    let data = &seen.body["data"];
    let doc = match id {
        FAMILY => fixture("member-family.json"),
        CLAIMS => {
            let (start, end) = (
                data["claimStartDate"].as_str().unwrap(),
                data["claimEndDate"].as_str().unwrap(),
            );
            let code = data["personCode"].as_str().unwrap();
            let mut doc = fixture("claim-history.json");
            let lists = doc["data"]["getClaimV2"]["memberClaimList"].as_array_mut().unwrap();
            lists.retain(|l| l["personCode"] == code);
            for l in lists.iter_mut() {
                l["claims"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|c| within(c["fillDate"].as_str().unwrap(), start, end));
            }
            doc
        }
        FINANCIAL => {
            let (start, end) = (data["startDate"].as_str().unwrap(), data["endDate"].as_str().unwrap());
            let mut doc = fixture("financial-summary.json");
            for p in doc["data"]["getFinancialSummary"]["patients"].as_array_mut().unwrap() {
                for l in p["linkedIDs"].as_array_mut().unwrap() {
                    for rx in l["prescriptionforPatient"].as_array_mut().unwrap() {
                        rx["fills"]
                            .as_array_mut()
                            .unwrap()
                            .retain(|f| within(f["dateTimeFilled"].as_str().unwrap(), start, end));
                    }
                }
            }
            doc
        }
        _ => return (404, JSON, "{}".into()),
    };
    (200, JSON, doc.to_string())
}

fn adult1() -> ProfileName {
    ProfileName::parse("adult1").unwrap()
}

/// A household's command line: a temp config with profile `adult1`, its
/// session in an in-memory vault, the loopback server unpaced.
struct Household {
    server: Server,
    env: Env,
    config: PathBuf,
    dir: PathBuf,
}

impl Household {
    fn new() -> Household {
        let dir = tmp("house");
        let config = dir.join("config.json");
        std::fs::write(&config, r#"{"profiles":{"adult1":{}}}"#).unwrap();
        let sessions = SessionStore::with_vault(Vault::new(MemoryKeys::new(), dir.join("vault")));
        sessions
            .put(
                &adult1(),
                &Session {
                    cookie_header: Secret::new(COOKIES),
                    user_agent: Some(AGENT.into()),
                    captured_at: "2026-01-02T03:04:05Z".into(),
                },
            )
            .unwrap();
        let server = Server::start();
        let env = Env {
            sessions,
            base: server.base.clone(),
            pace: Duration::ZERO,
            keys: None,
            today: Some(TODAY.into()),
        };
        Household {
            server,
            env,
            config,
            dir,
        }
    }

    /// `caremark --config <tmp> --profile adult1 <args>`: the exit code and
    /// what went to stdout.
    fn run(&self, args: &[&str]) -> Result<(i32, Vec<u8>), pk_cli_core::CliError> {
        let mut argv = vec![
            "caremark",
            "--config",
            self.config.to_str().unwrap(),
            "--profile",
            "adult1",
        ];
        argv.extend_from_slice(args);
        let matches = Cli::command().try_get_matches_from(&argv).unwrap();
        let cli = Cli::from_arg_matches(&matches).unwrap();
        let mut out = Vec::new();
        let code = site_cmd::run_args(&cli, &self.env, &mut out)?;
        Ok((code, out))
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.push("--json");
        let (code, out) = self.run(&args).unwrap();
        assert_eq!(code, 0);
        serde_json::from_slice(&out).unwrap()
    }

    fn stream(&self, args: &[&str]) -> (i32, Vec<Value>) {
        let (code, out) = self.run(args).unwrap();
        let lines = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (code, lines)
    }

    fn stored(&self) -> Session {
        self.env.sessions.get(&adult1()).unwrap().unwrap()
    }

    fn reads(&self, experience: &str) -> Vec<Seen> {
        self.server
            .seen()
            .into_iter()
            .filter(|s| s.headers.get("x-experienceid").map(String::as_str) == Some(experience))
            .collect()
    }
}

fn of_type<'a>(lines: &'a [Value], t: &str) -> Vec<&'a Value> {
    lines.iter().filter(|l| l["resourceType"] == t).collect()
}

#[test]
fn patients_lists_the_account_signed_in_member_first() {
    let h = Household::new();
    let doc = h.json(&["patients", "list"]);
    assert_eq!(doc["schema"], "patient-list/v1");
    let items = doc["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["name"], "Alex Example");
    assert_eq!(items[0]["relationship"], "self");
    assert_eq!(items[0]["member_role"], "subscriber");
    assert_eq!(items[0]["organization"], "caremark");
    assert_eq!(items[0]["kind"], "member");
    assert_eq!(items[1]["member_role"], "dependent");
    assert!(items[1].get("relationship").is_none());
    let id = items[0]["id"].as_str().unwrap();
    assert!(id.starts_with('p') && id.len() == 13, "{id}");
    // The family read only; env.js once for the keys.
    let seen = h.server.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].path, "/memberportal/env.js");
    assert_eq!(seen[1].body, json!({ "data": {} }));
}

#[test]
fn rx_list_is_one_row_per_prescription_with_its_latest_fill() {
    let h = Household::new();
    let doc = h.json(&["rx", "list", "--patient", "alex", "--since", "2025-01-01"]);
    assert_eq!(doc["schema"], "rx-list/v1");
    assert_eq!(doc["total"], 2);
    assert_eq!(doc["left_out"]["undated"], 0);
    let items = doc["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0]["drug"], "EXAMPLEPRIL 10 MG TABLET");
    assert_eq!(items[0]["last_filled"], "2026-10-01");
    assert_eq!(items[0]["fills"], 2);
    assert_eq!(items[0]["ndc"], "00000000001");
    assert_eq!(items[0]["prescriber"], "Sam Example");
    // The reversed fill does not count; the 2025 one does.
    assert_eq!(items[1]["drug"], "SAMPLESTATIN 20 MG TABLET");
    assert_eq!(items[1]["last_filled"], "2025-06-10");
    assert_eq!(items[1]["fills"], 1);
    // Two windows back from today to --since: claims and costs for each.
    let claims = h.reads(CLAIMS);
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0].body["data"]["claimStartDate"], "2025-11-19");
    assert_eq!(claims[0].body["data"]["claimEndDate"], TODAY);
    assert_eq!(claims[1].body["data"]["claimStartDate"], "2025-01-01");
    assert_eq!(claims[0].body["data"]["personCode"], "001");
    assert_eq!(h.reads(FINANCIAL).len(), 2);
}

#[test]
fn costs_sum_what_was_paid_and_leave_reversed_fills_out() {
    let h = Household::new();
    let doc = h.json(&["costs", "--since", "2025-01-01"]);
    assert_eq!(doc["schema"], "cost-list/v1");
    let items = doc["items"].as_array().unwrap();
    // Alex's four claims and Sam's one. Sam's TESTOCILLIN fill, which only
    // the financial summary lists, is left out: it could be a claim the two
    // reads write differently.
    assert_eq!(items.len(), 5, "{items:?}");
    assert!(items.iter().all(|i| i["drug"] != "TESTOCILLIN 250 MG/5ML SUSPENSION"));
    // The JSON says so too, not only stderr.
    assert_eq!(doc["left_out"], json!({ "unmatched": 1, "undated": 0 }));
    assert_eq!(doc["total"], 5);
    assert_eq!(items[0]["filled"], "2026-10-01");
    assert_eq!(items[0]["you_paid"]["amount"], "10.00");
    // The claim gave no amounts; the financial summary did.
    let september = items.iter().find(|i| i["filled"] == "2026-09-01").unwrap();
    assert_eq!(september["total"]["amount"], "12.50");
    assert_eq!(september["deductible"]["amount"], "5.00");
    assert_eq!(september["discount"]["amount"], "3.00");
    let reversed = items.iter().find(|i| i["filled"] == "2026-08-15").unwrap();
    assert_eq!(reversed["reversed"], true);
    let totals = &doc["totals"];
    // Sam's claim gives no amounts, so it adds a fill and no money.
    assert_eq!(totals["fills"], 4);
    assert_eq!(totals["total"]["amount"], "115.00");
    assert_eq!(totals["plan_paid"]["amount"], "65.00");
    assert_eq!(totals["you_paid"]["amount"], "50.00");
    assert_eq!(totals["deductible"]["amount"], "35.00");
    assert_eq!(totals["discount"]["amount"], "3.00");
}

#[test]
fn an_export_is_one_persons_fills_as_fhir() {
    let h = Household::new();
    let (code, lines) = h.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    assert_eq!(code, 0);
    let header = &lines[0];
    assert_eq!(header["schema"], "records-stream/v1");
    assert_eq!(header["method"], "portal");
    assert_eq!(header["organization"]["id"], "caremark");
    assert_eq!(header["patient"]["name"], "Alex Example");
    // Without --since, the last ten years, every one of them read.
    assert_eq!(header["since"], "2016-11-19");
    assert_eq!(h.reads(CLAIMS).len(), 10);
    assert_eq!(h.reads(FINANCIAL).len(), 10);
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["schema"], "records-stream-end/v1");
    assert_eq!(trailer["complete"], true);
    assert_eq!(
        trailer["counts"],
        json!({
            "Coverage": 1,
            "ExplanationOfBenefit": 4,
            "MedicationDispense": 4,
            "MedicationRequest": 2,
            "Organization": 1,
            "Patient": 1,
        })
    );
    let dispenses = of_type(&lines, "MedicationDispense");
    assert_eq!(dispenses[0]["whenHandedOver"], "2026-10-01");
    assert_eq!(
        dispenses[0]["medicationCodeableConcept"]["coding"][0]["code"],
        "00000000001"
    );
    assert_eq!(dispenses[0]["performer"][0]["actor"]["display"], "Example Pharmacy");
    let reversed = dispenses.iter().find(|d| d["whenHandedOver"] == "2026-08-15").unwrap();
    assert_eq!(reversed["status"], "cancelled");
    let request = of_type(&lines, "MedicationRequest")[0];
    assert_eq!(
        dispenses[0]["authorizingPrescription"][0]["reference"],
        format!("MedicationRequest/{}", request["id"].as_str().unwrap())
    );
    let eob = of_type(&lines, "ExplanationOfBenefit")[0];
    assert_eq!(eob["id"], dispenses[0]["id"]);
    assert_eq!(eob["type"]["coding"][0]["code"], "pharmacy");
    let patient = of_type(&lines, "Patient")[0];
    assert_eq!(
        eob["patient"]["reference"],
        format!("Patient/{}", patient["id"].as_str().unwrap())
    );
    // Every resource dated as read in this pull, a full instant.
    let pulled_at = header["pulled_at"].as_str().unwrap();
    assert!(pulled_at.len() == 20 && pulled_at.ends_with('Z'), "{pulled_at}");
    for r in &lines[1..lines.len() - 1] {
        if r["resourceType"] != "MedicationRequest" {
            assert_eq!(r["meta"]["lastUpdated"], pulled_at, "{}", r["resourceType"]);
        }
    }
    // A prescription is dated by its latest fill, so a narrower pull of the
    // same fills never outranks a fuller one.
    let requests = of_type(&lines, "MedicationRequest");
    let mut dated: Vec<&str> = requests
        .iter()
        .map(|r| r["meta"]["lastUpdated"].as_str().unwrap())
        .collect();
    dated.sort();
    assert_eq!(dated, ["2026-08-15T00:00:00Z", "2026-10-01T00:00:00Z"]);
    // Only Alex's claims were read.
    assert!(h.reads(CLAIMS).iter().all(|s| s.body["data"]["personCode"] == "001"));
}

#[test]
fn an_export_since_a_day_bounds_the_fills_only() {
    let h = Household::new();
    let (code, lines) = h.stream(&[
        "records",
        "export",
        "--patient",
        "Alex",
        "--since",
        "2026-09-01",
        "--ndjson",
    ]);
    assert_eq!(code, 0);
    assert_eq!(lines[0]["since"], "2026-09-01");
    assert_eq!(
        lines[0]["since_unbounded"],
        json!(["Coverage", "Organization", "Patient"])
    );
    assert_eq!(of_type(&lines, "MedicationDispense").len(), 2);
    assert_eq!(h.reads(CLAIMS).len(), 1);
}

#[test]
fn an_export_to_a_folder_prints_its_summary() {
    let h = Household::new();
    let dir = h.dir.join("out");
    let doc = h.json(&["records", "export", "--patient", "Alex", "-o", dir.to_str().unwrap()]);
    assert_eq!(doc["schema"], "records-export/v1");
    assert_eq!(doc["complete"], true);
    assert_eq!(doc["counts"]["MedicationDispense"], 4);
    let file = dir.join("records.ndjson");
    assert_eq!(doc["path"], file.display().to_string());
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.lines().count(), 1 + 13 + 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o077, 0);
    }
}

/// The family answer as another account would give it: the same people
/// and person codes, another signed-in member's id (or none).
fn family_of_another_account(id: Option<&str>) -> String {
    let mut v: Value =
        serde_json::from_str(&std::fs::read_to_string("tests/fixtures/site/member-family.json").unwrap()).unwrap();
    match id {
        Some(id) => v["data"]["getPatientV2"]["id"] = json!(id),
        None => {
            v["data"]["getPatientV2"].as_object_mut().unwrap().remove("id");
        }
    }
    v.to_string()
}

fn ids(lines: &[Value]) -> Vec<String> {
    lines[1..lines.len() - 1]
        .iter()
        .filter(|r| r["resourceType"] != "Organization")
        .map(|r| format!("{}/{}", r["resourceType"].as_str().unwrap(), r["id"].as_str().unwrap()))
        .collect()
}

#[test]
fn the_same_person_code_on_another_account_is_another_person() {
    let one = Household::new();
    let (_, mine) = one.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    let other = Household::new();
    other.server.fault(Fault {
        experience: FAMILY,
        nth: None,
        status: 200,
        content_type: JSON,
        body: family_of_another_account(Some("SYN0000009")),
    });
    let (code, theirs) = other.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    assert_eq!(code, 0);
    assert_ne!(mine[0]["patient"]["id"], theirs[0]["patient"]["id"]);
    // Patient, Coverage, the prescriptions and the fills: no id in common.
    let (a, b) = (ids(&mine), ids(&theirs));
    assert_eq!(a.len(), b.len());
    assert!(a.iter().all(|id| !b.contains(id)), "{a:?} {b:?}");
}

#[test]
fn a_family_without_the_members_own_id_is_the_sites_failure() {
    let h = Household::new();
    h.server.fault(Fault {
        experience: FAMILY,
        nth: None,
        status: 200,
        content_type: JSON,
        body: family_of_another_account(None),
    });
    let e = h.run(&["patients", "list"]).unwrap_err();
    assert_eq!(e.exit_code(), 5);
    assert!(e.message().contains("getPatientV2.id"), "{}", e.message());
}

#[test]
fn two_same_day_fills_with_no_claim_number_are_two() {
    let h = Household::new();
    let claim = json!({
        "rxNumber": "0000009",
        "claimStatus": "PAID",
        "fillDate": "2026-10-05",
        "drug": { "name": "EXAMPLEPRIL", "ndcId": "00000-0000-01" },
    });
    let body = json!({ "data": { "getClaimV2": {
        "statusCode": "0000",
        "memberClaimList": [{ "personCode": "001", "claims": [claim.clone(), claim] }],
    }}})
    .to_string();
    h.server.fault(Fault {
        experience: CLAIMS,
        nth: None,
        status: 200,
        content_type: JSON,
        body,
    });
    let (code, lines) = h.stream(&[
        "records",
        "export",
        "--patient",
        "Alex",
        "--since",
        "2025-01-01",
        "--ndjson",
    ]);
    assert_eq!(code, 0);
    // Both windows answer both claims: two fills, each once.
    let dispenses = of_type(&lines, "MedicationDispense");
    assert_eq!(dispenses.len(), 2);
    assert_ne!(dispenses[0]["id"], dispenses[1]["id"]);
}

#[test]
fn a_prescription_is_the_same_from_a_narrow_pull_and_a_full_one() {
    let h = Household::new();
    let (_, full) = h.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    let (_, narrow) = h.stream(&[
        "records",
        "export",
        "--patient",
        "Alex",
        "--since",
        "2026-09-15",
        "--ndjson",
    ]);
    let request = |lines: &[Value]| {
        of_type(lines, "MedicationRequest")
            .into_iter()
            .find(|r| r["identifier"][0]["value"] == "0000001")
            .unwrap()
            .clone()
    };
    assert_eq!(request(&full), request(&narrow));
}

#[test]
fn an_export_to_a_folder_that_cannot_be_written_reads_nothing() {
    let h = Household::new();
    let blocker = h.dir.join("a-file");
    std::fs::write(&blocker, "").unwrap();
    let e = h
        .run(&[
            "records",
            "export",
            "--patient",
            "Alex",
            "-o",
            blocker.join("out").to_str().unwrap(),
        ])
        .unwrap_err();
    assert_ne!(e.exit_code(), 0);
    assert!(h.server.seen().is_empty());
}

#[test]
fn claims_under_another_key_are_the_sites_failure_not_an_empty_account() {
    let h = Household::new();
    // Status 0000, but the list is not where the site's scripts read it.
    h.server.fault(Fault {
        experience: CLAIMS,
        nth: None,
        status: 200,
        content_type: JSON,
        body: json!({ "data": { "getClaimV2": { "statusCode": "0000", "claimList": [] } } }).to_string(),
    });
    let (code, lines) = h.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    assert_eq!(code, 5);
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["complete"], false);
    assert!(
        trailer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("gave no claim for"),
        "{trailer}"
    );
    assert_eq!(h.run(&["costs", "--patient", "Alex"]).unwrap_err().exit_code(), 5);
    // Someone the financial summary lists nothing for is an empty account:
    // Jordan has no fills, and that is no failure.
    let doc = h.json(&["costs", "--patient", "Jordan"]);
    assert_eq!(doc["items"].as_array().unwrap().len(), 0);
}

#[test]
fn an_export_to_a_folder_cut_short_is_named_incomplete() {
    let h = Household::new();
    h.server.fault(Fault {
        experience: CLAIMS,
        nth: Some(1),
        status: 401,
        content_type: JSON,
        body: "{}".into(),
    });
    let dir = h.dir.join("cut");
    let (code, out) = h
        .run(&[
            "records",
            "export",
            "--patient",
            "Alex",
            "-o",
            dir.to_str().unwrap(),
            "--json",
        ])
        .unwrap();
    assert_eq!(code, 3);
    let doc: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["schema"], "records-export/v1");
    assert_eq!(doc["complete"], false);
    let file = dir.join("records.incomplete.ndjson");
    assert_eq!(doc["path"], file.display().to_string());
    assert!(file.exists());
    assert!(!dir.join("records.ndjson").exists());
}

#[test]
fn an_export_the_site_cut_short_keeps_its_cookies() {
    let h = Household::new();
    h.server.fault(Fault {
        experience: CLAIMS,
        nth: Some(1),
        status: 503,
        content_type: JSON,
        body: "{}".into(),
    });
    h.server.state.lock().unwrap().set_cookie = Some("CVS_ONESITE_TOKEN=rotated; Path=/".into());
    let (code, lines) = h.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    assert_eq!(code, 5);
    assert_eq!(lines.last().unwrap()["complete"], false);
    // The site failed, not the session: the answers' cookies are kept.
    assert!(h.stored().cookie_header.expose().contains("CVS_ONESITE_TOKEN=rotated"));
}

#[test]
fn a_fill_on_a_shared_boundary_day_is_counted_once() {
    let h = Household::new();
    // One costed fill no claim has, on the day the two windows share.
    let body = json!({ "data": { "getFinancialSummary": {
        "statusCode": "0000",
        "patients": [{
            "patient": { "personCode": "001" },
            "prescriptionforPatient": [{
                "drugInfo": { "drug": { "name": "TESTOCILLIN", "ndcId": "00000000004" } },
                "fills": [{ "dateTimeFilled": "2025-11-19T00:00:00", "patientPayAmount": 5 }],
            }],
        }],
    }}})
    .to_string();
    h.server.fault(Fault {
        experience: FINANCIAL,
        nth: None,
        status: 200,
        content_type: JSON,
        body,
    });
    let (code, lines) = h.stream(&[
        "records",
        "export",
        "--patient",
        "Alex",
        "--since",
        "2025-01-01",
        "--ndjson",
    ]);
    assert_eq!(code, 0);
    assert_eq!(h.reads(FINANCIAL).len(), 2);
    let skipped = lines.last().unwrap()["skipped"].as_array().unwrap().clone();
    let unmatched: Vec<&Value> = skipped.iter().filter(|k| k["code"] == "unmatched").collect();
    assert_eq!(unmatched.len(), 1, "{skipped:?}");
    assert!(
        unmatched[0]["reason"].as_str().unwrap().starts_with("1 fill "),
        "{skipped:?}"
    );
}

#[test]
fn costs_limit_shortens_the_rows_not_the_totals() {
    let h = Household::new();
    let doc = h.json(&["costs", "--patient", "Alex", "--since", "2025-01-01", "--limit", "1"]);
    assert_eq!(doc["items"].as_array().unwrap().len(), 1);
    assert_eq!(doc["total"], 4);
    assert_eq!(doc["totals"]["fills"], 3);
    assert_eq!(doc["totals"]["total"]["amount"], "115.00");
}

#[test]
fn the_since_limit_is_counted_from_the_runs_today() {
    let h = Household::new();
    // TODAY is 2026-11-19: 30 years back is 1996-11-19.
    let e = h
        .run(&["costs", "--patient", "Alex", "--since", "1996-11-18"])
        .unwrap_err();
    assert_eq!(e.exit_code(), 2);
    assert!(e.message().contains("1996-11-19 or later"), "{}", e.message());
    assert!(h.server.seen().is_empty());
    let doc = h.json(&["costs", "--patient", "Alex", "--since", "1996-11-19"]);
    assert_eq!(doc["schema"], "cost-list/v1");
    assert_eq!(h.reads(CLAIMS).len(), 30);
}

#[test]
fn a_session_refused_partway_keeps_the_years_read_before_it() {
    let h = Household::new();
    // The second year's claims: the first year's three fills are read.
    h.server.fault(Fault {
        experience: CLAIMS,
        nth: Some(1),
        status: 401,
        content_type: JSON,
        body: "{}".into(),
    });
    h.server.state.lock().unwrap().set_cookie = Some("CVS_ONESITE_TOKEN=rotated; Path=/".into());
    let (code, lines) = h.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    assert_eq!(code, 3);
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["complete"], false);
    assert_eq!(trailer["error"]["exit_code"], 3);
    assert!(trailer["error"]["message"].as_str().unwrap().contains("HTTP 401"));
    assert_eq!(trailer["counts"]["MedicationDispense"], 3);
    assert_eq!(trailer["counts"]["ExplanationOfBenefit"], 3);
    let dates: Vec<&str> = of_type(&lines, "MedicationDispense")
        .iter()
        .map(|d| d["whenHandedOver"].as_str().unwrap())
        .collect();
    assert_eq!(dates, ["2026-10-01", "2026-09-01", "2026-08-15"]);
    // No year after the refusal is asked for.
    assert_eq!(h.reads(CLAIMS).len(), 2);
    // An incomplete export writes no rotated cookie back.
    assert_eq!(h.stored().cookie_header.expose(), COOKIES);
}

#[test]
fn a_refusal_before_the_first_fill_ends_an_empty_stream_incomplete() {
    let h = Household::new();
    h.server.fault(Fault {
        experience: FINANCIAL,
        nth: Some(0),
        status: 503,
        content_type: JSON,
        body: "{}".into(),
    });
    let (code, lines) = h.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    assert_eq!(code, 5);
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["complete"], false);
    assert_eq!(trailer["counts"]["MedicationDispense"], Value::Null);
    // The person and the plan are still written: they were read.
    assert_eq!(trailer["counts"]["Patient"], 1);
}

#[test]
fn a_fill_only_the_financial_summary_lists_is_a_skip() {
    let h = Household::new();
    let (code, lines) = h.stream(&["records", "export", "--patient", "Sam", "--ndjson"]);
    assert_eq!(code, 0);
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["complete"], true);
    // Sam's one claim; the TESTOCILLIN fill no claim has is the skip.
    let dispenses = of_type(&lines, "MedicationDispense");
    assert_eq!(dispenses.len(), 1);
    assert_eq!(dispenses[0]["whenHandedOver"], "2026-05-05");
    let skipped = trailer["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0]["code"], "unmatched");
    assert_eq!(skipped[0]["resource_type"], "MedicationDispense");
    assert!(skipped[0]["reason"].as_str().unwrap().starts_with("1 fill "));
    // Alex's fills all match a claim.
    let (_, lines) = h.stream(&["records", "export", "--patient", "Alex", "--ndjson"]);
    assert!(lines
        .last()
        .unwrap()
        .get("skipped")
        .is_none_or(|s| s.as_array().unwrap().is_empty()));
}

/// A claim-history answer of Alex's: one claim on `dated` (if any) and one
/// whose date of service is missing.
fn claims_with_an_undated_one(dated: Option<&str>) -> String {
    let claim = |number: &str, date: Option<&str>| {
        let mut c = json!({
            "rxNumber": "0000009",
            "fillNumber": 0,
            "rxclaimNumber": number,
            "claimStatus": "PAID",
            "drug": { "name": "EXAMPLEPRIL", "ndcId": "00000-0000-01" },
        });
        if let Some(d) = date {
            c["fillDate"] = json!(d);
        }
        c
    };
    let mut claims = vec![claim("4444444444", None)];
    if let Some(d) = dated {
        claims.push(claim("5555555555", Some(d)));
    }
    json!({ "data": { "getClaimV2": {
        "statusCode": "0000",
        "memberClaimList": [{ "personCode": "001", "claims": claims }],
    }}})
    .to_string()
}

#[test]
fn an_undated_claim_is_a_skip() {
    let h = Household::new();
    h.server.fault(Fault {
        experience: CLAIMS,
        nth: Some(0),
        status: 200,
        content_type: JSON,
        body: claims_with_an_undated_one(Some("2026-10-05")),
    });
    let (code, lines) = h.stream(&[
        "records",
        "export",
        "--patient",
        "Alex",
        "--ndjson",
        "--since",
        "2026-01-01",
    ]);
    assert_eq!(code, 0);
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["complete"], true);
    assert_eq!(trailer["counts"]["MedicationDispense"], 1);
    // The override replaced Alex's claims, so the financial summary's two
    // fills of them match nothing: an `unmatched` skip beside this one.
    let skipped = trailer["skipped"].as_array().unwrap();
    let undated: Vec<&Value> = skipped.iter().filter(|k| k["code"] == "undated").collect();
    assert_eq!(undated.len(), 1, "{skipped:?}");
    assert_eq!(undated[0]["resource_type"], "MedicationDispense");
    assert!(undated[0]["reason"].as_str().unwrap().starts_with("1 fill "));
}

#[test]
fn claims_that_are_all_undated_are_the_sites_failure() {
    let h = Household::new();
    h.server.fault(Fault {
        experience: CLAIMS,
        nth: None,
        status: 200,
        content_type: JSON,
        body: claims_with_an_undated_one(None),
    });
    let (code, lines) = h.stream(&[
        "records",
        "export",
        "--patient",
        "Alex",
        "--ndjson",
        "--since",
        "2026-01-01",
    ]);
    assert_eq!(code, 5);
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["complete"], false);
    assert!(
        trailer["error"]["message"].as_str().unwrap().contains("carry no date"),
        "{trailer}"
    );
    let e = h.run(&["costs", "--patient", "Alex"]).unwrap_err();
    assert_eq!(e.exit_code(), 5);
}

#[test]
fn a_since_further_back_than_ten_years_is_read_to() {
    let h = Household::new();
    let (code, lines) = h.stream(&[
        "records",
        "export",
        "--patient",
        "Alex",
        "--ndjson",
        "--since",
        "2014-01-01",
    ]);
    assert_eq!(code, 0);
    assert_eq!(lines[0]["since"], "2014-01-01");
    let claims = h.reads(CLAIMS);
    assert_eq!(claims.len(), 13);
    assert_eq!(claims.last().unwrap().body["data"]["claimStartDate"], "2014-01-01");
}

#[test]
fn an_until_after_today_reads_no_year_after_it() {
    let h = Household::new();
    let doc = h.json(&[
        "costs",
        "--patient",
        "Alex",
        "--since",
        "2026-01-01",
        "--until",
        "2206-01-01",
    ]);
    assert_eq!(doc["items"].as_array().unwrap().len(), 3);
    let claims = h.reads(CLAIMS);
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].body["data"]["claimEndDate"], TODAY);
}

#[test]
fn costs_print_a_table_and_the_totals() {
    let h = Household::new();
    let (code, out) = h.run(&["costs", "--since", "2025-01-01", "--patient", "Alex"]).unwrap();
    assert_eq!(code, 0);
    let text = String::from_utf8(out).unwrap();
    assert!(text.lines().next().unwrap().starts_with("FILLED | "), "{text}");
    assert_eq!(text.lines().filter(|l| l.starts_with("20")).count(), 4, "{text}");
    assert!(
        text.trim_end().ends_with(
            "Over 3 fills (reversed claims left out): total $115.00, plan paid $65.00, you paid $50.00, deductible $35.00, discounts $3.00"
        ),
        "{text}"
    );
}

#[test]
fn a_rotated_cookie_is_written_back_after_success() {
    let h = Household::new();
    h.server.state.lock().unwrap().set_cookie = Some("CVS_ONESITE_TOKEN=rotated; Path=/".into());
    h.json(&["patients", "list"]);
    assert!(h.stored().cookie_header.expose().contains("CVS_ONESITE_TOKEN=rotated"));
    assert_eq!(h.stored().captured_at, "2026-01-02T03:04:05Z");
}

#[test]
fn the_sites_refusals_are_exit_codes() {
    let cases: [(u16, &str, &str, i32, &str); 5] = [
        (401, JSON, "{}", 3, "signed out or expired"),
        (302, "text/html", "", 3, "redirect"),
        (403, "text/html", "<html>Access Denied</html>", 5, "edge refused"),
        (429, JSON, "{}", 5, "too many requests"),
        (503, JSON, "{}", 5, "HTTP 503"),
    ];
    for (status, content_type, body, code, words) in cases {
        let h = Household::new();
        h.server.fault(Fault {
            experience: FAMILY,
            nth: None,
            status,
            content_type,
            body: body.into(),
        });
        let e = h.run(&["patients", "list"]).unwrap_err();
        assert_eq!(e.exit_code(), code, "{status}");
        assert!(e.message().contains(words), "{status}: {}", e.message());
    }
}

#[test]
fn a_failure_the_answer_reports_is_exit_5() {
    let h = Household::new();
    h.server.fault(Fault {
        experience: CLAIMS,
        nth: None,
        status: 200,
        content_type: JSON,
        body: r#"{"data":{"getClaimV2":{"statusCode":"9999","statusDescription":"Synthetic failure"}}}"#.into(),
    });
    let e = h.run(&["rx", "list", "--patient", "Alex"]).unwrap_err();
    assert_eq!(e.exit_code(), 5);
    assert!(e.message().contains("9999: Synthetic failure"), "{}", e.message());
}

#[test]
fn an_account_with_no_one_on_it_is_not_found() {
    let h = Household::new();
    h.server.fault(Fault {
        experience: FAMILY,
        nth: None,
        status: 200,
        content_type: JSON,
        body: r#"{"data":{"getFamilyInfo":{"statusCode":"0000","familyMemberList":[]}}}"#.into(),
    });
    assert_eq!(h.run(&["patients", "list"]).unwrap_err().exit_code(), 4);
}

#[test]
fn the_patient_is_picked_or_refused_before_the_fills_are_read() {
    let h = Household::new();
    let e = h.run(&["records", "export", "--ndjson"]).unwrap_err();
    assert_eq!(e.exit_code(), 2);
    assert!(e.message().contains("3 people"), "{}", e.message());
    let e = h
        .run(&["records", "export", "--patient", "nobody", "--ndjson"])
        .unwrap_err();
    assert_eq!(e.exit_code(), 4);
    assert!(h.reads(CLAIMS).is_empty());
}

#[test]
fn arguments_are_checked_before_the_site_is_read() {
    let h = Household::new();
    for args in [
        &["records", "export", "--patient", "Alex", "--ndjson", "--json"][..],
        &[
            "records",
            "export",
            "--patient",
            "Alex",
            "--since",
            "2026-13-01",
            "--ndjson",
        ][..],
        &["costs", "--since", "2026-02-01", "--until", "2026-01-01"][..],
        &["rx", "list", "--since", "yesterday"][..],
        // Every year to --since is read: a far-off one is refused.
        &[
            "records",
            "export",
            "--patient",
            "Alex",
            "--since",
            "1990-01-01",
            "--ndjson",
        ][..],
        &["costs", "--since", "1960-01-01"][..],
        &[
            "api",
            "POST",
            "/api/client/experience/v2/load/00000000-0000-0000-0000-000000000000",
        ][..],
    ] {
        let e = h.run(args).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{args:?}: {}", e.message());
    }
    assert!(h.server.seen().is_empty());
}

#[test]
fn api_prints_the_sites_answer() {
    let h = Household::new();
    let (code, out) = h
        .run(&["api", "POST", "member-family", "--data", r#"{"data":{}}"#])
        .unwrap();
    assert_eq!(code, 0);
    let doc: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["data"]["getFamilyInfo"]["familyMemberList"][0]["firstName"], "Alex");
}

#[test]
fn a_profile_without_a_session_names_the_sign_in() {
    let h = Household::new();
    h.env.sessions.delete(&adult1()).unwrap();
    let e = h.run(&["patients", "list"]).unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert!(h.server.seen().is_empty());
}
