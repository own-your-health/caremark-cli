# Changelog

## Unreleased

v0.1.0 will be the first release. Breaking against `main` before it:

- `--patient` takes a person's id from `patients list`, or the label
  `patients label` gave them, and never a first name. A name typed on the
  command line stays in shell history and in any agent's transcript, and
  the label stands in for it (medical-common DESIGN §1). Give each person a
  label once, then pass it. The exit 2 for a word that was one person's
  label and another's first name is gone with the fallback.
- Giving a label held by a person the latest family read did not list is
  exit 2, as for anyone else's label, and no longer moves it. The remedy
  is the `patients label --patient <LABEL> --clear` that frees it.

Also:

- The repository moved to own-your-health/caremark-cli, and medical-common
  and medrec-cli with it. `self-update` reads releases there, the
  medical-common pin and `cargo install --git` name the new owner. The
  FHIR tag system and the keychain service keep their names, so records
  and sign-ins already stored still match. GitHub redirects the old URLs.
- Pins medical-common v0.6.10 with cli-common v0.33.5, the pair
  medical-common lists. A `--json` error carries `remedy` (and `reason`, where it has one)
  apart from the message (cli-common v0.33.4), so a script reads the next
  step without parsing the text.
