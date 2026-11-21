# AGENTS.md — caremark-cli

`caremark` is a read-only CLI for a CVS Caremark member's prescription fills
and pharmacy claims. It reads the member site (www.caremark.com, signed in
per profile) and emits FHIR for the household's medical-records system
(`records-stream/v1`, `medrec import --from caremark`). It conforms to
piekstra-cli/1 and the `health-records/v1` profile, on the `pk-cli-*` crates
from [cli-common](https://github.com/piekstra/cli-common) and
`pk-med-records` from
[medical-common](https://github.com/piekstra/medical-common), which owns the
records stream and the profile's DTOs. Every `pk-cli-*` stays on the
cli-common tag medical-common pins.

## Layout

| File | Owns |
|---|---|
| `src/cli.rs` | the clap tree |
| `src/app.rs` | the commands, argument checks, output |
| `src/auth.rs` | `auth login`, `status`, `logout` of one `--profile`, `auth reset`: the browser-profile seams, `--log-requests` |
| `src/session.rs` | `SessionStore`: get, put and delete a profile's session in the vault; the only code that knows where it lives |
| `src/source.rs` | which configured profile a command signs in as (`resolve_profile`) |
| `src/site.rs` | the member site's sign-in: the browser capture, the site's public keys (`SiteKeys`, from env.js), the session check (`SESSION_CHECK_URL`, `judge`) |
| `src/site_cmd.rs` | the commands that read the site (`patients`, `rx`, `costs`, `records export`, `api`) against an `Env` (the vault and the site, or a test's): the year windows, the fills, the stream |
| `src/labels.rs` | `patients label`: the household's names for a profile's people, kept in the profile's settings with every person the family read has listed (`record`), so the command sends no request |
| `src/portal.rs` | every path caremark reads in an answer, into `model` |
| `src/model.rs` | a person, a fill, a prescription, apart from the site's shapes; the stable ids |
| `src/fhir.rs` | the FHIR written by hand from `model` (`docs/fhir.md`) |
| `src/cents.rs` | amounts as whole cents |
| `src/client.rs` | the member-site reads: the experiences (`EXPERIENCES`), pacing, the cookie jar, `refuse` (answers to exit codes), and `api_request`, which checks an `api` request is a read before it is sent |
| `src/shape.rs` | an answer's shape without its values, for mapping (`examples/shape.rs`) |
| `docs/api.md` | how Caremark data can be reached, with the sources' own wording, the endpoints and the traps |
| `docs/fhir.md` | every FHIR field caremark writes and where it comes from |

## Medical data rules

These come from the household's plan and are not negotiable.

- The repository carries no personal data, in code, fixtures, docs, commits,
  pull requests or issues. Fixtures are synthetic (`tests/fixtures/README.md`);
  `tests/no_personal_data.rs` scans every tracked file and checks every
  identity field in the fixtures against its dummy shape.
- Records and their contents are never sent to an outside service: no
  health-data aggregator, cloud OCR or other company's API. A Claude session
  is not one of these (the household's decision, 2026-11-17). A session may
  read, print and summarize the household's real records and names when the
  work needs it: debugging a real export, mapping the member site with real
  answers, checking a pull, answering the owner. Real data stays out of
  tracked files, commits, pull requests and issues, and out of messages to
  other sessions unless they need it. Tests and fixtures stay synthetic, and
  a raw capture is deleted from `/tmp` once it has served.
- No FHIR or SMART libraries, no aggregators, no cloud OCR or AI. The
  household's own crates (cli-common, medical-common) are not third party.
  General-purpose crates are fine.
- Read-only. Nothing writes to Caremark: no refill, order, transfer,
  message or setting. A write would need SPEC §1.3's confirmation gate and a
  change to this paragraph in the same commit.

## The member site

caremark reads the member site because no API serves an employer plan's
member (`docs/api.md` § Route). The site's Terms of Use carry a risk each
user weighs for themselves (the README's "The site's terms"). Be gentle
with it: one request at a time, at least `client::PACE` apart, no bulk
crawling.

- **Sign-in.** `auth login --browser --profile <NAME>` opens Chrome in the
  profile's kept browser profile (`Launch::profile_name`) at the site's
  home page. The owner types the sign-in there; caremark never drives the
  page and never fills it: the sign-in page runs a bot check that refuses
  scripted input (`docs/api.md` § Traps). The capture reads the
  `caremark.com` cookie jar and the browser's user agent at the browser
  level, and hands them over once the site's session check accepts them
  (`site::Checker`, at most once per `site::CHECK_EVERY`). `--stdin` takes a
  Cookie header copied from DevTools with `--user-agent`.
- **Session.** The vault entry `<profile>/session`
  (`pk_cli_auth::vault::Vault::for_binary("caremark")`: one keychain item,
  `vault-key` in `piekstra.caremark`, and an encrypted file per entry under
  `~/.config/caremark/vault/`). `profiles remove` removes the session and
  the kept browser profile before the profile; `auth reset` is
  `Vault::clear`.
- **Keys.** The experience API and the session check take the site's public
  client keys, which every visitor's pages read from `/memberportal/env.js`.
  caremark reads them at run time (`site::SiteKeys`) and never stores,
  prints or commits them.
- **Reads.** `client::Client` POSTs to the experiences in
  `client::EXPERIENCES` only, each a read found in the site's own pages, and
  GETs nothing else of note. It follows no redirect and maps answers to exit
  codes: a 401 or a redirect is 3 (`auth login --browser --profile <NAME>
  --overwrite`), the edge's 403 page, 429 and 5xx are 5. A `Set-Cookie`
  updates the jar, written back after every command, unless a newer
  sign-in replaced the stored session meanwhile (`site_cmd::write_back`):
  a command the site cut short (exit 5) or one that failed on its own
  check after a read (a `--patient` that names no one) keeps the cookies
  the site's answers set. A command that ended on a refused session
  (exit 3) writes none.
- **Mapping.** `auth login --browser --log-requests <SECONDS>` prints the
  endpoints the site's pages called, ids hidden except the experience ids,
  which are the site's constants. `caremark api POST <experience> --data
  <JSON> | cargo run -q --example shape` prints an answer's shape, never its
  values, for `docs/api.md` and the synthetic fixtures.

## Tests are offline, always

`cargo test` reads no real keychain item or browser profile, stores no
session, contacts nothing beyond loopback, and reads no real record. Every
test that runs a credential-reading command sets
`CAREMARK_KEYCHAIN_SERVICE=piekstra.caremark.hermetic-tests`, a service that
holds nothing, with a temp `XDG_CONFIG_HOME`: under that variable the vault
is `$XDG_CONFIG_HOME/caremark/vault`, never the owner's, and caremark leaves
the kept browser profiles alone. No binary test stores a session, so none
writes or reads a key (the vault reads its key only when an entry exists).
Unit and in-process tests use `Vault::new(MemoryKeys::new(), dir)`.

- `tests/cli_surface.rs`: every `--help`, exit codes, JSON errors, argument
  checks before the site is read, profile selection, `NO_COLOR`, a per-test
  `XDG_CONFIG_HOME`.
- `tests/site_e2e.rs`: every site command in-process against a loopback
  server answering with `tests/fixtures/site/`, the session in an in-memory
  vault: the windows, the stream, a stream cut short keeping the years
  read before it, the financial summary's unmatched fills as a skip, the
  refusals as exit codes, the cookie write-back, the human tables.
- `tests/no_personal_data.rs`.
- Unit tests beside the code: the capture, the session check against a
  loopback server, `api_request`, `refuse`, the cookie jar.

`make verify` = fmt-check, clippy, test, smoke.
