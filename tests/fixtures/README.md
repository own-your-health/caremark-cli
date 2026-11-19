# Test fixtures

Every file here is **synthetic**. No value in them is a real member's, and no
real answer of the member site was copied into them: each is written by hand
after the *shape* of an answer (its keys, nesting, types, and null versus
absent), recorded with `examples/shape.rs`, which prints shapes and never
values.

| File | Answer of |
| --- | --- |
| `member-family.json` | `member-family`: the signed-in member (`getPatientV2`) and the family list |
| `claim-history.json` | `claim-history`: every person's claims; the test server answers one person's, cut to the request's dates |
| `financial-summary.json` | `financial-summary`: every person's costed fills, cut the same way |
| `env.js` | `/memberportal/env.js`, with dummy keys |

The household: Alex Example (the cardholder, person code 001), Sam Example
(spouse, 002) and Jordan Example (child, 003). Alex has two fills of
EXAMPLEPRIL, a reversed fill of SAMPLESTATIN and an older paid one; Sam has
one fill of TESTOCILLIN that only the financial summary lists.

## The dummies

`tests/no_personal_data.rs` checks every identity-bearing key, wherever it is
nested, against these:

| Key (any case) | Dummy |
| --- | --- |
| first name | `Alex`, `Sam` or `Jordan` |
| last name | `Example` |
| member, patient or full name | a dummy first name and `Example` |
| date of birth | a January first |
| member, subscriber or cardholder id | `SYNM` and six digits |
| group number | `SYNG` and four digits |
| Rx number, claim number, the site's ids | one digit repeated, zeros then one digit, or `SYN…` |
| prescriber, pharmacy or group name | a name with `Example`, `Sample` or `Test` in it |
| NPI, NCPDP or pharmacy id | one digit repeated, or zeros then one digit |
| phone | a 555 number |
| email | `@example.com` |

Drugs are named `EXAMPLEPRIL`, `SAMPLESTATIN` and the like, and their NDCs
are zeros then one digit. Amounts are round, invented figures.
