//! The repository carries no personal data; runtime output may.
//!
//! Walks every tracked file (`git ls-files`) for the *shapes* personal data
//! takes (phone numbers, long id numbers, street addresses, email
//! addresses) rather than a list of real values, which would publish what
//! it exists to keep out. Then checks the fixtures positively: every
//! identity-bearing field a member-site answer carries holds one of the
//! dummy shapes `tests/fixtures/README.md` lists.

use std::process::Command;

fn root() -> &'static str {
    env!("CARGO_MANIFEST_DIR")
}

fn tracked_files() -> Vec<String> {
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(root())
        .output()
        .expect("git ls-files");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn digits_run(s: &str, min: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            cur.push(c);
        } else {
            if cur.len() >= min {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    out
}

/// A digit run is a dummy when it is all one digit, zeros then one digit
/// (`00000001`), or a 555 phone number: `555…`, or any area code with the
/// 555-0100 to 555-0199 lines reserved for fiction.
fn is_dummy_number(n: &str) -> bool {
    let first = n.chars().next().unwrap();
    let head = &n[..n.len() - 1];
    let local = n.strip_prefix('1').filter(|r| r.len() == 10).unwrap_or(n);
    let fictional = local.len() == 10 && &local[3..8] == "55501";
    n.chars().all(|c| c == first)
        || head.chars().all(|c| c == '0')
        || n.starts_with("555")
        || n.starts_with("1555")
        || fictional
}

const PUBLIC_EMAIL_DOMAINS: &[&str] = &["example.com", "caremark.com", "cvs.com", "cvshealth.com", "aetna.com"];

/// `line` with every UUID (8-4-4-4-12 lowercase hex) blanked: the site's
/// experience ids are constants of its public pages, and their digit runs
/// are not phone numbers.
fn without_uuids(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = line.to_string().into_bytes();
    let is_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    let mut i = 0;
    while i + 36 <= bytes.len() {
        let w = &bytes[i..i + 36];
        let uuid = w.iter().enumerate().all(|(j, b)| match j {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => is_hex(*b),
        });
        let bounded = (i == 0 || !is_hex(bytes[i - 1])) && (i + 36 == bytes.len() || !is_hex(bytes[i + 36]));
        if uuid && bounded {
            out[i..i + 36].fill(b' ');
            i += 36;
        } else {
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// `line` with each lowercase hex digest (a run of 32 or more hex digits
/// holding a letter, such as a pinned SHA-256) blanked: its digit runs are
/// not numbers.
fn without_digests(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        let digest = run.len() >= 32 && run.bytes().any(|b| b.is_ascii_lowercase());
        out.push_str(if digest { "<digest>" } else { run.as_str() });
        run.clear();
    };
    for c in line.chars() {
        if c.is_ascii_digit() || ('a'..='f').contains(&c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// The personal-data shapes on one line, each as `kind value`.
fn scan_line(line: &str) -> Vec<String> {
    let mut findings = Vec::new();
    let line = &without_digests(&without_uuids(line));
    let phoneish = line.replace(['(', ')', '-', '.', ' '], "");
    for n in digits_run(line, 7)
        .into_iter()
        .chain(digits_run(&phoneish, 10).into_iter().filter(|n| n.len() <= 11))
    {
        if !is_dummy_number(&n) {
            findings.push(format!("number {n}"));
        }
    }
    for word in line.split(|c: char| c.is_whitespace() || "\"'<>(),;`".contains(c)) {
        if let Some((local, domain)) = word.split_once('@') {
            let domain = domain.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
            let ok = local.is_empty()
                || !domain.contains('.')
                || PUBLIC_EMAIL_DOMAINS.contains(&domain)
                || domain.ends_with(".example")
                || domain.ends_with("users.noreply.github.com");
            if !ok {
                findings.push(format!("email {word}"));
            }
        }
    }
    let lower = line.to_ascii_lowercase();
    for suffix in [" way", " st", " ave", " blvd", " dr", " ln", " ct", " rd"] {
        if let Some(pos) = lower.find(suffix) {
            let has_number = lower[..pos]
                .split_whitespace()
                .rev()
                .take(4)
                .any(|w| w.len() >= 3 && w.chars().all(|c| c.is_ascii_digit()));
            if has_number && !lower.contains("example st") {
                findings.push(format!("address-like `{}`", line.trim()));
            }
        }
    }
    findings
}

/// `<commit>:<path>:<rule>:<line>`: a 40-character hex commit, a path with
/// no space, a lowercase rule id and a line number.
fn is_fingerprint(line: &str) -> bool {
    let parts: Vec<&str> = line.split(':').collect();
    let [commit, path, rule, number] = parts.as_slice() else {
        return false;
    };
    commit.len() == 40
        && commit.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && !path.is_empty()
        && !path.contains(char::is_whitespace)
        && !rule.is_empty()
        && rule.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
}

#[test]
fn a_fingerprint_is_only_a_fingerprint() {
    assert!(is_fingerprint(
        "0000000000000000000000000000000000000001:src/site.rs:generic-api-key:12"
    ));
    assert!(!is_fingerprint(
        "0000000000000000000000000000000000000001:src/site.rs:generic-api-key:12 note"
    ));
    assert!(!is_fingerprint("call 5551234567"));
    assert!(!is_fingerprint("0000000:src/site.rs:generic-api-key:12"));
}

#[test]
fn tracked_files_carry_no_personal_data() {
    let files = tracked_files();
    if files.is_empty() {
        // Not a git checkout (a packaged crate): nothing to scan.
        return;
    }
    let mut findings = Vec::new();
    for file in files {
        if file.ends_with(".lock") || file.starts_with("LICENSE") || file == "tests/no_personal_data.rs" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(format!("{}/{file}", root())) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            // A gitleaks fingerprint's commit hash can hold a phone-length
            // digit run. Such a line is checked for that shape instead, so
            // nothing else can ride along in the file uncommented.
            if file == ".gitleaksignore" && !line.starts_with('#') {
                if !is_fingerprint(line) {
                    findings.push(format!("{file}:{}: not a gitleaks fingerprint", i + 1));
                }
                continue;
            }
            for f in scan_line(line) {
                findings.push(format!("{file}:{}: {f}", i + 1));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "personal-data shapes in tracked files:\n{}",
        findings.join("\n")
    );
}

#[test]
fn no_tracked_file_is_a_raw_capture_or_a_statement() {
    for file in tracked_files() {
        let lower = file.to_ascii_lowercase();
        for ext in [".pdf", ".har", ".csv", ".xlsx", ".raw.json", ".raw.html", ".ndjson"] {
            assert!(
                !lower.ends_with(ext),
                "{file} is tracked; real statements and captures never are"
            );
        }
    }
}

const PEOPLE: &[&str] = &["ALEX", "SAM", "JORDAN"];

fn is_dummy_name(full: &str) -> bool {
    match full
        .split_whitespace()
        .map(str::to_ascii_uppercase)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [first, last] => PEOPLE.contains(&first.as_str()) && last == "EXAMPLE",
        [first] => PEOPLE.contains(&first.as_str()),
        _ => false,
    }
}

fn is_dummy_org(name: &str) -> bool {
    ["EXAMPLE", "SAMPLE", "TEST"]
        .iter()
        .any(|w| name.to_ascii_uppercase().split_whitespace().any(|x| x == *w))
}

/// `prefix` then exactly `digits` digits.
fn shaped(value: &str, prefix: &str, digits: usize) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|d| d.len() == digits && d.chars().all(|c| c.is_ascii_digit()))
}

/// The identity-bearing keys of the member site's answers and the dummy
/// each must hold (tests/fixtures/README.md). `None` for a key that is not
/// one.
fn dummy_for(key: &str, s: &str) -> Option<bool> {
    let all_digits_dummy = |s: &str| s.bytes().all(|b| b.is_ascii_digit()) && is_dummy_number(s);
    let lower = key.to_ascii_lowercase();
    Some(match lower.as_str() {
        "firstname" | "first_name" | "givenname" | "prescriberfirstname" | "patientfirstname" => {
            PEOPLE.contains(&s.to_ascii_uppercase().as_str())
        }
        "lastname" | "last_name" | "familyname" | "prescriberlastname" | "patientlastname" => {
            s.eq_ignore_ascii_case("example")
        }
        "membername" | "patientname" | "fullname" => is_dummy_name(s),
        "dob" | "dateofbirth" | "birthdate" => s.ends_with("-01-01") || s.starts_with("01/01/"),
        "memberid" | "subscriberid" | "cardholderid" | "personid" | "membershipid" => shaped(s, "SYNM", 6),
        "groupnumber" | "groupid" | "rxgroup" => shaped(s, "SYNG", 4),
        "rxnumber"
        | "prescriptionnumber"
        | "claimnumber"
        | "claimid"
        | "rxclaimnumber"
        | "id"
        | "lookupid"
        | "externalid"
        | "familymemberinternalid" => all_digits_dummy(s) || s.starts_with("SYN"),
        "prescribername" | "doctorname" | "pharmacyname" | "groupname" => is_dummy_org(s) || is_dummy_name(s),
        "npi" | "prescribernpi" | "pharmacynpi" | "ncpdp" | "pharmacyid" | "nationalproviderid" | "pharmacynumber"
        | "pharmacynpiid" => all_digits_dummy(s),
        "phone" | "phonenumber" | "pharmacyphone" | "prescriberphone" => {
            all_digits_dummy(&s.replace(['(', ')', '-', ' ', '.'], ""))
        }
        "email" | "emailaddress" => s.ends_with("@example.com"),
        _ => return None,
    })
}

/// Every identity field the member-site fixtures carry, by key, wherever it
/// is nested.
#[test]
fn identity_fields_in_site_fixtures_use_dummy_values() {
    fn walk(name: &str, key: &str, v: &serde_json::Value, checked: &mut usize) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    walk(name, k, v, checked);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(name, key, v, checked)),
            serde_json::Value::String(s) if !s.is_empty() => {
                if let Some(ok) = dummy_for(key, s) {
                    assert!(ok, "{name}: `{key}` holds `{s}`, not a dummy");
                    *checked += 1;
                }
            }
            _ => {}
        }
    }
    let dir = format!("{}/tests/fixtures/site", root());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // No site fixtures yet.
        return;
    };
    let mut checked = 0;
    for entry in entries {
        let path = entry.unwrap().path();
        let name = path.display().to_string();
        if !name.ends_with(".json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        walk(&name, "", &v, &mut checked);
    }
    assert!(checked > 0, "no identity fields found; did the fixtures change?");
}

#[test]
fn the_scanner_flags_real_shapes_and_passes_dummies() {
    for real in [
        "call (772) 404-1234",
        "call 888-555-0200",
        "member id 9234567812",
        "mail someone@gmail.com",
        "lives at 1234 NW Oak St, Somewhere",
        "claim 7342918",
        "id 13ed331c-3e9c-4fdf-bab2-6396a12a3623 then 7342918",
        "bad1234567 is not a digest",
    ] {
        assert!(!scan_line(real).is_empty(), "missed: {real}");
    }
    for dummy in [
        "call (555) 010-0001",
        "customer service 888-555-0100",
        "SYNM000001 SYNC00000003",
        "000000000000000-A-S-PS-E",
        "pat@example.com",
        "100 Example St, Exampleton",
        "$16,000.00",
        "experience 656e55d9-691d-4b48-b5fa-cefa54284336",
        "SHA256: 551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb",
    ] {
        assert!(
            scan_line(dummy).is_empty(),
            "false positive: {dummy}: {:?}",
            scan_line(dummy)
        );
    }
    assert!(is_dummy_name("ALEX EXAMPLE"));
    assert!(is_dummy_name("Sam Example"));
    assert!(!is_dummy_name("ALEX SMITH"));
    assert!(shaped("SYNM000001", "SYNM", 6));
    assert!(!shaped("SYNM00001", "SYNM", 6));
    assert_eq!(dummy_for("firstName", "Alex"), Some(true));
    assert_eq!(dummy_for("firstName", "Robin"), Some(false));
    assert_eq!(dummy_for("rxNumber", "0000001"), Some(true));
    assert_eq!(dummy_for("rxNumber", "4827361"), Some(false));
    assert_eq!(dummy_for("drugName", "EXAMPLEPRIL 10 MG TABLET"), None);
}
