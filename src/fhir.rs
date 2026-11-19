//! The FHIR R4 resources `records export` writes, by hand (no FHIR
//! library, per the household's rules), from `crate::model`. The shapes
//! follow CARIN Blue Button's Pharmacy EOB and match uhc-cli's and
//! kaiser-cli's claims, so medrec reads every insurer's the same way.
//! `docs/fhir.md` lists each field and where it comes from.

use serde_json::{json, Map, Value};

use crate::cents::Cents;
use crate::model::{Amounts, BrandGeneric, Fill, Party, Person, Prescription};

pub const ORGANIZATION: &str = "caremark";
pub const ORGANIZATION_NAME: &str = "CVS Caremark";
pub const META_SOURCE: &str = "caremark:member-site";
pub const TAG_SYSTEM: &str = "https://github.com/piekstra/caremark-cli/fhir/tag";
pub const TAG_MAPPED: &str = "mapped-from-member-site";
pub const MEMBER_ID_SYSTEM: &str = "https://www.caremark.com/fhir/member-id";
pub const RX_NUMBER_SYSTEM: &str = "https://www.caremark.com/fhir/rx-number";
pub const CLAIM_NUMBER_SYSTEM: &str = "https://www.caremark.com/fhir/claim-number";

pub const NDC: &str = "http://hl7.org/fhir/sid/ndc";
pub const RXNORM: &str = "http://www.nlm.nih.gov/research/umls/rxnorm";
const GPI: &str = "https://www.wolterskluwer.com/en/solutions/medi-span/gpi";
const NPI: &str = "http://hl7.org/fhir/sid/us-npi";
const NCPDP_PROVIDER: &str = "http://terminology.hl7.org/NamingSystem/NCPDPProviderIdentificationNumber";
const ACT_CODE: &str = "http://terminology.hl7.org/CodeSystem/v3-ActCode";
const CLAIM_TYPE: &str = "http://terminology.hl7.org/CodeSystem/claim-type";
const C4BB_ADJUDICATION: &str = "http://hl7.org/fhir/us/carin-bb/CodeSystem/C4BBAdjudication";
const HL7_ADJUDICATION: &str = "http://terminology.hl7.org/CodeSystem/adjudication";
const C4BB_SUPPORTING_INFO: &str = "http://hl7.org/fhir/us/carin-bb/CodeSystem/C4BBSupportingInfoType";
const C4BB_IDENTIFIER: &str = "http://hl7.org/fhir/us/carin-bb/CodeSystem/C4BBIdentifierType";
const BRAND_GENERIC: &str = "http://terminology.hl7.org/CodeSystem/NCPDPBrandGenericIndicator";
const UCUM: &str = "http://unitsofmeasure.org";
const DATA_ABSENT_REASON: &str = "http://hl7.org/fhir/StructureDefinition/data-absent-reason";

fn meta() -> Value {
    json!({
        "source": META_SOURCE,
        "tag": [{ "system": TAG_SYSTEM, "code": TAG_MAPPED, "display": "Mapped by caremark from the CVS Caremark member site" }],
    })
}

fn absent() -> Value {
    json!({ "extension": [{ "url": DATA_ABSENT_REASON, "valueCode": "unknown" }] })
}

fn coding(system: &str, code: &str) -> Value {
    json!({ "coding": [{ "system": system, "code": code }] })
}

fn coding_display(system: &str, code: &str, display: &str) -> Value {
    json!({ "coding": [{ "system": system, "code": code, "display": display }] })
}

fn money(c: Cents) -> Value {
    json!({ "value": c.fhir_value(), "currency": "USD" })
}

fn patient_ref(patient: &str) -> Value {
    json!({ "reference": format!("Patient/{patient}") })
}

fn insurer_ref() -> Value {
    json!({ "reference": format!("Organization/{ORGANIZATION}"), "display": ORGANIZATION_NAME })
}

/// A quantity's value as a JSON number: `30`, `2.5`. `None` for anything
/// that is not a plain decimal.
fn number(raw: &str) -> Option<Value> {
    let raw = raw.trim();
    let ok = !raw.is_empty()
        && raw.bytes().filter(|b| *b == b'.').count() <= 1
        && raw.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && !raw.starts_with('.')
        && !raw.ends_with('.');
    if !ok {
        return None;
    }
    serde_json::from_str::<serde_json::Number>(raw).ok().map(Value::Number)
}

/// The drug as a CodeableConcept: RxNorm first when known (medrec prefers
/// it), then the NDC, then Medi-Span's GPI, with the site's label as text.
pub fn medication(fill_drug: &crate::model::Drug) -> Value {
    let label = fill_drug.label();
    let mut codings = Vec::new();
    if let Some(code) = &fill_drug.rxnorm {
        codings.push(json!({ "system": RXNORM, "code": code, "display": label }));
    }
    if let Some(code) = &fill_drug.ndc {
        codings.push(json!({ "system": NDC, "code": code, "display": label }));
    }
    if let Some(code) = &fill_drug.gpi {
        codings.push(json!({ "system": GPI, "code": code }));
    }
    let mut v = Map::new();
    if !codings.is_empty() {
        v.insert("coding".into(), Value::Array(codings));
    }
    if !label.is_empty() {
        v.insert("text".into(), label.into());
    }
    Value::Object(v)
}

/// A pharmacy or prescriber as a Reference: its name, and its NPI (or a
/// pharmacy's NCPDP id) as the identifier.
fn party(p: &Party) -> Value {
    let mut v = Map::new();
    if let Some(npi) = &p.npi {
        v.insert("identifier".into(), json!({ "system": NPI, "value": npi }));
    } else if let Some(id) = &p.ncpdp {
        v.insert("identifier".into(), json!({ "system": NCPDP_PROVIDER, "value": id }));
    }
    if let Some(name) = &p.name {
        v.insert("display".into(), name.clone().into());
    }
    Value::Object(v)
}

pub fn organization() -> Value {
    json!({
        "resourceType": "Organization",
        "id": ORGANIZATION,
        "meta": meta(),
        "active": true,
        "type": [coding("http://terminology.hl7.org/CodeSystem/organization-type", "pay")],
        "name": ORGANIZATION_NAME,
    })
}

pub fn patient(p: &Person) -> Value {
    let mut v = json!({
        "resourceType": "Patient",
        "id": p.id,
        "meta": meta(),
    });
    if let Some(id) = &p.member_id {
        v["identifier"] = json!([{
            "type": coding("http://terminology.hl7.org/CodeSystem/v2-0203", "MB"),
            "system": MEMBER_ID_SYSTEM,
            "value": id,
        }]);
    }
    let mut name = Map::new();
    if let Some(full) = p.name() {
        name.insert("text".into(), full.into());
    }
    if let Some(last) = p.last_name.as_deref().filter(|s| !s.trim().is_empty()) {
        name.insert("family".into(), last.into());
    }
    if let Some(first) = p.first_name.as_deref().filter(|s| !s.trim().is_empty()) {
        name.insert("given".into(), json!([first]));
    }
    if !name.is_empty() {
        v["name"] = json!([Value::Object(name)]);
    }
    if let Some(d) = &p.birth_date {
        v["birthDate"] = d.clone().into();
    }
    v
}

pub fn coverage_id(p: &Person) -> String {
    format!("cov-{}", p.id)
}

/// The pharmacy benefit Caremark administers for the person.
pub fn coverage(p: &Person) -> Value {
    let mut v = json!({
        "resourceType": "Coverage",
        "id": coverage_id(p),
        "meta": meta(),
        "status": "active",
        "type": coding_display(ACT_CODE, "DRUGPOL", "drug policy"),
    });
    if let Some(id) = &p.member_id {
        v["subscriberId"] = id.clone().into();
    }
    v["beneficiary"] = patient_ref(&p.id);
    if p.role == Some(pk_med_records::dto::member_role::SUBSCRIBER) {
        v["relationship"] = coding("http://terminology.hl7.org/CodeSystem/subscriber-relationship", "self");
    }
    v["payor"] = json!([insurer_ref()]);
    v
}

/// The prescription the fills of one number share.
pub fn medication_request(rx: &Prescription) -> Value {
    let mut v = json!({
        "resourceType": "MedicationRequest",
        "id": rx.id,
        "meta": meta(),
        "identifier": [{ "system": RX_NUMBER_SYSTEM, "value": rx.rx_number }],
        "status": request_status(rx.status.as_deref()),
        "intent": "order",
        "medicationCodeableConcept": medication(&rx.drug),
        "subject": patient_ref(&rx.patient),
    });
    if !rx.prescriber.is_empty() {
        v["requester"] = party(&rx.prescriber);
    }
    if let Some(n) = rx.refills_authorized {
        v["dispenseRequest"] = json!({ "numberOfRepeatsAllowed": n });
    }
    v
}

/// The site's status words, as `MedicationRequest.status`.
fn request_status(site: Option<&str>) -> &'static str {
    match site.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("active") | Some("refillable") | Some("ready") => "active",
        Some("expired") | Some("no refills") | Some("no refills remaining") | Some("completed") => "completed",
        Some("cancelled") | Some("canceled") | Some("discontinued") | Some("inactive") => "stopped",
        Some("on hold") | Some("hold") => "on-hold",
        _ => "unknown",
    }
}

fn identifiers(f: &Fill) -> Vec<Value> {
    let mut ids = Vec::new();
    if let Some(rx) = &f.rx_number {
        ids.push(json!({ "system": RX_NUMBER_SYSTEM, "value": rx }));
    }
    if let Some(claim) = &f.claim_number {
        ids.push(json!({
            "type": coding(C4BB_IDENTIFIER, "uc"),
            "system": CLAIM_NUMBER_SYSTEM,
            "value": claim,
        }));
    }
    ids
}

fn quantity(f: &Fill) -> Option<Value> {
    let mut q = Map::new();
    q.insert("value".into(), number(f.quantity.as_deref()?)?);
    if let Some(unit) = &f.quantity_unit {
        q.insert("unit".into(), unit.clone().into());
    }
    Some(Value::Object(q))
}

fn days(n: u32) -> Value {
    json!({ "value": n, "unit": "day", "system": UCUM, "code": "d" })
}

/// One fill, as dispensed. `prescription` is its MedicationRequest's id.
pub fn medication_dispense(f: &Fill, prescription: Option<&str>) -> Value {
    let mut v = json!({
        "resourceType": "MedicationDispense",
        "id": f.id,
        "meta": meta(),
    });
    let ids = identifiers(f);
    if !ids.is_empty() {
        v["identifier"] = Value::Array(ids);
    }
    v["status"] = if f.reversed { "cancelled" } else { "completed" }.into();
    v["medicationCodeableConcept"] = medication(&f.drug);
    v["subject"] = patient_ref(&f.patient);
    if !f.pharmacy.is_empty() {
        v["performer"] = json!([{ "actor": party(&f.pharmacy) }]);
    }
    if let Some(id) = prescription {
        v["authorizingPrescription"] = json!([{ "reference": format!("MedicationRequest/{id}") }]);
    }
    if let Some(n) = f.fill_number {
        let (code, display) = if n == 0 { ("FF", "First Fill") } else { ("RF", "Refill") };
        v["type"] = coding_display(ACT_CODE, code, display);
    }
    if let Some(q) = quantity(f) {
        v["quantity"] = q;
    }
    if let Some(n) = f.days_supply {
        v["daysSupply"] = days(n);
    }
    // The claim's date of service is the day the pharmacy dispensed the
    // fill; Da Vinci PDex maps it to `whenHandedOver` too.
    if let Some(d) = &f.filled {
        v["whenHandedOver"] = d.clone().into();
    }
    if let Some(text) = &f.dosage {
        v["dosageInstruction"] = json!([{ "text": text }]);
    }
    v
}

/// The adjudication rows of a fill: one per amount the site gave, in
/// CARIN's categories. `None` amounts are left out, never written as zero.
fn adjudication(a: &Amounts) -> Vec<Value> {
    let rows: [(&str, &str, &str, Option<Cents>); 7] = [
        (HL7_ADJUDICATION, "eligible", "Eligible Amount", a.total),
        (C4BB_ADJUDICATION, "discount", "Discount", a.discount),
        (HL7_ADJUDICATION, "benefit", "Benefit Amount", a.plan_paid),
        (HL7_ADJUDICATION, "deductible", "Deductible", a.deductible),
        (HL7_ADJUDICATION, "copay", "CoPay", a.copay),
        (C4BB_ADJUDICATION, "coinsurance", "Coinsurance", a.coinsurance),
        (C4BB_ADJUDICATION, "memberliability", "Member liability", a.patient_paid),
    ];
    rows.iter()
        .filter_map(|(system, code, display, amount)| {
            amount.map(|amount| {
                json!({
                    "category": coding_display(system, code, display),
                    "amount": money(amount),
                })
            })
        })
        .collect()
}

/// One fill's pharmacy claim (CARIN's Pharmacy EOB).
pub fn explanation_of_benefit(f: &Fill, coverage: &str) -> Value {
    let mut v = json!({
        "resourceType": "ExplanationOfBenefit",
        "id": f.id,
        "meta": meta(),
    });
    let ids = identifiers(f);
    if !ids.is_empty() {
        v["identifier"] = Value::Array(ids);
    }
    // A reversed claim is cancelled; its amounts are the original's.
    v["status"] = if f.reversed { "cancelled" } else { "active" }.into();
    v["type"] = coding_display(CLAIM_TYPE, "pharmacy", "Pharmacy");
    v["use"] = "claim".into();
    v["patient"] = patient_ref(&f.patient);
    match &f.filled {
        Some(d) => {
            v["billablePeriod"] = json!({ "start": d, "end": d });
            v["created"] = d.clone().into();
        }
        None => v["_created"] = absent(),
    }
    v["insurer"] = insurer_ref();
    v["provider"] = if f.pharmacy.is_empty() {
        absent()
    } else {
        party(&f.pharmacy)
    };
    v["outcome"] = "complete".into();

    let mut info: Vec<Value> = Vec::new();
    let mut push = |category: &str, mut entry: Value| {
        entry["sequence"] = (info.len() + 1).into();
        entry["category"] = coding(C4BB_SUPPORTING_INFO, category);
        info.push(entry);
    };
    if let Some(n) = f.days_supply {
        push("dayssupply", json!({ "valueQuantity": { "value": n } }));
    }
    if let Some(n) = f.fill_number {
        push("refillnum", json!({ "valueQuantity": { "value": n } }));
    }
    if let Some(n) = f.refills_authorized {
        push("refillsauthorized", json!({ "valueQuantity": { "value": n } }));
    }
    if let Some(bg) = f.drug.brand_generic {
        let (code, display) = match bg {
            BrandGeneric::Brand => ("1", "Brand"),
            BrandGeneric::Generic => ("2", "Generic"),
        };
        push(
            "brandgenericindicator",
            json!({ "code": coding_display(BRAND_GENERIC, code, display) }),
        );
    }
    if !info.is_empty() {
        v["supportingInfo"] = Value::Array(info);
    }
    v["insurance"] = json!([{ "focal": true, "coverage": { "reference": format!("Coverage/{coverage}") } }]);

    let mut item = json!({
        "sequence": 1,
        "productOrService": medication(&f.drug),
    });
    if let Some(d) = &f.filled {
        item["servicedDate"] = d.clone().into();
    }
    if let Some(q) = quantity(f) {
        item["quantity"] = q;
    }
    if let Some(total) = f.amounts.total {
        item["net"] = money(total);
    }
    let rows = adjudication(&f.amounts);
    if !rows.is_empty() {
        item["adjudication"] = Value::Array(rows.clone());
        v["total"] = Value::Array(rows);
    }
    v["item"] = json!([item]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{fill_id, patient_id, prescription_id, Drug};

    fn person() -> Person {
        Person {
            id: patient_id("01"),
            key: "01".into(),
            first_name: Some("Alex".into()),
            last_name: Some("Example".into()),
            birth_date: Some("1980-01-01".into()),
            role: Some(pk_med_records::dto::member_role::SUBSCRIBER),
            member_id: Some("SYNM000001".into()),
            signed_in: true,
        }
    }

    fn fill() -> Fill {
        let patient = patient_id("01");
        Fill {
            id: fill_id(&patient, Some("0000000001"), None, None, None, ""),
            patient,
            rx_number: Some("0000001".into()),
            fill_number: Some(2),
            refills_authorized: Some(5),
            claim_number: Some("0000000001".into()),
            drug: Drug {
                name: "EXAMPLEPRIL".into(),
                strength: Some("10 MG".into()),
                form: Some("TABLET".into()),
                ndc: Some("00000000001".into()),
                rxnorm: None,
                gpi: None,
                brand_generic: Some(BrandGeneric::Generic),
            },
            quantity: Some("30".into()),
            quantity_unit: Some("tablet".into()),
            days_supply: Some(30),
            filled: Some("2026-01-15".into()),
            pharmacy: Party {
                name: Some("Example Pharmacy".into()),
                npi: Some("0000000000".into()),
                ..Party::default()
            },
            prescriber: Party::default(),
            dosage: None,
            reversed: false,
            amounts: Amounts {
                total: Some(Cents(1250)),
                plan_paid: Some(Cents(250)),
                patient_paid: Some(Cents(1000)),
                copay: Some(Cents(1000)),
                ..Amounts::default()
            },
        }
    }

    fn category(v: &Value) -> &str {
        v["category"]["coding"][0]["code"].as_str().unwrap()
    }

    #[test]
    fn a_fill_is_a_dispense_medrec_can_read() {
        let f = fill();
        let mr = prescription_id(&f.patient, "0000001");
        let d = medication_dispense(&f, Some(&mr));
        assert_eq!(d["resourceType"], "MedicationDispense");
        assert_eq!(d["status"], "completed");
        assert_eq!(d["medicationCodeableConcept"]["coding"][0]["system"], NDC);
        assert_eq!(d["medicationCodeableConcept"]["coding"][0]["code"], "00000000001");
        assert_eq!(d["medicationCodeableConcept"]["text"], "EXAMPLEPRIL 10 MG TABLET");
        assert_eq!(d["whenHandedOver"], "2026-01-15");
        assert_eq!(d["quantity"], json!({ "value": 30, "unit": "tablet" }));
        assert_eq!(d["daysSupply"]["value"], 30);
        assert_eq!(d["type"]["coding"][0]["code"], "RF");
        assert_eq!(d["performer"][0]["actor"]["display"], "Example Pharmacy");
        assert_eq!(d["performer"][0]["actor"]["identifier"]["system"], NPI);
        assert_eq!(
            d["authorizingPrescription"][0]["reference"],
            format!("MedicationRequest/{mr}")
        );
        assert_eq!(d["meta"]["source"], META_SOURCE);
        assert!(d.get("dosageInstruction").is_none());
    }

    #[test]
    fn rxnorm_comes_first_when_known() {
        let mut f = fill();
        f.drug.rxnorm = Some("000001".into());
        let d = medication_dispense(&f, None);
        assert_eq!(d["medicationCodeableConcept"]["coding"][0]["system"], RXNORM);
        assert_eq!(d["medicationCodeableConcept"]["coding"][1]["system"], NDC);
        assert!(d.get("authorizingPrescription").is_none());
    }

    #[test]
    fn a_fill_is_a_carin_pharmacy_claim() {
        let f = fill();
        let e = explanation_of_benefit(&f, "cov-p0");
        assert_eq!(e["type"]["coding"][0]["code"], "pharmacy");
        assert_eq!(e["status"], "active");
        assert_eq!(e["created"], "2026-01-15");
        assert_eq!(e["provider"]["display"], "Example Pharmacy");
        assert_eq!(e["insurer"]["display"], ORGANIZATION_NAME);
        let info = e["supportingInfo"].as_array().unwrap();
        let codes: Vec<&str> = info.iter().map(category).collect();
        assert_eq!(
            codes,
            ["dayssupply", "refillnum", "refillsauthorized", "brandgenericindicator"]
        );
        assert_eq!(info[0]["valueQuantity"]["value"], 30);
        assert_eq!(info[1]["valueQuantity"]["value"], 2);
        assert_eq!(info[2]["valueQuantity"]["value"], 5);
        assert_eq!(info[3]["code"]["coding"][0]["code"], "2");
        assert_eq!(info[3]["sequence"], 4);
        let item = &e["item"][0];
        assert_eq!(item["productOrService"]["coding"][0]["code"], "00000000001");
        assert_eq!(item["quantity"]["value"], 30);
        assert_eq!(item["net"]["value"].to_string(), "12.5");
        let cats: Vec<&str> = item["adjudication"].as_array().unwrap().iter().map(category).collect();
        // Only the amounts the site gave: no zero deductible or coinsurance.
        assert_eq!(cats, ["eligible", "benefit", "copay", "memberliability"]);
        assert_eq!(e["total"], item["adjudication"]);
        assert_eq!(e["insurance"][0]["coverage"]["reference"], "Coverage/cov-p0");
    }

    #[test]
    fn a_reversed_claim_is_cancelled_and_a_bare_one_says_what_is_missing() {
        let mut f = fill();
        f.reversed = true;
        f.filled = None;
        f.pharmacy = Party::default();
        f.amounts = Amounts::default();
        f.quantity = Some("thirty".into());
        let e = explanation_of_benefit(&f, "cov-p0");
        assert_eq!(e["status"], "cancelled");
        assert_eq!(e["_created"], absent());
        assert_eq!(e["provider"], absent());
        assert!(e.get("total").is_none());
        assert!(e["item"][0].get("quantity").is_none());
        let d = medication_dispense(&f, None);
        assert_eq!(d["status"], "cancelled");
        assert!(d.get("performer").is_none());
        assert!(d.get("whenHandedOver").is_none());
    }

    #[test]
    fn people_and_coverage() {
        let p = person();
        let v = patient(&p);
        assert_eq!(v["name"][0]["text"], "Alex Example");
        assert_eq!(v["name"][0]["given"][0], "Alex");
        assert_eq!(v["identifier"][0]["value"], "SYNM000001");
        assert_eq!(v["birthDate"], "1980-01-01");
        let c = coverage(&p);
        assert_eq!(c["beneficiary"]["reference"], format!("Patient/{}", p.id));
        assert_eq!(c["relationship"]["coding"][0]["code"], "self");
        assert_eq!(c["payor"][0]["reference"], "Organization/caremark");
        let mut dependent = person();
        dependent.role = Some(pk_med_records::dto::member_role::DEPENDENT);
        dependent.member_id = None;
        let c = coverage(&dependent);
        assert!(c.get("relationship").is_none());
        assert!(c.get("subscriberId").is_none());
    }

    #[test]
    fn a_prescription_names_its_prescriber() {
        let f = fill();
        let rx = Prescription {
            id: prescription_id(&f.patient, "0000001"),
            patient: f.patient.clone(),
            rx_number: "0000001".into(),
            drug: f.drug.clone(),
            prescriber: Party {
                name: Some("Dr. Example".into()),
                npi: Some("0000000000".into()),
                ..Party::default()
            },
            refills_authorized: Some(5),
            refills_remaining: Some(3),
            status: Some("Active".into()),
            last_filled: f.filled.clone(),
        };
        let v = medication_request(&rx);
        assert_eq!(v["status"], "active");
        assert_eq!(v["intent"], "order");
        assert_eq!(v["requester"]["display"], "Dr. Example");
        assert_eq!(v["dispenseRequest"]["numberOfRepeatsAllowed"], 5);
        assert_eq!(request_status(Some("Expired")), "completed");
        assert_eq!(request_status(None), "unknown");
    }

    #[test]
    fn quantities_are_plain_numbers_only() {
        assert_eq!(number("30"), Some(json!(30)));
        assert_eq!(number("2.5"), Some(json!(2.5)));
        for bad in ["", "1.2.3", ".5", "5.", "-1", "30 TAB"] {
            assert_eq!(number(bad), None, "{bad}");
        }
    }
}
