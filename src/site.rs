//! The member site, www.caremark.com: what `auth login --browser` reads out
//! of the Chrome window, the public keys the site's pages send with their
//! API calls, and the one read that confirms a session before it is stored.
//!
//! The site sets visitor cookies before anyone signs in, and no cookie is
//! known to appear only once a sign-in completes, so a cookie in the jar
//! proves nothing. The capture hands each candidate jar to
//! [`confirm_candidate`], which asks the site's own session check
//! ([`SESSION_CHECK_URL`], the call the site's pages make to learn whether
//! they are signed in) at most once every [`CHECK_EVERY`], so a sign-in that
//! takes minutes costs a few requests, not one per poll.
//!
//! The capture reads cookies and the user agent at the browser level, which
//! no page can observe, and evaluates nothing in the site's pages: the
//! sign-in page runs a bot check (docs/api.md).

use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pk_cli_browser::{BrowserError, Capture, Captured, Confirm, HostRule};
use pk_cli_core::CliError;
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, COOKIE};

use crate::cli::BIN;

/// The provider's name in the browser crate's status lines and errors.
pub const LABEL: &str = "CVS Caremark";
/// The host whose pages the capture reads, and every read's.
pub const HOST: &str = "www.caremark.com";
/// The domain whose whole cookie jar is the session.
pub const DOMAIN: &str = "caremark.com";
/// The member site's origin.
pub const ORIGIN: &str = "https://www.caremark.com";
/// Where the Chrome window opens. A signed-out visitor is sent on to the
/// sign-in from here.
pub const START_URL: &str = "https://www.caremark.com/pharmacy/home/homepage";
/// What signing in involves, for the opening status line.
pub const HINT: &str = "your caremark.com password and any code it sends";

/// The site's public configuration script: the API keys its pages send
/// ([`SiteKeys`]). Served to anyone, signed in or not.
pub const ENV_JS_URL: &str = "https://www.caremark.com/memberportal/env.js";

/// The site's session check: `POST` with the session's cookies and the
/// site's public introspection key as HTTP Basic credentials. A signed-in
/// member gets `{"statusCode": "0000", "active": true, ...}`.
pub const SESSION_CHECK_URL: &str = "https://www.caremark.com/api/oauth2/v2/introspect";

/// The least time between two session checks while the owner signs in.
pub const CHECK_EVERY: Duration = Duration::from_secs(15);

/// The bound on a whole read, connecting included. The browser crate's poll
/// (and Ctrl-C) waits for it.
const TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The most of an answer's body read for [`judge`], and of env.js.
const MAX_BODY: u64 = 1024 * 1024;

/// The keys the site's own pages send with their API calls, read from
/// [`ENV_JS_URL`]. They are the site's public client keys, the same for
/// every visitor, not the member's credentials; still, nothing prints them.
#[derive(Clone)]
pub struct SiteKeys {
    /// `experienceApiKey`: the `x-api-key` of the experience API.
    pub experience: String,
    /// `introspectApiKey`: the session check's HTTP Basic credentials,
    /// already base64.
    pub introspect: String,
}

impl std::fmt::Debug for SiteKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SiteKeys(..)")
    }
}

impl SiteKeys {
    /// The keys `env.js` assigns: `name: 'value'` (or double quotes) inside
    /// `window.apiConfig = {...}`. `None` when either is missing or empty.
    pub fn parse(env_js: &str) -> Option<SiteKeys> {
        Some(SiteKeys {
            experience: assigned(env_js, "experienceApiKey")?,
            introspect: assigned(env_js, "introspectApiKey")?,
        })
    }

    /// The keys, read from the site: exit 5 when env.js does not answer or
    /// no longer assigns them.
    pub fn fetch(client: &reqwest::blocking::Client) -> Result<SiteKeys, CliError> {
        let what = format!("GET {}", path_of(ENV_JS_URL));
        let answer = client
            .get(ENV_JS_URL)
            .send()
            .map_err(|e| CliError::Upstream(format!("{what} got no answer: {}", e.without_url())))?;
        let status = answer.status().as_u16();
        if status != 200 {
            return Err(CliError::Upstream(format!("{what} answered HTTP {status}")));
        }
        let mut body = Vec::new();
        answer
            .take(MAX_BODY)
            .read_to_end(&mut body)
            .map_err(|_| CliError::Upstream(format!("{what}: the answer did not arrive whole")))?;
        SiteKeys::parse(&String::from_utf8_lossy(&body)).ok_or_else(|| {
            CliError::Upstream(format!(
                "{what} no longer assigns experienceApiKey and introspectApiKey; the site may have changed \
                 (docs/api.md)"
            ))
        })
    }
}

/// The string `name` is assigned in a JavaScript object literal:
/// `name: 'value'` or `name: "value"`. Empty values count as missing.
fn assigned(js: &str, name: &str) -> Option<String> {
    let mut rest = js;
    while let Some(at) = rest.find(name) {
        let after = &rest[at + name.len()..];
        let before = rest[..at].chars().next_back();
        let before_ok = before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        // A quoted name (`"experienceApiKey": ...`) closes its quote first.
        let key_end = match before {
            Some(q @ ('"' | '\'')) => after.strip_prefix(q).unwrap_or(after),
            _ => after,
        };
        let value = key_end
            .trim_start()
            .strip_prefix(':')
            .map(str::trim_start)
            .and_then(|v| {
                let quote = v.chars().next().filter(|c| *c == '\'' || *c == '"')?;
                let inner = &v[1..];
                inner.find(quote).map(|end| &inner[..end])
            });
        match value {
            Some(v) if before_ok && !v.is_empty() => return Some(v.to_string()),
            _ => rest = after,
        }
    }
    None
}

/// What `auth login --browser` reads: the whole [`DOMAIN`] jar and the
/// browser's user agent, handed over once [`confirm_candidate`] says the
/// jar is a signed-in one. With `log`, it also logs the endpoints the
/// site's pages call, and holds the window that long once confirmed.
pub fn capture(log: Option<Duration>) -> Result<Capture, BrowserError> {
    let checker = Checker::new();
    let mut capture = Capture::new(LABEL, HostRule::exact(HOST)?)
        .cookie_jar(DOMAIN)
        .with_user_agent()
        .hint(HINT)
        .confirm(move |candidate| checker.confirm(candidate));
    if let Some(during) = log {
        capture = capture.request_log(DOMAIN, during);
    }
    Ok(capture)
}

/// The capture's confirm hook, with its memory: when it last asked the site,
/// and the keys once read.
struct Checker {
    last: Mutex<Option<Instant>>,
    keys: Mutex<Option<SiteKeys>>,
}

impl Checker {
    fn new() -> Checker {
        Checker {
            last: Mutex::new(None),
            keys: Mutex::new(None),
        }
    }

    fn confirm(&self, candidate: &Captured) -> Confirm {
        let Some(header) = candidate.cookie_header() else {
            return Confirm::NotYet("the browser holds no caremark.com cookie yet".into());
        };
        {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|at| at.elapsed() < CHECK_EVERY) {
                return Confirm::NotYet("waiting for the sign-in to finish".into());
            }
            *last = Some(Instant::now());
        }
        let client = match client(candidate.user_agent()) {
            Ok(client) => client,
            Err(e) => return Confirm::Fail(e),
        };
        let cached = self.keys.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let keys = match cached {
            Some(keys) => keys,
            None => match SiteKeys::fetch(&client) {
                Ok(keys) => {
                    *self.keys.lock().unwrap_or_else(|e| e.into_inner()) = Some(keys.clone());
                    keys
                }
                Err(e) => return Confirm::undecided(e.message().to_string()),
            },
        };
        check_at(&client, SESSION_CHECK_URL, &keys, header.expose())
    }
}

/// The capture's confirm hook without its pacing: one session check of the
/// candidate's jar.
pub fn confirm_candidate(candidate: &Captured) -> Confirm {
    match candidate.cookie_header() {
        Some(header) => confirm_session(header.expose(), candidate.user_agent()),
        None => Confirm::NotYet("the browser holds no caremark.com cookie yet".into()),
    }
}

/// One session check with `cookie_header`: the keys from env.js, then
/// [`SESSION_CHECK_URL`], judged by [`judge`]. `user_agent` is the
/// browser's, for a session captured from one.
pub fn confirm_session(cookie_header: &str, user_agent: Option<&str>) -> Confirm {
    let client = match client(user_agent) {
        Ok(client) => client,
        Err(e) => return Confirm::Fail(e),
    };
    match SiteKeys::fetch(&client) {
        Ok(keys) => check_at(&client, SESSION_CHECK_URL, &keys, cookie_header),
        Err(e) => Confirm::undecided(e.message().to_string()),
    }
}

/// How the session check's answer shows a signed-in member.
///
/// `Ready` for a JSON answer with `statusCode` `"0000"` and `active` true.
/// `NotYet` for any other JSON answer (a visitor's) and for a 4xx. A 2xx
/// that is not JSON is `Undecided`, as is a status outside 2xx and 4xx. It
/// sees only answers [`check_at`] has not already decided: no 429, 5xx, 403
/// with an HTML page or redirect. `body` is at most 1 MiB.
pub fn judge(status: u16, content_type: Option<&str>, body: &[u8]) -> Confirm {
    let what = format!("POST {}", path_of(SESSION_CHECK_URL));
    if (400..500).contains(&status) {
        return Confirm::NotYet(format!("{what} answered HTTP {status}: not signed in"));
    }
    if !(200..300).contains(&status) {
        return Confirm::undecided(format!("{what} answered HTTP {status}"));
    }
    if !content_type.is_some_and(|t| t.contains("json")) {
        return Confirm::undecided(format!("{what} answered HTTP {status} without JSON"));
    }
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(doc) if is_signed_in(&doc) => Confirm::Ready,
        Ok(_) => Confirm::NotYet(format!("{what} says the session is not signed in")),
        Err(_) => Confirm::undecided(format!("{what} answered HTTP {status} with JSON that does not parse")),
    }
}

/// The session check's signed-in answer.
pub fn is_signed_in(doc: &serde_json::Value) -> bool {
    doc.get("statusCode").and_then(|c| c.as_str()) == Some("0000")
        && doc.get("active").and_then(|a| a.as_bool()) == Some(true)
}

/// The session check at `url`, apart from the constant so the tests can
/// point it at a local server.
fn check_at(client: &reqwest::blocking::Client, url: &str, keys: &SiteKeys, cookie_header: &str) -> Confirm {
    let what = format!("POST {}", path_of(url));
    let Ok(mut cookie) = HeaderValue::from_str(cookie_header) else {
        return Confirm::Fail(CliError::Other(
            "the session's Cookie header is not a valid HTTP header value".into(),
        ));
    };
    cookie.set_sensitive(true);
    let Ok(mut basic) = HeaderValue::from_str(&format!("Basic {}", keys.introspect)) else {
        return Confirm::undecided("the site's introspection key is not a valid header value");
    };
    basic.set_sensitive(true);
    let answer = match client
        .post(url)
        .header(COOKIE, cookie)
        .header(AUTHORIZATION, basic)
        .header(ACCEPT, "application/json")
        .send()
    {
        Ok(answer) => answer,
        Err(e) if e.is_timeout() => return Confirm::undecided(format!("{what} timed out")),
        Err(_) => return Confirm::undecided(format!("{what} got no answer")),
    };
    if let Some(said) = pk_cli_http::inconclusive(&answer) {
        return Confirm::Undecided {
            reason: format!("{what} answered HTTP {}", said.status),
            retry_after: said.retry_after,
        };
    }
    let status = answer.status().as_u16();
    if answer.status().is_redirection() {
        return Confirm::NotYet(format!(
            "{what} answered with a redirect (HTTP {status}): not signed in"
        ));
    }
    let content_type = answer
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut body = Vec::new();
    if answer.take(MAX_BODY + 1).read_to_end(&mut body).is_err() {
        return Confirm::undecided(format!("{what} answered HTTP {status} and its body did not arrive"));
    }
    if body.len() as u64 > MAX_BODY {
        return Confirm::undecided(format!("{what} answered with more than 1 MiB"));
    }
    judge(status, content_type.as_deref(), &body)
}

/// A read's client: no redirects followed, bounded in time, sending the
/// browser's user agent when there is one.
pub fn client(user_agent: Option<&str>) -> Result<reqwest::blocking::Client, CliError> {
    let mut builder = pk_cli_http::builder(BIN, env!("CARGO_PKG_VERSION"))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT);
    if let Some(agent) = user_agent {
        builder = builder.user_agent(agent);
    }
    builder
        .build()
        .map_err(|e| CliError::Other(format!("building the HTTP client: {}", e.without_url())))
}

/// A URL's path without its query, for a reason: reasons never carry a query.
pub(crate) fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = rest.find('/').map_or("/", |i| &rest[i..]);
    path.split(['?', '#']).next().unwrap_or("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pk_cli_browser::JarCookie;
    use std::io::Write;
    use std::net::TcpListener;

    const KEYS_JS: &str = "window.apiConfig = {\n  experienceApiKey: 'dummy-experience-key',\n  \
                           launchDarklyApiKey: 'x',\n  introspectApiKey: \"ZHVtbXk6a2V5\",\n};";

    fn keys() -> SiteKeys {
        SiteKeys::parse(KEYS_JS).unwrap()
    }

    /// The capture `pk-cli-browser` checks before Chrome starts, checked
    /// offline, with and without the request log.
    #[test]
    fn the_browser_capture_is_well_formed() {
        for log in [None, Some(Duration::from_secs(5))] {
            let capture = capture(log).unwrap();
            capture.validate().unwrap();
            assert_eq!(capture.label(), LABEL);
        }
        assert!(START_URL.starts_with(&format!("https://{HOST}/")));
        assert!(SESSION_CHECK_URL.starts_with(&format!("https://{HOST}/")));
        assert!(ENV_JS_URL.starts_with(&format!("https://{HOST}/")));
    }

    #[test]
    fn env_js_names_the_keys() {
        let k = keys();
        assert_eq!(k.experience, "dummy-experience-key");
        assert_eq!(k.introspect, "ZHVtbXk6a2V5");
        assert_eq!(format!("{k:?}"), "SiteKeys(..)", "Debug never shows a key");
        assert!(SiteKeys::parse("window.apiConfig = { experienceApiKey: '' };").is_none());
        let quoted =
            SiteKeys::parse(r#"window.apiConfig = {"experienceApiKey":"e-key","introspectApiKey":"i-key"};"#).unwrap();
        assert_eq!(
            (quoted.experience.as_str(), quoted.introspect.as_str()),
            ("e-key", "i-key")
        );
        assert!(SiteKeys::parse("experienceApiKey: 'a'").is_none(), "both keys or none");
        // A longer name that ends in the key's is not the key.
        assert!(SiteKeys::parse("xexperienceApiKey: 'a', introspectApiKey: 'b'").is_none());
        let spaced = SiteKeys::parse("experienceApiKey :  \"a\" , introspectApiKey:'b'").unwrap();
        assert_eq!((spaced.experience.as_str(), spaced.introspect.as_str()), ("a", "b"));
    }

    fn candidate(jar: bool) -> Captured {
        let built = Captured::for_test(LABEL, HOST).user_agent("Mozilla/5.0 (test)");
        if jar {
            built.jar([JarCookie::for_test(
                "dummy_visitor",
                "dummy-value",
                ".caremark.com",
                false,
                "/",
            )])
        } else {
            built
        }
        .build()
    }

    /// A candidate without a jar is not read at all.
    #[test]
    fn no_jar_is_not_signed_in_yet() {
        match confirm_candidate(&candidate(false)) {
            Confirm::NotYet(reason) => assert!(reason.contains("no caremark.com cookie"), "{reason}"),
            other => panic!("{other:?}"),
        }
        match Checker::new().confirm(&candidate(false)) {
            Confirm::NotYet(reason) => assert!(reason.contains("no caremark.com cookie"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    /// Within CHECK_EVERY of a check, the hook answers without a request.
    #[test]
    fn the_hook_paces_its_checks() {
        let checker = Checker::new();
        *checker.last.lock().unwrap() = Some(Instant::now());
        match checker.confirm(&candidate(true)) {
            Confirm::NotYet(reason) => assert!(reason.contains("waiting"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    /// What the site answers (docs/api.md), judged.
    #[test]
    fn judge_reads_the_sites_answers() {
        let json = Some("application/json");
        let signed_in = br#"{"statusCode":"0000","statusDescription":"Success","active":true}"#;
        assert!(matches!(judge(200, json, signed_in), Confirm::Ready));
        for visitor in [
            &br#"{"statusCode":"0000","active":false}"#[..],
            &br#"{"statusCode":"1001","statusDescription":"Invalid token"}"#[..],
            &br#"{}"#[..],
        ] {
            assert!(matches!(judge(200, json, visitor), Confirm::NotYet(_)));
        }
        assert!(matches!(judge(401, json, b"{}"), Confirm::NotYet(r) if r.contains("HTTP 401")));
        for (status, mime, body) in [
            (200, json, &b"{"[..]),
            (200, Some("text/html"), &b"<p>edge</p>"[..]),
            (304, None, &b""[..]),
        ] {
            assert!(
                matches!(judge(status, mime, body), Confirm::Undecided { .. }),
                "{status} {:?}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn a_reason_names_the_path_without_its_query() {
        assert_eq!(path_of("https://www.caremark.com/api/x?member=1#y"), "/api/x");
        assert_eq!(path_of("https://www.caremark.com"), "/");
        assert_eq!(path_of("http://127.0.0.1:9/a/b"), "/a/b");
    }

    /// Serves one canned answer on loopback and hands back the request's head
    /// and body, read whole so the connection closes cleanly.
    fn serve_once(answer: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/check?q=1", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).to_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .and_then(|n| n.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            stream.read_exact(&mut body).unwrap();
            stream.write_all(answer.as_bytes()).unwrap();
            format!("{head}{}", String::from_utf8_lossy(&body))
        });
        (url, handle)
    }

    /// The check's own decisions, before [`judge`]: what cannot decide is
    /// `Undecided` (with the provider's `Retry-After`), a redirect is
    /// `NotYet`, and the rest reaches `judge`. The request carries the jar,
    /// the site's Basic key and the browser's user agent, and follows no
    /// redirect.
    #[test]
    fn the_check_decides_what_says_nothing_about_the_session() {
        let ua = client(Some("Mozilla/5.0 (test)")).unwrap();
        let undecided = |answer: &'static str, retry: Option<u64>| {
            let (url, server) = serve_once(answer);
            match check_at(&ua, &url, &keys(), "a=b; c=d") {
                Confirm::Undecided { reason, retry_after } => {
                    assert!(reason.starts_with("POST /api/check answered HTTP"), "{reason}");
                    assert!(!reason.contains("q=1"), "{reason}");
                    assert_eq!(retry_after, retry.map(Duration::from_secs));
                }
                other => panic!("{answer:?}: {other:?}"),
            }
            server.join().unwrap()
        };
        let head = undecided("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n", None);
        assert!(head.starts_with("post /api/check?q=1 "), "{head}");
        assert!(head.contains("\r\ncookie: a=b; c=d\r\n"), "{head}");
        assert!(head.contains("\r\nauthorization: basic zhvtbxk6a2v5\r\n"), "{head}");
        assert!(head.contains("\r\nuser-agent: mozilla/5.0 (test)\r\n"), "{head}");
        undecided(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\nContent-Length: 0\r\n\r\n",
            Some(30),
        );
        undecided(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/html\r\nContent-Length: 9\r\n\r\n<p>no</p>",
            None,
        );

        let plain = client(None).unwrap();
        let (url, server) =
            serve_once("HTTP/1.1 302 Found\r\nLocation: https://example.com/sign-in\r\nContent-Length: 0\r\n\r\n");
        match check_at(&plain, &url, &keys(), "a=b") {
            Confirm::NotYet(reason) => assert!(reason.contains("redirect (HTTP 302)"), "{reason}"),
            other => panic!("{other:?}"),
        }
        let head = server.join().unwrap();
        assert!(head.contains(&format!("\r\nuser-agent: {BIN}/")), "{head}");

        let (url, server) =
            serve_once("HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}");
        assert!(matches!(check_at(&plain, &url, &keys(), "a=b"), Confirm::NotYet(r) if r.contains("HTTP 401")));
        server.join().unwrap();
        let (url, server) = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 35\r\n\r\n{\"statusCode\":\"0000\",\"active\":true}",
        );
        assert!(matches!(check_at(&plain, &url, &keys(), "a=b"), Confirm::Ready));
        server.join().unwrap();
    }

    #[test]
    fn no_answer_is_undecided() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let plain = client(None).unwrap();
        match check_at(&plain, &format!("http://127.0.0.1:{port}/api/check"), &keys(), "a=b") {
            Confirm::Undecided { reason, .. } => assert_eq!(reason, "POST /api/check got no answer"),
            other => panic!("{other:?}"),
        }
    }
}
