# The FHIR caremark writes

`records export` writes FHIR R4 by hand (`src/fhir.rs`), as
`records-stream/v1` with `method: "portal"`. The claims follow CARIN Blue
Button's Pharmacy EOB and match uhc-cli's and kaiser-cli's, so medrec reads
every insurer's claims the same way. Every resource carries
`meta.source: "caremark:member-site"`, the tag
`https://github.com/piekstra/caremark-cli/fhir/tag|mapped-from-member-site`,
and `meta.lastUpdated` (medical-common DESIGN.md §1.4). That is the
stream header's `pulled_at`, since the site shows no date of its own for
when a fill or a person last changed. The exception is a MedicationRequest,
dated by its latest fill (`YYYY-MM-DDT00:00:00Z`) and built from that fill
alone, so a narrower pull (a later `--since`) writes the same content under
the same date; a new fill makes it newer. Every fill in a stream brings its MedicationRequest,
whatever `--since` bounds, so the header's `since_unbounded` names only
Coverage, Organization and Patient.

One export is one person's: the stream's `patient` is that person's
`PatientContext`, as `patients list` prints it.

## Ids

| Resource | `id` | From |
| --- | --- | --- |
| `Organization` | `caremark` | constant |
| `Patient` | `p` and 12 hex digits | SHA-256 of the signed-in member's own id at the site (`getPatientV2.id`) and the person's person code (`model::patient_id`): person codes are numbered per account, so the code alone would give two accounts' cardholders one id |
| `Coverage` | `cov-<patient id>` | |
| `MedicationRequest` | `mr` and 16 hex digits | the patient id and the prescription number |
| `MedicationDispense`, `ExplanationOfBenefit` | `rx` and 16 hex digits, the same for a fill's two | the claim number; else the prescription number, fill number and date; else the date and the drug (`model::fill_id`). A second claim of one read with the same id and no claim number (a split fill) gets its own (`model::nth_fill_id`) |

Every id is a SHA-256 digest of its parts, each written as its length in
bytes and `:` before it (medical-common DESIGN.md §1.3), so no two sets of
parts hash alike.

The ids carry no name, member id or prescription number, and they are the
same on every pull, so a re-export replaces rather than duplicates. They
are fixed from v0.1.0, which is released only after a signed-in read has
confirmed the answer paths they are built from (the claim number above
all). A later change to them is a breaking change, named in its release. They
are keyed on the account signed in: the family read shows no identifier
that every sign-in on an account shares, so a person seen from two sign-ins
(a spouse with a login of their own on the cardholder's account) gets one
id per sign-in. medrec then files one source's two patients, which a hand
merge can join, never one patient for two people (DESIGN.md §1.3). If the
first signed-in read shows an account-level identifier, the ids move to it
before v0.1.0. A family answer without the member's own id is exit 5
rather than ids that could collide.

A MedicationRequest is built from its prescription's latest fill alone:
the drug, the prescriber and the refills authorized. Nothing in it depends
on which older fills a pull's range reached, so a narrow pull and a full
one write the same request under the same date.

## Resources

| Resource | Field | From the site |
| --- | --- | --- |
| `Organization` | `name` `CVS Caremark`, `type` `pay` | |
| `Patient` | `name` (`text`, `family`, `given`), `birthDate` | the family list's `firstName`, `lastName`, `dateOfBirth` |
| `Coverage` | `status` `active`, `type` v3-ActCode `DRUGPOL`, `beneficiary`, `payor` Caremark, `relationship` `self` for the cardholder | `relationship: CARDHOLDER` |
| `MedicationRequest` | `identifier` (the Rx number), `status` (`unknown` unless the site says), `intent` `order`, `medicationCodeableConcept`, `subject`, `requester` (name and NPI), `dispenseRequest.numberOfRepeatsAllowed` | the latest fill of one Rx number: its drug, its prescriber, its refills authorized |
| `MedicationDispense` | `identifier` (Rx number; claim number with C4BB type `uc`) | `rxNumber`, `rxclaimNumber` |
| | `status`: `completed`, or `cancelled` for a reversed claim | `claimStatus` |
| | `medicationCodeableConcept`: RxNorm when known, then the NDC (11 digits, `http://hl7.org/fhir/sid/ndc`), then the GPI; `text` the site's label | `drug.ndcId`, `drug.genericProductId`, `drug.name` + `formStrengthName` + `formName` |
| | `performer[0].actor`: the pharmacy's name, with its NPI (or NCPDP id) | `pharmacy` |
| | `authorizingPrescription`: the MedicationRequest; for a fill with a prescriber and no prescription number, a contained MedicationRequest `#rx` whose `requester` is the prescriber (medical-common DESIGN.md §2.1) | |
| | `type`: v3-ActCode `FF` (first fill) or `RF` (refill) | `fillNumber` 0 or more |
| | `quantity`, `daysSupply` (UCUM `d`) | `dispensedQuantity`, `daysSupplyQuantity` |
| | `dosageInstruction[0].text`: the directions on the label, left out when the site gives none | `sig`, `directions` or `dosageInstructions` (names from the site's scripts, unconfirmed) |
| | `whenHandedOver`: the claim's date of service, as Da Vinci PDex maps it | `fillDate` |
| `ExplanationOfBenefit` | `type` claim-type `pharmacy`, `use` `claim`, `status` `active` or `cancelled` | |
| | `billablePeriod` and `created`: the fill date (`_created` data-absent-reason when there is none) | |
| | `insurer` Caremark; `provider` the pharmacy (data-absent-reason when unnamed) | |
| | `supportingInfo`: C4BBSupportingInfoType `dayssupply`, `refillnum`, `refillsauthorized` (each `valueQuantity.value`), `brandgenericindicator` (`code`, NCPDP Brand Generic Indicator: `1` brand, `2` generic) | `daysSupplyQuantity`, `fillNumber`, refills authorized, `brandGenericIndicator` |
| | `item[0]`: `productOrService` (as the dispense's medication), `servicedDate`, `quantity`, `net` (total cost) | |
| | `item[0].adjudication` and `total`, one entry per amount the site gave (none is written as zero) | see below |

### Amounts

| Category | Code system | From |
| --- | --- | --- |
| `eligible` | `http://terminology.hl7.org/CodeSystem/adjudication` | the prescription's total cost (`totalRxCost`) |
| `discount` | C4BBAdjudication | manufacturer discounts and coupons (`manufacturerDiscountAmount` + `drugCouponInfo.patientSavings`) |
| `benefit` | adjudication | what the plan paid (`planPaidAmount`) |
| `deductible` | adjudication | `deductibleAmount` |
| `copay` | adjudication | `patientCopayAmount` |
| `coinsurance` | C4BBAdjudication | `coinsuranceAmount` |
| `memberliability` | C4BBAdjudication | what the member paid (`patientPayAmount`) |

A claim that gives no amounts takes them from the financial summary's
fill of the same day and drug, and the same prescription number (leading
zeros aside) when both give one (`portal::add_costs`). A fill only the
financial summary lists is left out: it may be a claim the two reads write
differently, and a second id for one dispense would stay in medrec. The
trailer counts them in one skip, code `unmatched`, resource type
`MedicationDispense`, and `costs` and `rx list` say so on stderr.

A claim with no date caremark reads is left out too, since no `--since`
can hold it, and counted in a skip with code `undated`. When every claim
read is undated, the answer's shape has changed rather than the account:
the export ends incomplete and `costs` and `rx list` fail, exit 5.

## What medrec reads

medrec (own-your-health/medrec-cli#17) reads a `MedicationDispense`'s
`medicationCodeableConcept` (RxNorm, else NDC), `whenHandedOver`, `status`,
`quantity`, `daysSupply.value`, `dosageInstruction[0].text`, `type` and
`performer[0].actor.display`; and a pharmacy `ExplanationOfBenefit`'s
`provider`, `insurer`, `total`, per-item adjudication, `supportingInfo`
(`dayssupply`, `refillnum`, `refillsauthorized`, `brandgenericindicator`)
and `item[].quantity`. Two sources' dispenses of one fill are one record
when the drug code and the handover date match.
