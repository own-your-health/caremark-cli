# caremark

Unofficial; not affiliated with or endorsed by CVS Caremark or CVS Health.

`caremark` reads a CVS Caremark member's prescription fills and pharmacy
claims from the member site, www.caremark.com, and hands them to the
household's medical-records system as FHIR (`records-stream/v1`, read by
`medrec import --from caremark`). It is read-only: nothing it does writes to
Caremark.

`docs/api.md` records why there is no API route (no FHIR or patient-access
API serves an employer plan's Caremark member), what the site's Terms of
Use say about automated access (the household accepted that risk), and
every read caremark makes.

## Install

```sh
cargo install --git https://github.com/piekstra/caremark-cli --locked
# or download a release: caremark-<target>.tar.gz, then `caremark self-update` later
```

## Use

One profile per member account that signs in:

```sh
caremark profiles add adult1
caremark auth login --browser --profile adult1   # sign in in the Chrome window it opens

caremark --profile adult1 patients list           # the people on the account, with their ids
caremark --profile adult1 rx list                  # prescriptions filled in the last year
caremark --profile adult1 costs --since 2026-01-01 # each fill's cost, and the sums
caremark --profile adult1 records export --patient <ID> --ndjson | medrec import --from caremark

# One read of the site, printed as the JSON it answers (docs/api.md lists them):
caremark --profile adult1 api POST member-family --data '{"data":{}}'
```

`records export` writes one person's Patient, Coverage, prescriptions
(MedicationRequest), fills (MedicationDispense) and pharmacy claims
(ExplanationOfBenefit, CARIN's Pharmacy EOB); `docs/fhir.md` maps every
field. Without `--since` it reads a year at a time back to the first year in
which no one on the account had a fill (at most 10). `-o <DIR>` writes the
stream to `<DIR>/records.ndjson` instead and prints `records-export/v1`.
`rx list` and `costs` read the last year unless `--since` says otherwise,
everyone's unless `--patient`.

`api` is read-only too: it sends a `GET`, the site's session check, or a
`POST` to one of the experience reads caremark knows (by name, id or path),
on `https://www.caremark.com` only. Any other method, host or experience is
exit 2 before a request is sent.

caremark reads the site one request at a time, a second apart. A session
that has expired is exit 3 naming the sign-in that renews it.

A command that reads the site names its profile: `--profile <NAME>` (or
`CAREMARK_PROFILE`). Without one it is exit 2. A profile must be
configured (`caremark profiles add <NAME>`); there is no default, and a
lone profile is never picked. `info`, `config`, `profiles`, `auth reset`,
`self-update` and `completions` need no sign-in and take no `--profile`.

Every command takes `--json` and prints one schema-tagged document.

| Command | Output schema |
| --- | --- |
| `patients list` | `patient-list/v1` |
| `rx list` | `rx-list/v1` |
| `costs` | `cost-list/v1`: `items` and `totals` (fills not reversed) |
| `records export` | `records-stream/v1` (`--ndjson`), or `records-export/v1` (`-o`) |
| `api` | the site's own JSON answer |
| `auth login` / `status` / `logout` / `reset` | `auth-login/v1` / `auth-status/v1` / `auth-logout/v1` / `auth-reset/v1` |
| `profiles list` / `add` / `remove` | `profile-list/v1` / `profile-add/v1` / `profile-remove/v1` |
| `config path` / `show` | `config-path/v1` / `config/v1` |
| `info` | `cli-info/v1`: profile `health-records/v1`; capabilities `login-profiles`, `patients`, `rx`, `costs`, `records`, `api`; `auth.required` true |

Exit codes: 0 ok, 2 usage, 3 sign-in not finished or session refused, 4
not found, 5 the member site's answer could not decide, 6 a destructive
command (`auth reset`, `profiles remove`) without a terminal to ask on and
without `--force`.

## Profiles and the member-site session

One session per profile: one per member account that signs in, named by
you (1 to 32 lowercase letters, digits, `-` or `_`) and kept in caremark's
config.

```sh
caremark profiles add adult1                       # once per account
caremark profiles list
caremark auth login --browser --profile adult1     # a Chrome window on caremark.com; sign in there
caremark auth login --browser --wait 60 --profile adult1   # wait up to an hour for the sign-in
caremark auth login --browser --fresh-profile --profile adult1   # a scratch browser profile, removed afterwards
pbpaste | caremark auth login --stdin --user-agent "<the browser's>" --profile adult1   # a Cookie header from DevTools
caremark auth status --profile adult1              # stored or not, and when; sends no request
caremark auth logout --profile adult1              # removes the session, then the browser profile
caremark profiles remove adult1                    # the session, the browser profile, then the profile
caremark auth reset                                # every profile's session and the vault's key
```

You type the sign-in in the Chrome window: caremark never fills or drives
the page (its bot check refuses scripted input). The window waits for the
site's own session check to say you are signed in, checking at most every
15 seconds, then stores the session and closes. A `--stdin` session is
checked the same way before it is stored, unless `--no-verify`
(`"verified": false` in `auth-login/v1`). With a session stored for the
profile, `auth login` refuses before Chrome opens unless `--overwrite`.

| What | Where | Removed by |
| --- | --- | --- |
| The session: the `Cookie` header, the browser's user agent and when it was captured, as one JSON entry `<profile>/session` | caremark's vault: one encrypted, owner-only (`0600`) file per entry under `~/.config/caremark/vault/`, sealed with one key, the keychain item `vault-key` in service `piekstra.caremark` | `caremark auth logout --profile <NAME>`, first; `caremark auth reset` removes every entry and the key |
| The Chrome profile `--browser` signs in through, one per profile, kept so the site remembers this device: the site's cookies (encrypted by Chrome), its site storage and the window's history | `~/Library/Application Support/piekstra.caremark/browser-profile-<profile>`, owner-only (`0700`); under `$XDG_DATA_HOME` on Linux | `caremark auth logout --profile <NAME>`, after the session |

The session never goes on a command line or into a log, and on disk only
encrypted. One keychain item for every profile means a new build of
caremark asks the keychain once, and a run with nothing stored reads no
item at all (cli-common DESIGN.md §2.5). `auth reset` is for a lost or
damaged key: it asks first, and without a terminal needs `--force`.

## Development

`make verify` runs fmt-check, clippy, the tests and the smoke test. Tests
are offline and use synthetic data only (`tests/fixtures/README.md`). See
`AGENTS.md` and `CONTRIBUTING.md`.

## License

MIT or Apache-2.0, at your option.
