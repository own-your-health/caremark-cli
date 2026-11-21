# caremark

Unofficial; not affiliated with or endorsed by CVS Caremark or CVS Health.

`caremark` is for anyone with a caremark.com member account: a member of a
plan whose pharmacy benefit CVS Caremark manages, often an employer plan
whose medical insurer is another company. It reads that member's
prescriptions, fills, pharmacy claims and what each fill cost from the
member site, www.caremark.com, as the member sees them signed in. It writes
them as FHIR (`records-stream/v1`) for
[medrec](https://github.com/piekstra/medrec-cli) (`medrec import --from
caremark`) or any tool that reads FHIR, and as JSON lists. It is read-only:
nothing it does writes to Caremark.

No developer registration, API key or app is needed. No API serves an
employer plan's Caremark member, so caremark uses the member site with the
session you sign in to (`docs/api.md` says why each API route was ruled
out, and lists every read caremark makes).

## Quick start

```sh
cargo install --git https://github.com/piekstra/caremark-cli --locked
caremark profiles add me                            # one profile per caremark.com login
caremark auth login --browser --profile me          # a Chrome window opens: sign in there yourself
caremark --profile me patients list                 # the people on the account, with their ids
caremark --profile me patients label --patient <ID> alex   # optional: your own name for each person
caremark --profile me costs                         # last year's fills and what each cost
medrec import --from caremark --from-profile me --patient alex --person Alex   # into medrec's records
```

medrec runs `caremark records export` itself. Run it by hand
(`caremark --profile me records export --patient alex --ndjson`) to see
the stream.

### What needs a person

| Step | Why | How often |
| --- | --- | --- |
| Typing the sign-in in the Chrome window: username, password, and a code by text or email on a device the site does not know | The sign-in page runs a bot check (Akamai Bot Manager) that refuses scripted input, so caremark never fills or clicks the page | Each time the session has ended. The Chrome profile caremark keeps lets the site remember the device, so a later sign-in usually asks no code |
| Clicking "Always Allow" in the macOS keychain dialog | caremark's sessions are sealed with one key in the keychain, and macOS asks once for each newly built or upgraded binary | After an install or upgrade |

caremark.com goes down for maintenance at times, often overnight. A sign-in
cannot finish then, and a read is exit 5 saying the site is down for
maintenance. Try again later.

Everything else (lists, exports, imports) runs unattended while the
session lasts. The site decides how long that is, and caremark has not
measured it yet. A command run after it has ended is exit 3, naming
`caremark auth login --browser --profile <NAME> --overwrite`. Nothing is
read with an ended session, and nothing stored is lost.

### The site's terms

Caremark's Terms of Use (caremark.com, last updated 2026-10-16) say
nothing about scripts or automated access. They allow use of the site
for your own personal, non-commercial purposes, and forbid copying or
"systematically" downloading and storing its content beyond that.
caremark reads only the signed-in member's own records, one request a
second, for their own use. But keeping those records is storing the
site's content, which a strict reading of the terms forbids. Using
caremark means weighing that risk yourself. caremark does not get around
the bot check: you sign in yourself, in a real browser.

## Install

```sh
cargo install --git https://github.com/piekstra/caremark-cli --locked
# or download a release: caremark-<target>.tar.gz, then `caremark self-update` later
```

## Use

One profile per member account that signs in:

```sh
caremark profiles add me
caremark auth login --browser --profile me   # sign in in the Chrome window it opens

caremark --profile me patients list           # the people on the account, with their ids
caremark --profile me patients label --patient <ID> alex   # your own name for one of them
caremark --profile me rx list                  # prescriptions filled in the last year
caremark --profile me costs --since 2026-01-01 # each fill's cost, and the sums
caremark --profile me records export --patient alex --ndjson   # one person's records-stream/v1

# One read of the site, printed as the JSON it answers (docs/api.md lists them):
caremark --profile me api POST member-family --data '{"data":{}}'
```

`--patient` takes an id from `patients list`, a label, or a first name.
`patients label --patient <ID> <WHO>` gives one person a name of your own,
and `--clear` removes it. One sign-in sees the whole family, so this is
how each person gets one. A label is unique within the profile and never
a person's id. It is kept in caremark's config with the people
`patients list` has read, so the command sends no request. A person must
have been listed once before they can be labelled. An error that has to
name people names them by label, never by id.

`records export` writes one person's Patient, Coverage, prescriptions
(MedicationRequest), fills (MedicationDispense) and pharmacy claims
(ExplanationOfBenefit, CARIN's Pharmacy EOB); `docs/fhir.md` maps every
field. It reads a year at a time from today back to `--since` (at most 30
years back, on every command), or over the last 10 years without it, and
always runs to today: `rx list` and `costs`
take `--until` for a closed range. One of `--ndjson` (the stream on stdout)
and `-o <DIR>` is required; `-o` writes the stream to
`<DIR>/records.ndjson` and prints `records-export/v1`. A read that fails
partway ends the stream incomplete (`"complete": false` in its trailer,
`records.incomplete.ndjson` with `-o`) with the fills of the years read
before it, and the command exits with the failure's code, 3 or 5.
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
| `patients list` / `label` | `patient-list/v1` (`label` prints the one person) |
| `rx list` | `rx-list/v1`, with `total` and `left_out` as `costs` |
| `costs` | `cost-list/v1`: `items` (a `--limit` slice of `total` rows), `totals` over the range (fills not reversed): `fills`, `total_cost`, `plan_paid`, `you_paid`, `deductible`, `discount`, the names each item's amounts take, `left_out` (fills left out: `unmatched`, `undated`) |
| `records export` | `records-stream/v1` (`--ndjson`), or `records-export/v1` (`-o`) |
| `api` | the site's own JSON answer |
| `auth login` / `status` / `logout` / `reset` | `auth-login/v1` / `auth-status/v1` / `auth-logout/v1` / `auth-reset/v1` |
| `profiles list` / `add` / `remove` | `profile-list/v1` / `profile-add/v1` / `profile-remove/v1` |
| `config path` / `show` | `config-path/v1` / `config/v1` |
| `info` | `cli-info/v1`: profile `health-records/v1`; capabilities `profiles`, `patients`, `rx`, `costs`, `records`, `api`, `login-profiles`, `patient-labels`; `auth.required` true |

Exit codes: 0 ok, 2 usage, 3 sign-in not finished or session refused, 4
not found, 5 the member site's answer could not decide (an export's stream
that ended incomplete exits 3 or 5, as its trailer's error), 6 a destructive
command (`auth reset`, `profiles remove`) without a terminal to ask on and
without `--force`.

## Profiles and the member-site session

One session per profile: one per member account that signs in, named by
you (1 to 32 lowercase letters, digits, `-` or `_`) and kept in caremark's
config.

```sh
caremark profiles add me                       # once per account
caremark profiles list
caremark auth login --browser --profile me     # a Chrome window on caremark.com; sign in there
caremark auth login --browser --wait 60 --profile me   # wait up to an hour for the sign-in
caremark auth login --browser --fresh-profile --profile me   # a scratch browser profile, removed afterwards
pbpaste | caremark auth login --stdin --user-agent "<the browser's>" --profile me   # a Cookie header from DevTools
caremark auth status --profile me              # stored or not, and when; sends no request
caremark auth logout --profile me              # removes the session, then the browser profile
caremark profiles remove me                    # the session, the browser profile, then the profile
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
| The people `patients list` has read, per profile: each person's `Patient.id`, member role, whether they sign in, and the household's label (`patients label`); no name the site gives | caremark's config (`caremark config path`), owner-only (`0600`) | `caremark profiles remove <NAME>`, with the profile's settings |
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
