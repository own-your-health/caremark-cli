//! The member-site reads, made with a profile's stored session (docs/api.md
//! § Member site).
//!
//! - A request carries the session's `Cookie` header, the browser's user
//!   agent captured with it and `Accept: application/json`. A read of the
//!   site's experience API also sends the site's public experience key from
//!   env.js (`site::SiteKeys`) and the experience's ids, as the site's own
//!   pages do.
//! - It follows no redirect (the site sends a visitor to its sign-in) and is
//!   bounded in time: 30 s for the whole answer, 10 s to connect.
//! - One request at a time, at least [`PACE`] after the last answer.
//! - Read-only: `GET`s, and `POST`s only to the experiences in
//!   [`EXPERIENCES`], each a read. `caremark api`'s requests are checked for
//!   that before one is sent ([`api_request`]).
//! - [`refuse`] turns the answers that are no data into errors: a 401 or a
//!   redirect is a refused session (exit 3); the edge's 403 page, a 429 and a
//!   5xx are exit 5.
//! - A `Set-Cookie` on an answer updates the client's jar; the caller writes
//!   the updated session back once the command has read what it needed
//!   ([`Client::updated_cookie_header`]).

use std::cell::{Cell, RefCell};
use std::io::Read as _;
use std::time::{Duration, Instant};

use pk_cli_config::profile::ProfileName;
use pk_cli_core::{CliError, Reason};
use pk_cli_secrets::Secret;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, COOKIE, RETRY_AFTER, SET_COOKIE, USER_AGENT,
};
use serde_json::Value;

use crate::cli::{ApiMethod, BIN};
use crate::session::Session;
use crate::site::{self, SiteKeys};

/// The member site's origin.
pub const BASE: &str = site::ORIGIN;
/// The least time between one answer and the next request.
pub const PACE: Duration = Duration::from_secs(1);
const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The most of a JSON answer read.
const MAX_JSON: u64 = 16 * 1024 * 1024;
/// The most of a document read.
pub const MAX_FILE: u64 = 64 * 1024 * 1024;

/// One read of the site's experience API: a `POST` of a JSON body to
/// `<path>/<id>`, with the experience's ids as headers. Every one here is
/// a read: it changes nothing at Caremark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Experience {
    /// The name `caremark` gives it (`caremark api POST <name>`).
    pub name: &'static str,
    /// The API path the id is appended to.
    pub path: &'static str,
    /// The `x-experienceId` header and the path's last segment.
    pub id: &'static str,
    /// The `x-clientId` header.
    pub client_id: &'static str,
    /// The other headers the site's own page sends with it (`x-route`,
    /// `x-appName`, `x-source-type`).
    pub headers: &'static [(&'static str, &'static str)],
}

impl Experience {
    pub fn url_path(&self) -> String {
        format!("{}/{}", self.path, self.id)
    }
}

const LOAD: &str = "/api/client/experience/v2/load";

/// The experiences caremark reads (docs/api.md § Experiences), each found
/// in the site's own pages and named after the call that makes it there.
pub const EXPERIENCES: &[Experience] = &[
    // The financial summary page: every patient on the account, each
    // prescription's fills with what was paid.
    Experience {
        name: "financial-summary",
        path: LOAD,
        id: "13ed331c-3e9c-4fdf-bab2-6396a12a3623",
        client_id: "benefits",
        headers: &[("x-source-type", "pbm")],
    },
    // The drug cost checker's claim history: one person's adjudicated
    // pharmacy claims.
    Experience {
        name: "claim-history",
        path: LOAD,
        id: "19378b96-0bc0-43c1-a37c-9308e71a22f1",
        client_id: "benefits",
        headers: &[],
    },
    // The dashboard's family list.
    Experience {
        name: "member-family",
        path: LOAD,
        id: "e496ecd3-4067-4499-a1cd-27e353397cb4",
        client_id: "client_engagement",
        headers: &[("x-route", "PHARMACY-GKE-RKE"), ("x-appName", "cmk-notifications")],
    },
    // The dashboard's member and plan details.
    Experience {
        name: "member-info",
        path: LOAD,
        id: "656e55d9-691d-4b48-b5fa-cefa54284336",
        client_id: "client_engagement",
        headers: &[("x-route", "CMK-PHARMACY-HA"), ("x-appName", "cmk-dashboard")],
    },
    // The member profile.
    Experience {
        name: "member-profile",
        path: LOAD,
        id: "53e50e5a-6c39-40a7-9676-55c4f2395600",
        client_id: "client_engagement",
        headers: &[("x-route", "CMK-PHARMACY-HA")],
    },
    // The plan summary's deductible.
    Experience {
        name: "plan-deductible",
        path: LOAD,
        id: "79d00edc-3b6d-46bc-b9e6-54ea048d9513",
        client_id: "benefits",
        headers: &[],
    },
    // The home page's prescription list.
    Experience {
        name: "prescriptions-list",
        path: LOAD,
        id: "3386858f-7d5e-41ab-ab18-f46fa4205c93",
        client_id: "benefits",
        headers: &[],
    },
];

/// The experience whose name, id or `<path>/<id>` this is.
pub fn experience_for(path: &str) -> Option<&'static Experience> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    EXPERIENCES
        .iter()
        .find(|e| e.url_path() == path || e.id == path || e.name == path)
}

/// The experience named `name`; a name missing from [`EXPERIENCES`] is a
/// bug, caught by the tests.
pub fn experience(name: &str) -> &'static Experience {
    EXPERIENCES
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no experience named {name}"))
}

/// A session's cookies, in the order the header gave them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jar {
    cookies: Vec<(String, String)>,
    changed: bool,
}

impl Jar {
    pub fn parse(header: &str) -> Jar {
        let cookies = header
            .split(';')
            .filter_map(|pair| {
                let (name, value) = pair.trim().split_once('=')?;
                (!name.trim().is_empty()).then(|| (name.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        Jar {
            cookies,
            changed: false,
        }
    }

    pub fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.cookies.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// Whether the jar holds the site's maintenance waiting-room cookie
    /// ([`in_maintenance`]).
    pub fn in_maintenance(&self) -> bool {
        self.cookies.iter().any(|(n, _)| maintenance_cookie(n))
    }

    /// One `Set-Cookie` value: a new value replaces the cookie's, a new
    /// cookie is added, and an empty value or `Max-Age` of zero or less
    /// removes it. The other attributes (domain, path, expiry) are the
    /// browser's business; the site's are all on its own hosts.
    pub fn apply(&mut self, set_cookie: &str) {
        let mut parts = set_cookie.split(';');
        let Some((name, value)) = parts.next().and_then(|p| p.split_once('=')) else {
            return;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() {
            return;
        }
        let expired = parts.any(|a| {
            a.trim().split_once('=').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case("max-age") && v.trim().parse::<i64>().is_ok_and(|n| n <= 0)
            })
        });
        let at = self.cookies.iter().position(|(n, _)| n == name);
        match (at, value.is_empty() || expired) {
            (Some(i), true) => {
                self.cookies.remove(i);
                self.changed = true;
            }
            (Some(i), false) if self.cookies[i].1 != value => {
                self.cookies[i].1 = value.to_string();
                self.changed = true;
            }
            (None, false) => {
                self.cookies.push((name.to_string(), value.to_string()));
                self.changed = true;
            }
            _ => {}
        }
    }

    pub fn changed(&self) -> bool {
        self.changed
    }
}

/// What a read sends and to whom: the session's cookies and user agent.
pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
    jar: RefCell<Jar>,
    pace: Duration,
    last: Cell<Option<Instant>>,
    profile: ProfileName,
    keys: RefCell<Option<SiteKeys>>,
}

impl Client {
    /// A client for `profile`'s `session` against `base` ([`BASE`], or a
    /// test's loopback server), `pace` apart.
    pub fn new(session: &Session, profile: &ProfileName, base: &str, pace: Duration) -> Result<Client, CliError> {
        let agent = session
            .user_agent
            .clone()
            .unwrap_or_else(|| format!("{BIN}/{}", env!("CARGO_PKG_VERSION")));
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&agent)
                .map_err(|_| CliError::Other("the stored user agent is not a valid HTTP header value".into()))?,
        );
        // A plain client: no cookie store (the jar is ours), no redirects.
        let http = reqwest::blocking::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| CliError::Other(format!("building the HTTP client: {}", e.without_url())))?;
        Ok(Client {
            http,
            base: base.trim_end_matches('/').to_string(),
            jar: RefCell::new(Jar::parse(session.cookie_header.expose())),
            pace,
            last: Cell::new(None),
            profile: profile.clone(),
            keys: RefCell::new(None),
        })
    }

    /// The same client with `keys` already known, for the tests.
    pub fn with_keys(self, keys: SiteKeys) -> Client {
        *self.keys.borrow_mut() = Some(keys);
        self
    }

    /// The site's public keys, read from env.js once per client.
    fn keys(&self) -> Result<SiteKeys, CliError> {
        if let Some(keys) = self.keys.borrow().clone() {
            return Ok(keys);
        }
        let answer = self.send(
            "GET",
            "/memberportal/env.js",
            self.http.get(self.url("/memberportal/env.js")),
            MAX_JSON,
        )?;
        refuse(&answer, &self.profile)?;
        let keys = SiteKeys::parse(&String::from_utf8_lossy(&answer.body)).ok_or_else(|| {
            CliError::Upstream(
                "GET /memberportal/env.js no longer assigns experienceApiKey and introspectApiKey; the site may \
                 have changed (docs/api.md)"
                    .into(),
            )
        })?;
        *self.keys.borrow_mut() = Some(keys.clone());
        Ok(keys)
    }

    /// The site's session check with this session: whether it is signed in.
    pub fn signed_in(&self) -> Result<bool, CliError> {
        let keys = self.keys()?;
        let mut basic = HeaderValue::from_str(&format!("Basic {}", keys.introspect))
            .map_err(|_| CliError::Upstream("the site's introspection key is not a valid header value".into()))?;
        basic.set_sensitive(true);
        let path = site::path_of(site::SESSION_CHECK_URL);
        let request = self
            .http
            .post(self.url(path))
            .header(AUTHORIZATION, basic)
            .header(ACCEPT, "application/json");
        let answer = self.send("POST", path, request, MAX_JSON)?;
        match answer.status {
            401 => return Ok(false),
            300..=399 => return Ok(false),
            _ => refuse(&answer, &self.profile)?,
        }
        let doc = json_of(&answer)?;
        Ok(site::is_signed_in(&doc))
    }

    /// One experience read: its answer as JSON, after the refusals every
    /// read gets.
    pub fn experience(&self, exp: &Experience, body: &Value) -> Result<Value, CliError> {
        let keys = self.keys()?;
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let value = |s: &str| {
            HeaderValue::from_str(s).map_err(|_| CliError::Usage(format!("`{s}` is not a valid header value")))
        };
        let mut key = value(&keys.experience)?;
        key.set_sensitive(true);
        headers.insert("x-api-key", key);
        headers.insert("x-experienceId", value(exp.id)?);
        headers.insert("x-clientId", value(exp.client_id)?);
        headers.insert("x-channel-type", HeaderValue::from_static("WEB"));
        for (name, v) in exp.headers {
            headers.insert(*name, value(v)?);
        }
        let url_path = exp.url_path();
        let request = self
            .http
            .post(self.url(&url_path))
            .headers(headers)
            .body(body.to_string());
        let answer = self.send("POST", &url_path, request, MAX_JSON)?;
        refuse(&answer, &self.profile)?;
        json_of(&answer)
    }

    /// `caremark api`: one checked request, its answer as JSON, after the
    /// refusals every read gets.
    pub fn api(&self, request: &ApiRequest) -> Result<Value, CliError> {
        match request {
            ApiRequest::Get(path) => {
                let builder = self.http.get(self.url(path)).header(ACCEPT, "application/json");
                let answer = self.send("GET", path, builder, MAX_JSON)?;
                refuse(&answer, &self.profile)?;
                json_of(&answer)
            }
            ApiRequest::Experience(exp, body) => self.experience(exp, body),
            ApiRequest::SessionCheck => {
                let signed_in = self.signed_in()?;
                Ok(serde_json::json!({ "signed_in": signed_in }))
            }
        }
    }

    /// One file the site serves (a PDF): its bytes and content type.
    pub fn file(&self, path: &str) -> Result<(Vec<u8>, Option<String>), CliError> {
        let builder = self.http.get(self.url(path)).header(ACCEPT, "application/pdf, */*");
        let answer = self.send("GET", path, builder, MAX_FILE)?;
        refuse(&answer, &self.profile)?;
        Ok((answer.body, answer.content_type))
    }

    /// The session's cookie header as the answers left it, when one changed it.
    pub fn updated_cookie_header(&self) -> Option<Secret> {
        let jar = self.jar.borrow();
        jar.changed().then(|| Secret::new(jar.header()))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        request: reqwest::blocking::RequestBuilder,
        limit: u64,
    ) -> Result<Answer, CliError> {
        let what = format!("{method} {}", site::path_of(path));
        if let Some(last) = self.last.get() {
            let since = last.elapsed();
            if since < self.pace {
                std::thread::sleep(self.pace - since);
            }
        }
        let mut cookie = HeaderValue::from_str(&self.jar.borrow().header())
            .map_err(|_| CliError::Other("the session's Cookie header is not a valid HTTP header value".into()))?;
        cookie.set_sensitive(true);
        let result = request.header(COOKIE, cookie).send();
        let answer = match result {
            Ok(a) => a,
            Err(e) => {
                self.last.set(Some(Instant::now()));
                return Err(if e.is_timeout() {
                    CliError::Upstream(format!("{what} timed out after {} s", TIMEOUT.as_secs()))
                } else {
                    CliError::Upstream(format!("{what} got no answer from the member site"))
                });
            }
        };
        let status = answer.status().as_u16();
        let header = |name| {
            answer
                .headers()
                .get(name)
                .and_then(|v: &HeaderValue| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header(CONTENT_TYPE);
        let retry_after = header(RETRY_AFTER);
        {
            let mut jar = self.jar.borrow_mut();
            for v in answer.headers().get_all(SET_COOKIE) {
                if let Ok(v) = v.to_str() {
                    jar.apply(v);
                }
            }
        }
        let mut body = Vec::new();
        let read = answer.take(limit + 1).read_to_end(&mut body);
        self.last.set(Some(Instant::now()));
        if read.is_err() {
            return Err(CliError::Upstream(format!(
                "{what} answered HTTP {status} and its body did not arrive"
            )));
        }
        if body.len() as u64 > limit {
            return Err(CliError::Upstream(format!(
                "{what} answered with more than {} MiB",
                limit >> 20
            )));
        }
        let maintenance = self.jar.borrow().in_maintenance();
        Ok(Answer {
            what,
            status,
            content_type,
            retry_after,
            body,
            maintenance,
        })
    }
}

/// What `caremark api` may send, checked by [`api_request`] before any
/// request.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiRequest {
    /// A `GET` of a path on the member site.
    Get(String),
    /// A `POST` to one of [`EXPERIENCES`], with its body.
    Experience(&'static Experience, Value),
    /// A `POST` to the session check.
    SessionCheck,
}

const READ_ONLY: &str = "caremark is read-only: `api` sends a GET, or a POST to the session check or to one of the \
                         experiences it reads (docs/api.md § Experiences)";

/// The path of `path` on the member site: a path with a leading `/`, or a
/// URL on its host over https. Anything else is exit 2, before a request.
pub fn site_path(path: &str) -> Result<String, CliError> {
    let path = path.trim();
    let rest = if let Some(rest) = path.strip_prefix(site::ORIGIN) {
        if rest.is_empty() {
            "/"
        } else {
            rest
        }
    } else {
        path
    };
    if !rest.starts_with('/') || rest.starts_with("//") {
        return Err(CliError::Usage(format!(
            "`{path}` is not a path on {} (start it with /)",
            site::ORIGIN
        )));
    }
    if rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CliError::Usage("a path holds no spaces or control characters".into()));
    }
    Ok(rest.to_string())
}

/// `caremark api`'s request, checked: a `GET` of a path on the site, or a
/// `POST` to the session check or a listed experience with a JSON body.
pub fn api_request(method: ApiMethod, path: &str, data: Option<&str>) -> Result<ApiRequest, CliError> {
    // An experience by name: `caremark api POST financial-summary`.
    if method == ApiMethod::Post && !path.starts_with('/') && !path.contains("://") {
        if let Some(exp) = EXPERIENCES.iter().find(|e| e.name == path) {
            return Ok(ApiRequest::Experience(exp, experience_body(data)?));
        }
    }
    let path = site_path(path)?;
    match method {
        ApiMethod::Get => {
            if data.is_some() {
                return Err(CliError::Usage("--data goes with POST".into()));
            }
            Ok(ApiRequest::Get(path))
        }
        ApiMethod::Post => {
            if path == site::path_of(site::SESSION_CHECK_URL) {
                return Ok(ApiRequest::SessionCheck);
            }
            let Some(exp) = experience_for(&path) else {
                return Err(CliError::Usage(format!("{READ_ONLY}; `{path}` is none of them")));
            };
            Ok(ApiRequest::Experience(exp, experience_body(data)?))
        }
    }
}

/// An experience read's body: `--data`, a JSON object (the site's pages
/// send `{"data": {...}}`).
fn experience_body(data: Option<&str>) -> Result<Value, CliError> {
    let body = match data {
        None => {
            return Err(CliError::Usage(
                "POST needs --data with the request body as JSON".into(),
            ))
        }
        Some(d) => serde_json::from_str::<Value>(d).map_err(|_| CliError::Usage("--data is not JSON".into()))?,
    };
    if !body.is_object() {
        return Err(CliError::Usage("--data is a JSON object".into()));
    }
    Ok(body)
}

/// An answer, read whole.
pub struct Answer {
    /// `POST /path`, no query.
    pub what: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
    /// The jar held the site's maintenance waiting-room cookie once the
    /// answer's cookies were applied.
    pub maintenance: bool,
}

/// Akamai's visitor-prioritization cookie (`akavpau_<property>_<policy>`)
/// for a policy named for maintenance: caremark.com sets it, and answers
/// 503, while it is down for maintenance (docs/api.md).
fn maintenance_cookie(name: &str) -> bool {
    name.starts_with("akavpau_") && name.contains("maintenance")
}

/// Whether a `Cookie` header holds the site's maintenance waiting-room
/// cookie: names only, no value is read.
pub fn in_maintenance(cookie_header: &str) -> bool {
    cookie_header
        .split(';')
        .filter_map(|pair| pair.split_once('=').map(|(n, _)| n.trim()))
        .any(maintenance_cookie)
}

fn login_remedy(profile: &ProfileName) -> String {
    format!("{BIN} auth login --browser --profile {profile} --overwrite")
}

/// An answer's JSON, or exit 5.
fn json_of(a: &Answer) -> Result<Value, CliError> {
    let what = &a.what;
    if !a.content_type.as_deref().is_some_and(|t| t.contains("json")) {
        return Err(CliError::Upstream(format!(
            "{what} answered HTTP {} with {}, not JSON",
            a.status,
            a.content_type.as_deref().unwrap_or("no content type")
        )));
    }
    serde_json::from_slice(&a.body).map_err(|_| CliError::Upstream(format!("{what} answered JSON that does not parse")))
}

/// The answers that are no data whatever the read: a refused session, the
/// edge's page, too many requests, the site's own failures.
pub fn refuse(a: &Answer, profile: &ProfileName) -> Result<(), CliError> {
    let what = &a.what;
    let status = a.status;
    let html = a.content_type.as_deref().is_some_and(|t| t.contains("html"));
    match status {
        200..=299 => Ok(()),
        401 => Err(CliError::Auth(format!(
            "the member site refused profile `{profile}`'s session ({what}: HTTP 401): it has signed out or expired"
        ))
        .because(Reason::CredentialRefused)
        .with_remedy(login_remedy(profile))),
        300..=399 => Err(CliError::Auth(format!(
            "{what} answered with a redirect (HTTP {status}), as the site answers a visitor: profile `{profile}`'s \
             session has signed out"
        ))
        .because(Reason::CredentialRefused)
        .with_remedy(login_remedy(profile))),
        403 if html => Err(CliError::Upstream(format!(
            "the member site's edge refused {what} (HTTP 403 with its error page)"
        ))
        .because(Reason::ClientBlocked)
        .with_remedy(format!(
            "sign in again with `{}`, which keeps the browser's user agent",
            login_remedy(profile)
        ))),
        403 => Err(CliError::Upstream(format!("the member site refused {what} (HTTP 403)")).because(Reason::Forbidden)),
        429 => {
            let mut e = CliError::Upstream(format!(
                "the member site answered {what} with HTTP 429: too many requests"
            ));
            e = match &a.retry_after {
                Some(after) => e.with_remedy(format!("try again after {} s", after.trim())),
                None => e.with_remedy("try again in a few minutes"),
            };
            Err(e)
        }
        500..=599 if a.maintenance => Err(CliError::Upstream(format!(
            "caremark.com is down for maintenance: it answered {what} with HTTP {status} and set its maintenance \
             waiting-room cookie"
        ))
        .with_remedy("try again in a few hours; the site's maintenance is often overnight")),
        500..=599 => Err(
            CliError::Upstream(format!("the member site answered {what} with HTTP {status}"))
                .with_remedy("try again later"),
        ),
        400 => Err(CliError::Upstream(format!(
            "the member site refused {what} as a bad request (HTTP 400); the site may have changed"
        ))
        .because(Reason::BadRequest)),
        _ => Err(CliError::Upstream(format!(
            "the member site answered {what} with HTTP {status}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adult1() -> ProfileName {
        ProfileName::parse("adult1").unwrap()
    }

    fn answer(status: u16, content_type: &str, body: &str) -> Answer {
        Answer {
            what: "POST /api/x".into(),
            status,
            content_type: Some(content_type.into()),
            retry_after: None,
            body: body.as_bytes().to_vec(),
            maintenance: false,
        }
    }

    #[test]
    fn a_503_with_the_maintenance_cookie_says_so() {
        let p = adult1();
        let mut down = answer(503, "text/html", "");
        down.maintenance = true;
        let e = refuse(&down, &p).unwrap_err();
        assert_eq!(e.exit_code(), 5);
        assert!(e.message().contains("down for maintenance"), "{e}");
        let plain = refuse(&answer(503, "text/html", ""), &p).unwrap_err();
        assert!(!plain.message().contains("maintenance"), "{plain}");
        assert!(in_maintenance(
            "a=1; akavpau_vp_www_example_com_example_maintenance=x; b=2"
        ));
        assert!(!in_maintenance("a=1; akavpau_vp_www_example_com_queue=x"));
        assert!(!in_maintenance("maintenance=1"));
    }

    #[test]
    fn api_requests_are_reads_on_the_member_site() {
        assert_eq!(
            api_request(ApiMethod::Get, "/memberportal/env.js", None).unwrap(),
            ApiRequest::Get("/memberportal/env.js".into())
        );
        assert_eq!(
            api_request(
                ApiMethod::Get,
                "https://www.caremark.com/pharmacy/rx/prescriptions?x=1",
                None
            )
            .unwrap(),
            ApiRequest::Get("/pharmacy/rx/prescriptions?x=1".into())
        );
        assert_eq!(
            api_request(ApiMethod::Get, "https://www.caremark.com", None).unwrap(),
            ApiRequest::Get("/".into())
        );
        assert_eq!(
            api_request(ApiMethod::Post, "/api/oauth2/v2/introspect", None).unwrap(),
            ApiRequest::SessionCheck
        );
        for bad in [
            "https://example.com/x",
            "http://www.caremark.com/x",
            "https://www.caremark.com.example.com/x",
            "//example.com/x",
            "relative/path",
            "/a b",
        ] {
            assert_eq!(
                api_request(ApiMethod::Get, bad, None).unwrap_err().exit_code(),
                2,
                "{bad}"
            );
        }
        assert_eq!(
            api_request(ApiMethod::Get, "/x", Some("{}")).unwrap_err().exit_code(),
            2,
            "--data is POST's"
        );
        // A POST anywhere but the session check and the listed experiences
        // is refused before a request.
        for path in [
            "/api/oauth2/v2/token",
            "/api/client/experience/v2/load/unlisted",
            "/pharmacy/rx",
        ] {
            let e = api_request(ApiMethod::Post, path, Some("{}")).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{path}");
            assert!(e.message().contains("read-only"), "{e}");
        }
    }

    #[test]
    fn every_experience_is_on_the_site_and_listed_once() {
        let mut ids = std::collections::BTreeSet::new();
        for exp in EXPERIENCES {
            assert!(
                exp.path.starts_with("/api/") || exp.path.starts_with("/apims/"),
                "{exp:?}"
            );
            assert!(ids.insert(exp.id), "{} listed twice", exp.id);
            assert_eq!(experience_for(&exp.url_path()), Some(exp));
            assert_eq!(experience(exp.name), exp);
            // A read, by its name: nothing that orders, refills, submits,
            // marks or changes.
            for verb in [
                "order", "refill", "submit", "mark", "dismiss", "save", "set", "update", "delete", "cart",
            ] {
                assert!(!exp.name.contains(verb), "{}", exp.name);
            }
            match api_request(ApiMethod::Post, exp.name, Some(r#"{"data":{}}"#)).unwrap() {
                ApiRequest::Experience(e, _) => assert_eq!(e, exp),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn refuse_reads_the_sites_answers() {
        let p = adult1();
        refuse(&answer(200, "application/json", "{}"), &p).unwrap();
        let e = refuse(&answer(401, "application/json", "{}"), &p).unwrap_err();
        assert_eq!(e.exit_code(), 3);
        assert!(
            e.remedy().unwrap().contains("auth login --browser --profile adult1"),
            "{e}"
        );
        assert_eq!(refuse(&answer(302, "text/html", ""), &p).unwrap_err().exit_code(), 3);
        let edge = refuse(&answer(403, "text/html", "<p>no</p>"), &p).unwrap_err();
        assert_eq!(edge.exit_code(), 5);
        assert_eq!(edge.reason(), Some(Reason::ClientBlocked));
        for status in [400, 403, 404, 429, 500, 503] {
            assert_eq!(
                refuse(&answer(status, "application/json", "{}"), &p)
                    .unwrap_err()
                    .exit_code(),
                5,
                "{status}"
            );
        }
        assert!(json_of(&answer(200, "text/html", "<p/>")).is_err());
        assert!(json_of(&answer(200, "application/json", "{")).is_err());
        assert_eq!(json_of(&answer(200, "application/json", "{\"a\":1}")).unwrap()["a"], 1);
    }

    #[test]
    fn set_cookie_updates_the_jar() {
        let mut jar = Jar::parse("a=1; b=2");
        jar.apply("a=1; Path=/");
        assert!(!jar.changed());
        jar.apply("b=3; Path=/; Secure");
        jar.apply("c=4");
        jar.apply("a=; Max-Age=0");
        assert!(jar.changed());
        assert_eq!(jar.header(), "b=3; c=4");
        assert_eq!(jar.get("c"), Some("4"));
        jar.apply("c=5; Max-Age=-1");
        assert_eq!(jar.header(), "b=3");
    }
}
