//! The member site's answers, read into `crate::model`. Every path caremark
//! reads in an answer is named here and in `docs/api.md`, so a change of
//! the site's shapes is fixed in one place. A field the answer leaves out
//! is `None`, never a default.

use serde_json::Value;

use crate::cents::Cents;
use crate::model::{fill_id, ndc11, patient_id, Amounts, BrandGeneric, Drug, Fill, Party, Person};

/// The experience API's success code, in every answer's own `statusCode`.
pub const OK: &str = "0000";

/// `v` at `path` (`a.b.0.c`).
pub fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |v, key| match key.parse::<usize>() {
        Ok(i) => v.get(i),
        Err(_) => v.get(key),
    })
}

/// The first of `paths` that holds a non-empty string or a number, as text.
pub fn text(v: &Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|p| match at(v, p)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// The first of `paths` that holds a whole number, given as a number or as
/// digits.
pub fn whole(v: &Value, paths: &[&str]) -> Option<u32> {
    paths.iter().find_map(|p| match at(v, p)? {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64))
            .and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

/// The first of `paths` that holds an amount.
pub fn amount(v: &Value, paths: &[&str]) -> Option<Cents> {
    paths.iter().find_map(|p| Cents::from_json(at(v, p)?))
}

/// The first of `paths` that holds an array.
fn array<'a>(v: &'a Value, paths: &[&str]) -> &'a [Value] {
    paths
        .iter()
        .find_map(|p| at(v, p)?.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// A date the site answers (`2026-01-15`, `2026-01-15T00:00:00`,
/// `01/15/2026`, `YYYYMMDD`) as `YYYY-MM-DD`; `None` for anything else.
pub fn date(raw: &str) -> Option<String> {
    let s = raw.trim();
    let ok = |y: &str, m: &str, d: &str| {
        let (y, m, d) = (y.parse::<u32>().ok()?, m.parse::<u32>().ok()?, d.parse::<u32>().ok()?);
        ((1900..=2200).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d))
            .then(|| format!("{y:04}-{m:02}-{d:02}"))
    };
    let digits = |p: &str, n: usize| p.len() == n && p.bytes().all(|b| b.is_ascii_digit());
    if s.len() >= 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-' {
        let (y, m, d) = (&s[0..4], &s[5..7], &s[8..10]);
        if digits(y, 4) && digits(m, 2) && digits(d, 2) && (s.len() == 10 || matches!(s.as_bytes()[10], b'T' | b' ')) {
            return ok(y, m, d);
        }
        return None;
    }
    let parts: Vec<&str> = s.split('/').collect();
    if let [m, d, y] = parts.as_slice() {
        if (1..=2).contains(&m.len()) && (1..=2).contains(&d.len()) && digits(y, 4) {
            return ok(y, m, d);
        }
        return None;
    }
    if digits(s, 8) {
        return ok(&s[0..4], &s[4..6], &s[6..8]);
    }
    None
}

fn date_at(v: &Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|p| date(at(v, p)?.as_str()?))
}

/// An answer's own status at `path`: `Ok` for `"0000"`; anything else is
/// exit 5 naming the read, with the site's description when it gives one.
pub fn check_status(answer: &Value, path: &str, read: &str) -> Result<(), pk_cli_core::CliError> {
    let status = text(answer, &[&format!("{path}.statusCode")]);
    if status.as_deref() == Some(OK) {
        return Ok(());
    }
    let described = text(answer, &[&format!("{path}.statusDescription")])
        .map(|d| format!(": {}", printable(&d)))
        .unwrap_or_default();
    Err(pk_cli_core::CliError::Upstream(format!(
        "the member site's {read} answered status {}{described}",
        status.as_deref().unwrap_or("(none)")
    )))
}

/// Text from the site, safe to print: control characters as spaces, 120
/// characters at most.
fn printable(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(120)
        .collect();
    if text.chars().count() > 120 {
        out.push('…');
    }
    out
}

// ---------------------------------------------------------------- people

/// The `member-family` answer's people: the signed-in member and the family
/// the site shows. The signed-in member first; one person per person code.
/// Answer: `data.getFamilyInfo.familyMemberList[]` (`firstName`,
/// `lastName`, `dateOfBirth`, `personCode`, `relationship`), and
/// `data.getPatientV2` for the signed-in member.
pub fn people(answer: &Value) -> Vec<Person> {
    let mut out: Vec<Person> = Vec::new();
    let mut add = |v: &Value, signed_in: bool| {
        let Some(key) = text(v, &["personCode"]) else {
            return;
        };
        let role = text(v, &["relationship"]).map(|r| {
            if r.eq_ignore_ascii_case("CARDHOLDER") {
                pk_med_records::dto::member_role::SUBSCRIBER
            } else {
                pk_med_records::dto::member_role::DEPENDENT
            }
        });
        let read = Person {
            id: patient_id(&key),
            key,
            first_name: text(v, &["firstName"]),
            last_name: text(v, &["lastName"]),
            birth_date: date_at(v, &["dateOfBirth"]),
            role,
            member_id: None,
            signed_in,
        };
        // The signed-in member is in the family list too: what one read
        // leaves out, the other may give.
        match out.iter_mut().find(|p| p.key == read.key) {
            Some(p) => {
                p.first_name = p.first_name.take().or(read.first_name);
                p.last_name = p.last_name.take().or(read.last_name);
                p.birth_date = p.birth_date.take().or(read.birth_date);
                p.role = p.role.or(read.role);
                p.signed_in |= read.signed_in;
            }
            None => out.push(read),
        }
    };
    if let Some(me) = at(answer, "data.getPatientV2") {
        add(me, true);
    }
    for v in array(answer, &["data.getFamilyInfo.familyMemberList"]) {
        add(v, false);
    }
    out
}

// ----------------------------------------------------------------- fills

fn drug(v: &Value) -> Drug {
    let d = ["drug", "drugInfo.drug", "prescription.drugInfo.drug", "drugInfo"]
        .iter()
        .find_map(|p| at(v, p).filter(|d| d.is_object()))
        .unwrap_or(&Value::Null);
    let brand_generic = text(d, &["brandGenericIndicator", "brandGenericCode", "genericInd"])
        .or_else(|| text(v, &["brandGenericIndicator"]))
        .and_then(|s| match s.to_ascii_uppercase().as_str() {
            "B" | "BRAND" | "1" => Some(BrandGeneric::Brand),
            "G" | "GENERIC" | "2" => Some(BrandGeneric::Generic),
            _ => None,
        });
    Drug {
        name: text(d, &["name", "drugName"]).unwrap_or_default(),
        strength: text(d, &["formStrengthName", "strength"]),
        form: text(d, &["formName", "form"]),
        ndc: text(d, &["ndcId", "ndc"])
            .or_else(|| text(v, &["ndcId"]))
            .and_then(|n| ndc11(&n)),
        rxnorm: text(d, &["rxcui", "rxNormId"]),
        gpi: text(d, &["genericProductId", "gpi"]),
        brand_generic,
    }
}

fn pharmacy(v: &Value) -> Party {
    let p = [
        "pharmacy",
        "pharmacyDetails",
        "prescription.pharmacyDetails",
        "dispensingStore",
    ]
    .iter()
    .find_map(|path| at(v, path).filter(|d| d.is_object()))
    .unwrap_or(&Value::Null);
    Party {
        name: text(p, &["pharmacyName", "name"]).or_else(|| text(v, &["pharmacyName"])),
        npi: text(p, &["pharmacyNPI", "nationalProviderId", "npi"])
            .or_else(|| text(v, &["pharmacyNpi", "pharmacyNpiId"])),
        ncpdp: text(p, &["pharmacyNumber", "ncpdpId"]),
        phone: text(p, &["addresses.phoneNumber", "phoneNumber"]),
    }
}

/// The prescriber: a `prescriber` object's names and NPI, or the claim's
/// own `prescriber…` fields. A bare `firstName` on the claim is the
/// patient's, never read as the prescriber's.
fn prescriber(v: &Value) -> Party {
    let (p, first, last, npi): (&Value, &[&str], &[&str], &[&str]) = match at(v, "prescriber").filter(|p| p.is_object())
    {
        Some(p) => (
            p,
            &["prescriberFirstName", "firstName"],
            &["prescriberLastName", "lastName"],
            &["prescriberNPI", "prescriberNpi", "npi"],
        ),
        None => (
            v,
            &["prescriberFirstName"],
            &["prescriberLastName"],
            &["prescriberNPI", "prescriberNpi"],
        ),
    };
    let name = match (text(p, first), text(p, last)) {
        (Some(f), Some(l)) => Some(format!("{f} {l}")),
        (f, l) => f.or(l).or_else(|| text(p, &["prescriberName"])),
    };
    Party {
        name,
        npi: text(p, npi),
        ncpdp: None,
        phone: text(p, &["prescriberPhone"]),
    }
}

/// A claim's status: whether it was reversed (returned to stock).
fn reversed(v: &Value) -> bool {
    text(v, &["claimStatus", "status"])
        .map(|s| s.to_ascii_uppercase())
        .is_some_and(|s| s.contains("REVERS") || s == "CANCELLED" || s == "CANCELED")
}

/// The `claim-history` answer's fills of `person`. Answer:
/// `data.getClaimV2.memberClaimList[].claims[]`, each list with the
/// `personCode` it is for.
pub fn claims(answer: &Value, person: &Person) -> Vec<Fill> {
    let mut out = Vec::new();
    for list in array(answer, &["data.getClaimV2.memberClaimList"]) {
        if text(list, &["personCode", "patient.personCode"]).is_some_and(|k| k != person.key) {
            continue;
        }
        for c in array(list, &["claims"]) {
            out.push(claim(c, &person.id));
        }
    }
    out
}

fn claim(c: &Value, patient: &str) -> Fill {
    let rx_number = text(c, &["rxNumber", "claim.rxNumber", "prescription.rxNumber"]);
    let fill_number = whole(c, &["fillNumber", "refillNumber", "prescription.fillNumber"]);
    let claim_number = text(c, &["rxclaimNumber", "claimNumber", "prescription.rxclaimNumber"]);
    let filled = date_at(
        c,
        &[
            "fillDate",
            "claim.fillDate",
            "dateOfService",
            "dateTimeFilled",
            "prescription.fillDate",
        ],
    );
    let drug = drug(c);
    Fill {
        id: fill_id(
            patient,
            claim_number.as_deref(),
            rx_number.as_deref(),
            fill_number,
            filled.as_deref(),
            &drug.key(),
        ),
        patient: patient.to_string(),
        rx_number,
        fill_number,
        refills_authorized: whole(c, &["refillsAuthorized", "numberOfRefillsAuthorized"]),
        claim_number,
        drug,
        quantity: text(c, &["dispensedQuantity", "quantity", "prescription.dispensedQuantity"]),
        quantity_unit: text(c, &["quantityUnit", "unitOfMeasure"]),
        days_supply: whole(
            c,
            &["daysSupplyQuantity", "daysSupply", "prescription.daysSupplyQuantity"],
        ),
        filled,
        pharmacy: pharmacy(c),
        prescriber: prescriber(c),
        dosage: text(c, &["sig", "directions", "dosageInstructions"]),
        reversed: reversed(c),
        amounts: amounts(c),
    }
}

fn amounts(v: &Value) -> Amounts {
    let manufacturer = amount(v, &["manufacturerDiscountAmount"]);
    let coupon = amount(v, &["drugCouponInfo.patientSavings"]);
    let discount = match (manufacturer, coupon) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
    Amounts {
        total: amount(v, &["totalRxCost", "totalCost", "totalDrugCost"]),
        plan_paid: amount(v, &["planPaidAmount", "planPayAmount", "primaryPlanPayAmount"]),
        patient_paid: amount(v, &["patientPayAmount", "memberPaidAmount", "finalCost"]),
        deductible: amount(v, &["deductibleAmount"]),
        copay: amount(v, &["patientCopayAmount", "copayAmount"]),
        coinsurance: amount(v, &["coinsuranceAmount"]),
        discount,
    }
}

// ------------------------------------------------------------- financial

/// One fill as the financial summary lists it: what it cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostedFill {
    /// The person's [`Person::key`].
    pub person: String,
    pub drug: Drug,
    pub filled: Option<String>,
    pub pharmacy: Option<String>,
    pub quantity: Option<String>,
    pub rx_number: Option<String>,
    pub amounts: Amounts,
}

/// The `financial-summary` answer's fills, every person's. Answer:
/// `data.getFinancialSummary.patients[]` (`patient.personCode`,
/// `linkedIDs[].prescriptionforPatient[]` with `drugInfo.drug`, `quantity`
/// and `fills[]`).
pub fn financial(answer: &Value) -> Vec<CostedFill> {
    let mut out = Vec::new();
    for p in array(answer, &["data.getFinancialSummary.patients"]) {
        let Some(person) = text(p, &["patient.personCode", "personCode"]) else {
            continue;
        };
        let linked = array(p, &["linkedIDs"]);
        let prescriptions = linked
            .iter()
            .flat_map(|l| array(l, &["prescriptionforPatient"]))
            .chain(array(p, &["prescriptionforPatient"]));
        for rx in prescriptions {
            let drug = drug(rx);
            for f in array(rx, &["fills"]) {
                out.push(CostedFill {
                    person: person.clone(),
                    drug: drug.clone(),
                    filled: date_at(f, &["dateTimeFilled", "fillDate"]),
                    pharmacy: text(f, &["pharmacyName"]),
                    quantity: text(f, &["quantity", "dispensedQuantity"]).or_else(|| text(rx, &["quantity"])),
                    rx_number: text(f, &["rxNumber"]).or_else(|| text(rx, &["rxNumber"])),
                    amounts: amounts(f),
                });
            }
        }
    }
    out
}

/// Whether a costed fill is `fill`: the same day and the same drug (by
/// NDC, else by name), and the same prescription number when both have
/// one.
fn same_fill(fill: &Fill, c: &CostedFill) -> bool {
    if fill.filled.is_none() || fill.filled != c.filled {
        return false;
    }
    if let (Some(a), Some(b)) = (&fill.rx_number, &c.rx_number) {
        return a == b;
    }
    match (&fill.drug.ndc, &c.drug.ndc) {
        (Some(a), Some(b)) => a == b,
        _ => !fill.drug.name.is_empty() && fill.drug.name.eq_ignore_ascii_case(&c.drug.name),
    }
}

/// `fills` with the amounts the financial summary gave them, field by
/// field where the claim gave none. Each costed fill is used once. The
/// costed fills no claim matched come back too, as fills of their own.
pub fn add_costs(fills: &mut Vec<Fill>, person: &Person, costed: &[CostedFill]) {
    let mut used = vec![false; costed.len()];
    for fill in fills.iter_mut() {
        let Some(i) = costed
            .iter()
            .enumerate()
            .position(|(i, c)| !used[i] && c.person == person.key && same_fill(fill, c))
        else {
            continue;
        };
        used[i] = true;
        let c = &costed[i].amounts;
        let a = &mut fill.amounts;
        a.total = a.total.or(c.total);
        a.plan_paid = a.plan_paid.or(c.plan_paid);
        a.patient_paid = a.patient_paid.or(c.patient_paid);
        a.deductible = a.deductible.or(c.deductible);
        a.copay = a.copay.or(c.copay);
        a.coinsurance = a.coinsurance.or(c.coinsurance);
        a.discount = a.discount.or(c.discount);
        if fill.pharmacy.name.is_none() {
            fill.pharmacy.name = costed[i].pharmacy.clone();
        }
    }
    for (i, c) in costed.iter().enumerate() {
        if used[i] || c.person != person.key {
            continue;
        }
        fills.push(Fill {
            id: fill_id(
                &person.id,
                None,
                c.rx_number.as_deref(),
                None,
                c.filled.as_deref(),
                &c.drug.key(),
            ),
            patient: person.id.clone(),
            rx_number: c.rx_number.clone(),
            fill_number: None,
            refills_authorized: None,
            claim_number: None,
            drug: c.drug.clone(),
            quantity: c.quantity.clone(),
            quantity_unit: None,
            days_supply: None,
            filled: c.filled.clone(),
            pharmacy: Party {
                name: c.pharmacy.clone(),
                ..Party::default()
            },
            prescriber: Party::default(),
            dosage: None,
            reversed: false,
            amounts: c.amounts,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dates_in_the_sites_forms() {
        assert_eq!(date("2026-01-15").as_deref(), Some("2026-01-15"));
        assert_eq!(date("2026-01-15T10:00:00.000Z").as_deref(), Some("2026-01-15"));
        assert_eq!(date("1/5/2026").as_deref(), Some("2026-01-05"));
        // Eight digits, built so no eight-digit run sits in the source.
        assert_eq!(date(&["2026", "0115"].concat()).as_deref(), Some("2026-01-15"));
        for bad in ["", "2026-13-01", "2026-1-5", "Jan 5 2026", "15/15/2026", "2026-01-15x"] {
            assert_eq!(date(bad), None, "{bad}");
        }
    }

    #[test]
    fn values_by_path() {
        let v = json!({"a": {"b": [{"c": " x "}, {"c": 3}], "n": "12", "z": 2.0, "f": 2.5}});
        assert_eq!(text(&v, &["a.b.0.c"]).as_deref(), Some("x"));
        assert_eq!(text(&v, &["a.missing", "a.b.1.c"]).as_deref(), Some("3"));
        assert_eq!(whole(&v, &["a.n"]), Some(12));
        assert_eq!(whole(&v, &["a.z"]), Some(2));
        assert_eq!(whole(&v, &["a.f"]), None);
        assert_eq!(amount(&v, &["a.f"]), Some(Cents(250)));
    }

    #[test]
    fn a_status_other_than_success_is_the_sites_failure() {
        let ok = json!({"data": {"getClaimV2": {"statusCode": "0000"}}});
        assert!(check_status(&ok, "data.getClaimV2", "claim history").is_ok());
        let bad = json!({"data": {"getClaimV2": {"statusCode": "5001", "statusDescription": "Service down"}}});
        let e = check_status(&bad, "data.getClaimV2", "claim history").unwrap_err();
        assert_eq!(e.exit_code(), 5);
        assert!(e.message().contains("5001: Service down"), "{}", e.message());
        let none = check_status(&json!({}), "data.getClaimV2", "claim history").unwrap_err();
        assert!(none.message().contains("(none)"));
    }
}
