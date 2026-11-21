# Security

Report a vulnerability privately through GitHub's "Report a vulnerability"
(Security tab), not in an issue.

## What caremark touches

- Contacts no server except GitHub, for `self-update`, and
  `https://www.caremark.com`: for `auth login`, a Chrome window the owner
  signs in through and the site's own session check, at most every 15
  seconds while the owner signs in; for a command run with `--profile`, the
  member's own reads. Every request is a `GET`, the session check, or a
  `POST` to one of the experience reads listed in `src/client.rs`
  (`EXPERIENCES`), one at a time and at least a second apart, with a
  30-second limit and no redirect followed. Every URL is built from that
  fixed origin. `caremark api` is held to the same, checked before anything
  is sent.
- Reads the site's public client keys from `/memberportal/env.js`, the
  same for every visitor, at run time, and sends them only to the site.
  They are never stored or printed.
- A cookie the site changes is written back to the profile's session once
  the command has succeeded, unless a newer sign-in replaced the stored
  session meanwhile. A failed write-back is a warning on stderr, not an
  error.
- Stores one credential per profile, its member-site session: the
  `Cookie` header, the browser's user agent and the capture time, as the
  vault entry `<profile>/session`. The vault (`pk_cli_auth::vault`) is
  encrypted owner-only files under `~/.config/caremark/vault/`, with one key
  in the keychain (service `piekstra.caremark`, account `vault-key`). A
  session never goes on a command line or into a log.
- Keeps one Chrome profile per caremark profile for `auth login --browser`,
  so the site remembers the device:
  `~/Library/Application Support/piekstra.caremark/browser-profile-<profile>`
  (under `$XDG_DATA_HOME` on Linux), owner-only (`0700`, a launch is refused
  otherwise). It holds the site's cookies, which Chrome encrypts, and its
  site storage and the window's history, which only the mode protects.
  `--fresh-profile` signs in without it.
- `auth logout --profile <NAME>` and `profiles remove <NAME>` remove that
  profile's session, then its Chrome profile. `auth reset` removes every
  session and the vault key.
- The config file (`caremark config path`, owner-only, `0600`) holds the
  profile names and, per profile, every person the family read has
  listed: their `Patient.id`, member role, whether they are the member
  who signs in, and the household's label once `patients label` set one.
  It holds no name the site gives. A `Patient.id` is a digest of short
  identifiers, which trying every value recovers, and a label is often a
  first name, so the file is as private as the records: do not sync,
  share or commit it. A person the latest family read did not list stays,
  marked `unlisted`, until they are listed again or `profiles remove
  <NAME>` deletes the profile's entries with its settings. Their label
  moves to anyone still listed who is given it.

## What caremark never does

- Writes to Caremark: no refill, order, transfer, message or setting.
- Fills or drives the sign-in page. The owner types the sign-in in the
  Chrome window; caremark reads cookies at the browser level only.
- Sends a record or its contents anywhere: the output goes to stdout.
