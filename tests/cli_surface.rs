//! Black-box tests of the binary's surface. None reads the member site:
//! each renders help or info, or is refused by argument checks that run
//! before the site is read. The config directory is a temp dir, never the
//! developer's, and `CAREMARK_PROFILE` is cleared.
//!
//! None reaches the real keychain or browser profile:
//! `CAREMARK_KEYCHAIN_SERVICE` names a service that holds nothing (a missing
//! item needs no approval), and under it the binary never touches the kept
//! profile. `XDG_DATA_HOME`, where Linux keeps that profile, is a temp dir
//! besides. `HOME` stays: on macOS the keychain is found through it. No
//! test stores a session, so no test writes the vault's key or reads one:
//! the vault reads its key only when an entry exists (cli-common DESIGN.md
//! §2.5).
//!
//! The temp config of [`caremark`] configures one profile, `adult1`.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn tmp() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "caremark-cli-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The empty keychain service every test reads instead of `piekstra.caremark`.
const HERMETIC_SERVICE: &str = "piekstra.caremark.hermetic-tests";

fn hermetic(cmd: &mut Command) -> &mut Command {
    cmd.env("CAREMARK_KEYCHAIN_SERVICE", HERMETIC_SERVICE)
        .env("XDG_DATA_HOME", tmp())
        .env_remove("CAREMARK_CHROME")
}

/// A config directory whose config configures `profiles`.
fn config_with(profiles: &[&str]) -> std::path::PathBuf {
    let base = tmp();
    std::fs::create_dir_all(base.join("caremark")).unwrap();
    let names: serde_json::Map<String, Value> = profiles
        .iter()
        .map(|p| (p.to_string(), serde_json::json!({})))
        .collect();
    std::fs::write(
        base.join("caremark").join("config.json"),
        serde_json::json!({ "profiles": names }).to_string(),
    )
    .unwrap();
    base
}

fn caremark() -> Command {
    let mut cmd = Command::cargo_bin("caremark").unwrap();
    hermetic(&mut cmd)
        .env("XDG_CONFIG_HOME", config_with(&["adult1"]))
        .env_remove("CAREMARK_PROFILE")
        .env_remove("NO_COLOR");
    cmd
}

fn error_of(out: &assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&out.get_output().stdout).expect("one JSON error document on stdout")
}

#[test]
fn every_command_renders_help() {
    for args in [
        vec!["--help"],
        vec!["auth", "--help"],
        vec!["auth", "login", "--help"],
        vec!["auth", "status", "--help"],
        vec!["auth", "logout", "--help"],
        vec!["auth", "reset", "--help"],
        vec!["profiles", "--help"],
        vec!["profiles", "list", "--help"],
        vec!["profiles", "add", "--help"],
        vec!["profiles", "remove", "--help"],
        vec!["config", "--help"],
        vec!["config", "path", "--help"],
        vec!["config", "show", "--help"],
        vec!["patients", "--help"],
        vec!["patients", "list", "--help"],
        vec!["rx", "--help"],
        vec!["rx", "list", "--help"],
        vec!["costs", "--help"],
        vec!["records", "--help"],
        vec!["records", "export", "--help"],
        vec!["api", "--help"],
        vec!["self-update", "--help"],
        vec!["completions", "--help"],
        vec!["info", "--help"],
    ] {
        caremark().args(&args).assert().success();
    }
}

#[test]
fn help_says_unofficial_and_read_only() {
    caremark()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Unofficial read-only"))
        .stdout(predicate::str::contains(
            "Not affiliated with or endorsed by CVS Caremark",
        ));
}

#[test]
fn version_prints() {
    caremark()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("caremark "));
}

#[test]
fn info_declares_the_profile_and_the_browser_sign_in() {
    let out = caremark().arg("info").assert().success();
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "cli-info/v1");
    assert_eq!(v["name"], "caremark");
    assert_eq!(v["spec"], "piekstra-cli/1");
    assert_eq!(v["profiles"], serde_json::json!(["health-records/v1"]));
    // medrec passes --profile to a provider listing login-profiles among its
    // capabilities (medical-common DESIGN.md §1.1).
    let capabilities = v["capabilities"].as_array().unwrap();
    assert!(capabilities.contains(&serde_json::json!("login-profiles")), "{v}");
    for c in ["api", "patients", "rx", "costs", "records"] {
        assert!(capabilities.contains(&serde_json::json!(c)), "{c}: {v}");
    }
    assert!(!capabilities.contains(&serde_json::json!("records-files")), "{v}");
    assert_eq!(v["auth"]["required"], true);
    assert_eq!(v["auth"]["method"], "browser-session");
    assert_eq!(
        v["auth"]["login_hint"],
        "caremark auth login --browser --profile <NAME>"
    );
}

#[test]
fn auth_status_when_nothing_is_stored() {
    let out = caremark()
        .args(["auth", "status", "--profile", "adult1", "--json"])
        .assert()
        .success();
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "auth-status/v1");
    assert_eq!(v["required"], true);
    assert_eq!(v["authenticated"], false);
    assert_eq!(v["method"], "browser-session");
    assert_eq!(v["credential_in_keychain"], false);
    assert_eq!(v["account"], "adult1");
    assert!(v.get("captured_at").is_none(), "{v}");
    // Under the test service the real profile is not reported.
    assert!(v.get("browser_profile").is_none(), "{v}");
    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(stderr.contains("CAREMARK_KEYCHAIN_SERVICE is set"), "{stderr}");
    caremark()
        .env("CAREMARK_PROFILE", "adult1")
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Method:        browser-session"))
        .stdout(predicate::str::contains("Keychain:      empty"));
}

/// A profile must be configured: an unknown one is exit 2 naming the
/// configured ones, none configured names `profiles add`, and a lone
/// profile is never picked for a command that names none.
#[test]
fn a_profile_must_be_configured() {
    let out = caremark()
        .args(["auth", "status", "--profile", "adult2", "--json"])
        .assert()
        .code(2);
    let e = error_of(&out);
    assert!(e["error"]["message"].as_str().unwrap().contains("adult1"), "{e}");
    let out = caremark().args(["auth", "status", "--json"]).assert().code(2);
    assert!(error_of(&out)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("configured: adult1"));
    let out = caremark()
        .env("XDG_CONFIG_HOME", tmp())
        .args(["auth", "status", "--profile", "adult1", "--json"])
        .assert()
        .code(2);
    let e = error_of(&out);
    assert!(e.to_string().contains("caremark profiles add adult1"), "{e}");
}

#[test]
fn profiles_add_list_remove() {
    let cfg = tmp();
    let run = |args: &[&str]| {
        let mut c = caremark();
        c.env("XDG_CONFIG_HOME", &cfg).args(args);
        c
    };
    let list =
        |c: &mut Command| -> Value { serde_json::from_slice(&c.assert().success().get_output().stdout).unwrap() };
    assert_eq!(
        list(&mut run(&["profiles", "list", "--json"])),
        serde_json::json!({ "schema": "profile-list/v1", "profiles": [] })
    );
    for name in ["adult2", "adult1"] {
        let v = list(&mut run(&["profiles", "add", name, "--json"]));
        assert_eq!(
            v,
            serde_json::json!({ "schema": "profile-add/v1", "profile": name, "added": true })
        );
    }
    let v = list(&mut run(&["profiles", "add", "adult1", "--json"]));
    assert_eq!(v["added"], false);
    let v = list(&mut run(&["profiles", "list", "--json"]));
    assert_eq!(
        v["profiles"],
        serde_json::json!([{ "name": "adult1" }, { "name": "adult2" }])
    );
    run(&["profiles", "list"]).assert().success().stdout("adult1\nadult2\n");
    for bad in ["Adult1", "a b", "-x"] {
        run(&["profiles", "add", bad]).assert().code(2);
    }
    // Destructive: without a terminal it needs --force, decided first.
    run(&["profiles", "remove", "adult2"]).assert().code(6);
    run(&["profiles", "remove", "adult3", "--force"]).assert().code(4);
    let v = list(&mut run(&["profiles", "remove", "adult2", "--force", "--json"]));
    assert_eq!(v["schema"], "profile-remove/v1");
    assert_eq!(v["profile"], "adult2");
    assert_eq!(v["session_removed"], false);
    // The kept browser profile is left alone under the test service.
    assert!(v.get("browser_profile_removed").is_none(), "{v}");
    let v = list(&mut run(&["profiles", "list", "--json"]));
    assert_eq!(v["profiles"], serde_json::json!([{ "name": "adult1" }]));
    run(&["--profile", "adult1", "profiles", "list"]).assert().code(2);
}

/// `auth reset` clears every profile's session: without a terminal it
/// needs --force (exit 6), before the vault is opened, and it is no one
/// profile's.
#[test]
fn auth_reset_needs_force_without_a_terminal() {
    let out = caremark().args(["auth", "reset", "--json"]).assert().code(6);
    assert_eq!(error_of(&out)["error"]["code"], "confirmation_required");
    caremark()
        .args(["auth", "reset", "--force", "--profile", "adult1"])
        .assert()
        .code(2);
}

#[test]
fn auth_logout_when_nothing_is_stored() {
    let out = caremark()
        .args(["--profile", "adult1", "auth", "logout", "--json"])
        .assert()
        .success();
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "auth-logout/v1");
    assert_eq!(v["session_removed"], false);
    // The kept profile is left alone under the test service.
    assert!(v.get("browser_profile_removed").is_none(), "{v}");
    caremark()
        .args(["auth", "logout", "--profile", "adult1"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no session was stored"))
        .stderr(predicate::str::contains("left alone"));
}

/// The usage rules of DESIGN.md §2.2: one credential source, decided before
/// any keychain or network work.
#[test]
fn auth_login_takes_exactly_one_source() {
    for args in [
        vec!["--browser", "--stdin"],
        vec!["--browser", "--from-env", "CAREMARK_TEST_COOKIE"],
        vec!["--stdin", "--from-env", "CAREMARK_TEST_COOKIE"],
        // --browser always checks the session.
        vec!["--browser", "--no-verify"],
        // No source at all.
        vec![],
    ] {
        let out = caremark()
            .args(["auth", "login", "--profile", "adult1", "--json"])
            .args(&args)
            .assert()
            .code(2);
        assert_eq!(error_of(&out)["error"]["code"], "usage", "{args:?}");
    }
    for args in [
        vec!["--browser", "--op", "op://Example/Login/cookie"],
        vec!["--fresh-profile"],
        vec!["--fresh-profile", "--stdin"],
        vec!["--browser", "--wait", "0"],
        vec!["--browser", "--wait", "121"],
        vec!["--browser", "--log-requests", "901"],
        vec!["--stdin", "--user-agent", "Mozilla/5.0 (test)", "--log-requests", "5"],
    ] {
        caremark()
            .args(["auth", "login", "--profile", "adult1"])
            .args(&args)
            .assert()
            .code(2);
    }
}

/// The sign-in is one profile's: none named or a malformed name is exit 2,
/// before the keychain is read.
#[test]
fn auth_commands_take_one_well_formed_profile() {
    let too_long = "a".repeat(33);
    for args in [
        vec!["auth", "status"],
        vec!["auth", "logout"],
        vec!["auth", "login", "--stdin"],
        vec!["auth", "status", "--profile", "Adult1"],
        vec!["auth", "status", "--profile=-x"],
        vec!["auth", "status", "--profile", "a.b"],
        vec!["auth", "status", "--profile", &too_long],
    ] {
        let out = caremark().args(&args).arg("--json").assert().code(2);
        assert_eq!(error_of(&out)["error"]["code"], "usage", "{args:?}");
    }
    caremark()
        .env("CAREMARK_PROFILE", "Not Valid")
        .args(["auth", "status"])
        .assert()
        .code(2);
}

/// A `--stdin` import checks what it read before anything is stored or
/// sent. Nothing it reads here is a header, so nothing is stored.
#[test]
fn auth_login_stdin_refuses_what_is_not_a_cookie_header() {
    for input in ["", "\n", "not a cookie header", "dummy_a=1\ndummy_b=2"] {
        let out = caremark()
            .args([
                "auth",
                "login",
                "--profile",
                "adult1",
                "--stdin",
                "--user-agent",
                "Mozilla/5.0 (test)",
                "--no-verify",
                "--json",
            ])
            .write_stdin(input)
            .assert()
            .code(2);
        let e = error_of(&out);
        assert_eq!(e["error"]["code"], "usage", "{input:?}");
        assert!(!e.to_string().contains("dummy"), "{e}");
    }
}

/// `--user-agent` names the browser a pasted Cookie header came from, so
/// it goes with --stdin or --from-env, and they need it.
#[test]
fn auth_login_user_agent_goes_with_a_pasted_session() {
    for args in [
        vec!["--browser", "--user-agent", "Mozilla/5.0 (test)"],
        vec!["--user-agent", "Mozilla/5.0 (test)"],
        vec!["--stdin"],
        vec!["--from-env", "CAREMARK_TEST_COOKIE"],
    ] {
        caremark()
            .args(["auth", "login", "--profile", "adult1"])
            .args(&args)
            .assert()
            .code(2);
    }
}

/// `api` reads: another method, a path off the site, or a POST to anything
/// but the session check and the listed experiences is a usage error,
/// before a profile or the vault is read.
#[test]
fn api_requests_are_checked_first() {
    for m in ["PUT", "DELETE", "PATCH", "HEAD"] {
        caremark()
            .args(["--profile", "adult1", "api", m, "/x"])
            .assert()
            .code(2);
    }
    for args in [
        vec!["api", "GET", "https://example.com/x"],
        vec!["api", "GET", "relative"],
        vec!["api", "GET", "/x", "--data", "{}"],
        vec!["api", "POST", "/api/oauth2/v2/token", "--data", "{}"],
        vec!["api", "POST", "/api/client/experience/v2/load/unlisted", "--data", "{}"],
    ] {
        // No profile is configured here: a usage error from the request
        // comes before the profile's.
        let out = caremark()
            .env("XDG_CONFIG_HOME", tmp())
            .args(&args)
            .arg("--json")
            .assert()
            .code(2);
        let e = error_of(&out);
        assert!(!e.to_string().contains("profiles add"), "{args:?}: {e}");
    }
    let help = caremark().args(["api", "--help"]).assert().success();
    let text = String::from_utf8_lossy(&help.get_output().stdout).to_string();
    assert!(text.contains("[possible values: GET, POST]"), "{text}");
    // Without a stored session, a read is exit 3 naming the sign-in.
    let out = caremark()
        .args(["api", "GET", "/x", "--profile", "adult1", "--json"])
        .assert()
        .code(3);
    assert!(error_of(&out)
        .to_string()
        .contains("auth login --browser --profile adult1"));
}

/// The record commands check their arguments before the profile, the vault
/// or the site; then they need a configured profile, then a stored session.
#[test]
fn record_commands_check_their_arguments_first() {
    for args in [
        // --json is added below: with --ndjson it is a usage error.
        vec!["records", "export", "--ndjson"],
        vec!["records", "export", "--since", "2026-02-30", "--ndjson"],
        vec!["costs", "--since", "2026-02-02", "--until", "2026-02-01"],
        vec!["rx", "list", "--until", "soon"],
    ] {
        let out = caremark()
            .env("XDG_CONFIG_HOME", tmp())
            .args(&args)
            .arg("--json")
            .assert()
            .code(2);
        let e = error_of(&out);
        assert!(!e.to_string().contains("profiles add"), "{args:?}: {e}");
    }
    // clap's own refusals: no destination, and two.
    caremark()
        .env("XDG_CONFIG_HOME", tmp())
        .args(["records", "export"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--ndjson"));
    caremark()
        .args(["records", "export", "--ndjson", "-o", "/tmp/x"])
        .assert()
        .code(2);
    for args in [
        vec!["patients", "list"],
        vec!["rx", "list"],
        vec!["costs"],
        vec!["records", "export", "-o", "target/never-written"],
    ] {
        // No profile: exit 2 naming `profiles add`.
        let out = caremark()
            .env("XDG_CONFIG_HOME", tmp())
            .args(&args)
            .arg("--json")
            .assert()
            .code(2);
        assert!(error_of(&out).to_string().contains("profiles add"), "{args:?}");
        // A profile with no session: exit 3 naming the sign-in.
        let out = caremark()
            .args(&args)
            .args(["--profile", "adult1", "--json"])
            .assert()
            .code(3);
        assert!(
            error_of(&out)
                .to_string()
                .contains("auth login --browser --profile adult1"),
            "{args:?}"
        );
    }
}

/// The commands that need no sign-in take no --profile; CAREMARK_PROFILE in
/// the environment does not trouble them.
#[test]
fn commands_without_a_sign_in_take_no_profile() {
    for args in [
        vec!["info"],
        vec!["config", "path"],
        vec!["config", "show"],
        vec!["profiles", "list"],
        vec!["completions", "bash"],
        vec!["self-update", "--check"],
    ] {
        caremark().args(&args).args(["--profile", "adult1"]).assert().code(2);
    }
    caremark()
        .env("CAREMARK_PROFILE", "adult1")
        .arg("info")
        .assert()
        .success();
}

#[test]
fn config_path_and_show() {
    let cfg = tmp();
    let run = |args: &[&str]| {
        let mut c = caremark();
        c.env("XDG_CONFIG_HOME", &cfg).args(args);
        c
    };
    let out = run(&["config", "path", "--json"]).assert().success();
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "config-path/v1");
    assert!(v["path"].as_str().unwrap().ends_with("caremark/config.json"), "{v}");
    run(&["config", "show"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no settings"));
    run(&["profiles", "add", "adult1"]).assert().success();
    let out = run(&["config", "show", "--json"]).assert().success();
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "config/v1");
    assert!(v["profiles"].get("adult1").is_some(), "{v}");
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    caremark().arg("frobnicate").assert().code(2);
    caremark().args(["profiles", "delete"]).assert().code(2);
}

#[test]
fn no_color_is_accepted_as_any_value() {
    caremark()
        .env("NO_COLOR", "1")
        .args(["auth", "status", "--profile", "adult1"])
        .assert()
        .success();
    caremark().args(["--no-color", "info"]).assert().success();
}

#[test]
fn completions_render() {
    for shell in ["bash", "zsh", "fish"] {
        caremark()
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("caremark"));
    }
}

#[test]
fn there_is_no_write_command() {
    let out = caremark().arg("--help").assert().success();
    let help = String::from_utf8(out.get_output().stdout.clone())
        .unwrap()
        .to_lowercase();
    for verb in ["refill", "order", "submit", "pay ", "upload", "transfer", "mcp"] {
        assert!(!help.contains(verb), "help mentions {verb}");
    }
}
