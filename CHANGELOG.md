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
