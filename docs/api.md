# CVS Caremark: how prescription fills and pharmacy claims are reached

Caremark is the pharmacy benefit manager (PBM) of many employer plans
whose medical insurer is another company. Its pharmacy claims do not
appear in the medical insurer's Explanations of Benefits, so for such a
member Caremark is the only source of fill history and pharmacy costs.

## Route

The member site, www.caremark.com, signed in in a Chrome window per
profile (`auth login --browser`). No API serves an employer plan's
Caremark member:

- **CMS-9115-F's Patient Access API** binds Medicare Advantage
  organizations, Medicaid and CHIP programs and plans, and qualified health
  plan issuers on the federal exchanges (from January 1, 2021). An
  employer-sponsored group plan is none of these, so its PBM has no
  obligation to offer one.
- **Aetna's developer portal** (developerportal.aetna.com, read
  2026-11-19) lists Aetna's interoperability APIs: Patient Access,
  Payer-to-Payer, Prior Authorization, Provider Access, Provider Directory
  and Realtime Pharmacy Benefit Check, with payer-to-payer onboarding
  moving to 1upHealth in December 2026. They are Aetna's, the insurer's,
  for Aetna's own members. A member whose medical insurer is not Aetna
  has no Aetna claims there, and a third-party aggregator is ruled out by
  the household's rules besides.
- **CVS Health's interoperability page** (cvshealth.com/interoperability)
  answered with an error page on 2026-11-19.

The household chose the member site, accepting the risk under its Terms of
Use (decision aceea06c).

### Terms of Use

Caremark's terms (caremark.com, last updated 2026-10-16) have no clause
naming scrapers, robots or automated access. The nearest is the
content-use paragraph:

> You hereby agree not to reproduce, duplicate, copy, sell, resell,
> decompile, disassemble, or exploit for any purpose other than for your
> personal, non-commercial use the Services (including any portion
> thereof) or the use (including any access to) thereof, or to collect any
> information about visitors or users of the Services, or otherwise
> systematically download and store Services content.

caremark reads the member's own records for the member's personal,
non-commercial use, one request at a time. Keeping them in the
household's records is storing Services content, which is the risk
aceea06c accepted.

## Member site

### Sign-in

The sign-in page (`/memberportal/...`, a Next.js app) runs Akamai Bot
Manager. caremark never fills or drives it:

| Tried, 2026-11-19 | Result |
| --- | --- |
| Headless Chromium (Playwright), username and password typed into the fields | "We can't sign you in right now"; the experience API answered 403 |
| Real Chrome, attached over CDP and driven by Playwright | the same |

So the owner types the sign-in in a plain Chrome window, started with no
automation flags (`pk-cli-browser`: only `--remote-debugging-port=0`,
`--user-data-dir`, `--no-first-run`, `--no-default-browser-check`).
caremark reads the cookie jar and the user agent at the browser level,
which no page can observe, and evaluates nothing in the site's pages.
Signing in goes through the username, the password, and a code by email
or text when the device is new. "Keep me signed in" sets `kmsi_yn` and
`KMSI_ID`, and the kept browser profile is then remembered.

### Public keys

`GET /memberportal/env.js`, served to anyone, assigns
`window.apiConfig = {...}` with the site's client keys. caremark reads two
at run time (`site::SiteKeys`) and never stores or prints them:

| Key | Sent as |
| --- | --- |
| `experienceApiKey` | `x-api-key` on every experience read |
| `introspectApiKey` | `Authorization: Basic <key>` (already base64) on the session check |

### The session

The session is the whole `caremark.com` cookie jar; `CVS_ONESITE_TOKEN`
(domain `.caremark.com`) carries the sign-in. The site sets visitor
cookies before anyone signs in, so a cookie in the jar proves nothing.

**Session check**: `POST /api/oauth2/v2/introspect` with the jar and the
introspection key. A signed-in member gets
`{"statusCode": "0000", "statusDescription": ..., "active": true, ...}`;
anything else is not signed in (`site::judge`). The site's own pages call
it to learn whether they are signed in. During `auth login --browser`
caremark calls it at most every 15 seconds (`site::CHECK_EVERY`).

The pages renew the token with `POST {refreshTokenApi.url}/<experience>`
(`x-clientId: core_access`, `x-route: account-gke-rke`, body
`{"data":{"inputReq":{"lob":"pbm"}}}`). caremark does not.

### Experience API

The site's data comes from one endpoint shape, a `POST` of
`{"data": {...}}` to

```
/api/client/experience/v2/load/<experience id>
```

with headers `x-api-key` (the experience key), `x-experienceId` (the same
id), `x-clientId` (the experience's client), `x-channel-type: WEB`,
`Accept` and `Content-Type: application/json`, and any of `x-route`,
`x-appName` and `x-source-type` the page sends. Ids, clients and bodies
are constants of the site's public JavaScript
(`/pharmacy/benefits/caremark-benefits-summary/`, `.../caremark-benefits-claims/`,
`/pharmacy/rx/`). The member is the session's: no body names a member
id. Answers carry their own `statusCode`, `"0000"` for success.

The experiences caremark reads (`client::EXPERIENCES`), each a read:

| Name | Experience id | Client | Other headers | Page and call | Body `data` | Answer |
| --- | --- | --- | --- | --- | --- | --- |
| `financial-summary` | `13ed331c-3e9c-4fdf-bab2-6396a12a3623` | `benefits` | `x-source-type: pbm` | Financial summary, `getFinancialSummaryExperience` | `startDate`, `endDate` (`YYYY-MM-DD`), `idType: "PBM_QL_PARTICIPANT_ID_TYPE"`, `profileId: ""` | `data.getFinancialSummary`: `statusCode`, `patients[]` (`id`, `patient` with `firstName`, `lastName`, `dateOfBirth`, `stcob`, `linkedIDs[].prescriptionforPatient[]` with `id`, `quantity`, `drugInfo.drug` (`name`, `formStrengthName`, `formName`) and `fills[]` (`dateTimeFilled`, `pharmacyName`, `patientPayAmount`, `totalRxCost`, `planPaidAmount`, `deductibleAmount`, `finalOOPAmtAdjusted`, `primaryPlanPayAmount`, `secondaryPlanPayAmount`, `manufacturerDiscountAmount`, `otherAdjustments`, `hRAAppliedAmount`, `drugCouponInfo.patientSavings`)), `planDetails` |
| `claim-history` | `19378b96-0bc0-43c1-a37c-9308e71a22f1` | `benefits` | | Check drug cost, `getClaimHistoryExperience` | `idType: "PBM_QL_PARTICIPANT_ID_TYPE"`, `personCode`, `allClaims: true`, `dedupRxClaims: true`, `claimStartDate`, `claimEndDate` | `data.getClaimV2`: `statusCode`, `memberClaimList[].claims[]` with `drug` (`name`, `formStrengthName`, `formName`) and `daysSupplyQuantity` |
| `member-family` | `e496ecd3-4067-4499-a1cd-27e353397cb4` | `client_engagement` | `x-route: PHARMACY-GKE-RKE`, `x-appName: cmk-notifications` | Dashboard, `getMemberFamily` | `{}` | `data.getFamilyInfo.familyMemberList[]` |
| `member-info` | `656e55d9-691d-4b48-b5fa-cefa54284336` | `client_engagement` | `x-route: CMK-PHARMACY-HA`, `x-appName: cmk-dashboard` | Dashboard, `getMemberInfoExp` | `idType`, `idType2`: `"PBM_QL_PARTICIPANT_ID_TYPE"` | member and coverage details (`coverageEffectiveDate`) |
| `member-profile` | `53e50e5a-6c39-40a7-9676-55c4f2395600` | `client_engagement` | `x-route: CMK-PHARMACY-HA` | `getMemberProfile` | `startDate: ""`, `endDate: ""`, `cardholderInternalID: ""` | the member profile |
| `plan-deductible` | `79d00edc-3b6d-46bc-b9e6-54ea048d9513` | `benefits` | | Plan summary, `getPlanSummaryDeductibleInfo` | `idType: "PBM_QL_PARTICIPANT_ID_TYPE"` | deductible and out-of-pocket |
| `prescriptions-list` | `3386858f-7d5e-41ab-ab18-f46fa4205c93` | `benefits` | | Home, `getPrescriptionsList` | | the home page's prescriptions |

The family-info answer the pages keep has
`getPatientV2` (`firstName`, `lastName`, `personCode`, `id`,
`dateOfBirth`), `getFamilyInfo.familyMemberList[]` and
`getPlanDetailsList.planDetailsList[]`; `personCode` is what
`claim-history` takes.

### What caremark reads in the answers

`src/portal.rs` names every path. The family, the claims and the
financial summary are read as the site's pages read them:

| Read | Paths |
| --- | --- |
| `member-family` | `data.getPatientV2` (the signed-in member) and `data.getFamilyInfo.familyMemberList[]`: `personCode`, `firstName`, `lastName`, `dateOfBirth`, `relationship` (`CARDHOLDER`, `SPOUSE`, `CHILD`) |
| `claim-history`, per person code and year | `data.getClaimV2.statusCode`; `memberClaimList[].claims[]`: `rxNumber`, `fillNumber`, `rxclaimNumber`, `claimStatus`, `fillDate`, `daysSupplyQuantity`, `dispensedQuantity`, `drug` (`name`, `formStrengthName`, `formName`, `ndcId`, `genericProductId`), `pharmacy` (`pharmacyName`, `pharmacyNPI`), `prescriber` (`prescriberFirstName`, `prescriberLastName`, `prescriberNPI`), the amounts |
| `financial-summary`, per year | `data.getFinancialSummary.statusCode`; `patients[]`: `patient.personCode`, `linkedIDs[].prescriptionforPatient[]` with `drugInfo.drug` and `fills[]` (`dateTimeFilled`, `pharmacyName`, the amounts) |

The claim fields come from the claim-history and drug-cost pages' own
code. Each read is a year (`claimStartDate`/`claimEndDate`,
`startDate`/`endDate`), as the claims page asks for one, from today back:
to `--since`, or 10 years for an export without it. Every year is read,
since a year with no fills says nothing of the years before it. Per year,
the financial summary is one read and the claims one read per person.

A claim's fill is matched to the financial summary's by the day and the
drug, and the prescription number when both give one, for the amounts a
claim leaves out. A financial-summary fill no claim matches is left out
and counted (`docs/fhir.md`, Amounts). Until a signed-in answer confirms
the two reads write a fill alike (the date as a calendar day, the
prescription number's leading zeros), expect some to be counted.

The same scripts hold experiences that write (refills, orders, carts,
transfers, notification marks, reimbursement claims, prior-authorization
requests, preferences). caremark lists none of them, and `caremark api`
refuses a `POST` to any experience not in the table.

## Traps

- **The bot check refuses scripted sign-ins**, headless or not, and its
  answer is a friendly error page, not a status: "We can't sign you in
  right now". It is not a wrong password. Do not retry with a driven
  browser; the owner signs in by hand.
- **A cookie is not a session.** The jar fills with visitor cookies before
  sign-in. Only the session check decides.
- **`200` does not mean success.** The experience API answers errors in
  the body's `statusCode`; `"0000"` is success.
- **The keys are in env.js, not in the scripts.** They are public and the
  same for every visitor, but they change with the site's releases:
  caremark reads them at run time, never from a constant.
- **`--log-requests` hides ids** in logged paths, except the experience id
  of `/api/client/experience/v2/load/<id>`, a constant that names the read.
