//! What caremark knows of a member and a fill, apart from how the site
//! answers it (`crate::portal` reads the answers into these), so the FHIR
//! mapping (`crate::fhir`) and the commands share one shape.

use sha2::{Digest, Sha256};

use crate::cents::Cents;

/// A person on the account: the signed-in member or one of the family the
/// site shows under the sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// The export's `Patient.id` ([`patient_id`]).
    pub id: String,
    /// The site's own key for the person, which reads take (a person code).
    pub key: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    /// `YYYY-MM-DD`.
    pub birth_date: Option<String>,
    /// `subscriber` or `dependent` (`pk_med_records::member_role`).
    pub role: Option<&'static str>,
    /// The Caremark member id printed on the card, when the site gives it.
    pub member_id: Option<String>,
    /// The member the session is signed in as.
    pub signed_in: bool,
}

impl Person {
    /// First and last name, as the site shows them.
    pub fn name(&self) -> Option<String> {
        let parts: Vec<&str> = [self.first_name.as_deref(), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

/// A pharmacy or a prescriber.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Party {
    pub name: Option<String>,
    pub npi: Option<String>,
    /// A pharmacy's NCPDP provider id.
    pub ncpdp: Option<String>,
    pub phone: Option<String>,
}

impl Party {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.npi.is_none() && self.ncpdp.is_none()
    }
}

/// Brand or generic, as the claim says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrandGeneric {
    Brand,
    Generic,
}

/// A fill's amounts. `None` is "not given", never zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Amounts {
    /// The prescription's total cost.
    pub total: Option<Cents>,
    /// What the plan paid.
    pub plan_paid: Option<Cents>,
    /// What the member paid.
    pub patient_paid: Option<Cents>,
    pub deductible: Option<Cents>,
    pub copay: Option<Cents>,
    pub coinsurance: Option<Cents>,
    /// Manufacturer discounts and coupons that lowered the member's cost.
    pub discount: Option<Cents>,
}

impl Amounts {
    pub fn is_empty(&self) -> bool {
        *self == Amounts::default()
    }
}

/// One dispensed fill, with its pharmacy claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fill {
    /// The export's id for the fill ([`fill_id`]): the `MedicationDispense`
    /// and its `ExplanationOfBenefit` share it.
    pub id: String,
    /// The person's [`Person::id`].
    pub patient: String,
    pub rx_number: Option<String>,
    /// 0 for the first fill, then the refill number.
    pub fill_number: Option<u32>,
    pub refills_authorized: Option<u32>,
    pub claim_number: Option<String>,
    pub drug: Drug,
    /// The dispensed quantity, as the site gives it (`30`, `2.5`).
    pub quantity: Option<String>,
    pub quantity_unit: Option<String>,
    pub days_supply: Option<u32>,
    /// `YYYY-MM-DD`.
    pub filled: Option<String>,
    pub pharmacy: Party,
    pub prescriber: Party,
    pub dosage: Option<String>,
    /// A reversed claim: the fill did not happen (returned to stock).
    pub reversed: bool,
    pub amounts: Amounts,
}

/// A prescription: what the fills of one prescription number share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prescription {
    /// The export's `MedicationRequest.id` ([`prescription_id`]).
    pub id: String,
    pub patient: String,
    pub rx_number: String,
    pub drug: Drug,
    pub prescriber: Party,
    pub refills_authorized: Option<u32>,
    pub refills_remaining: Option<u32>,
    /// The site's own word for it (`Active`, `Expired`), when it gives one.
    pub status: Option<String>,
    /// `YYYY-MM-DD`: the latest fill caremark has.
    pub last_filled: Option<String>,
}

/// The drug dispensed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Drug {
    pub name: String,
    pub strength: Option<String>,
    pub form: Option<String>,
    /// 11 digits, no dashes.
    pub ndc: Option<String>,
    pub rxnorm: Option<String>,
    /// Medi-Span's Generic Product Identifier.
    pub gpi: Option<String>,
    pub brand_generic: Option<BrandGeneric>,
}

impl Drug {
    /// What identifies the drug when nothing else names a fill: its NDC,
    /// else its label in upper case.
    pub fn key(&self) -> String {
        self.ndc.clone().unwrap_or_else(|| self.label().to_ascii_uppercase())
    }

    /// "Name strength form", as the site's own pages label it.
    pub fn label(&self) -> String {
        [Some(self.name.as_str()), self.strength.as_deref(), self.form.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `prefix` and the first `hex` hex digits of SHA-256 of `parts` joined by
/// `|`.
fn hashed(prefix: &str, parts: &[&str], hex: usize) -> String {
    let digest = Sha256::digest(parts.join("|").as_bytes());
    let mut s = String::with_capacity(prefix.len() + hex);
    s.push_str(prefix);
    for b in digest.iter() {
        if s.len() >= prefix.len() + hex {
            break;
        }
        s.push_str(&format!("{b:02x}"));
    }
    s.truncate(prefix.len() + hex);
    s
}

/// A person's `Patient.id`: stable across pulls, and carrying no name or
/// member id. `key` is what identifies the person to the site.
pub fn patient_id(key: &str) -> String {
    hashed("p", &["caremark", key], 12)
}

/// A fill's id, under the patient's: the claim number when there is one,
/// else the prescription number, fill number and date, else the date and
/// the drug ([`Drug::key`]).
pub fn fill_id(
    patient: &str,
    claim_number: Option<&str>,
    rx: Option<&str>,
    fill: Option<u32>,
    date: Option<&str>,
    drug: &str,
) -> String {
    let fill = fill.map(|n| n.to_string());
    let date = date.unwrap_or("");
    match (claim_number.filter(|c| !c.is_empty()), rx.filter(|r| !r.is_empty())) {
        (Some(claim), _) => hashed("rx", &[patient, "claim", claim], 16),
        (None, Some(rx)) => hashed("rx", &[patient, rx, fill.as_deref().unwrap_or(""), date], 16),
        (None, None) => hashed("rx", &[patient, "drug", drug, date], 16),
    }
}

/// A prescription's id: its number under the patient's id.
pub fn prescription_id(patient: &str, rx_number: &str) -> String {
    hashed("mr", &[patient, "rx", rx_number], 16)
}

/// An NDC as 11 digits: `00000-0000-01` and `00000000001` alike. `None`
/// for anything else.
pub fn ndc11(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let parts: Vec<&str> = raw.split('-').collect();
    let digits = match parts.as_slice() {
        [all] if all.len() == 11 && all.bytes().all(|b| b.is_ascii_digit()) => all.to_string(),
        // 4-4-2, 5-3-2 and 5-4-1 pad to 5-4-2.
        [a, b, c]
            if [a, b, c]
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|x| x.is_ascii_digit())) =>
        {
            format!("{a:0>5}{b:0>4}{c:0>2}")
        }
        _ => return None,
    };
    (digits.len() == 11).then_some(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_carry_nothing_of_the_person() {
        let p = patient_id("SYNM000001");
        assert_eq!(p.len(), 13);
        assert!(p.starts_with('p'));
        assert_eq!(p, patient_id("SYNM000001"));
        assert_ne!(p, patient_id("SYNM000002"));
        assert!(!p.contains("SYNM"));
        let by_claim = fill_id(
            &p,
            Some("0000000001"),
            Some("0000001"),
            Some(0),
            Some("2026-01-01"),
            "x",
        );
        assert_eq!(by_claim.len(), 18);
        // The claim number decides alone: a re-read with more details is the
        // same fill.
        assert_eq!(by_claim, fill_id(&p, Some("0000000001"), None, None, None, "y"));
        let by_rx = fill_id(&p, None, Some("0000001"), Some(1), Some("2026-02-01"), "x");
        assert_ne!(
            by_rx,
            fill_id(&p, None, Some("0000001"), Some(2), Some("2026-03-01"), "x")
        );
        assert_eq!(
            by_rx,
            fill_id(&p, Some(""), Some("0000001"), Some(1), Some("2026-02-01"), "y")
        );
        // With neither, two drugs filled on one day are two fills.
        let a = fill_id(&p, None, None, None, Some("2026-02-01"), "00000000001");
        assert_ne!(a, fill_id(&p, None, None, None, Some("2026-02-01"), "00000000002"));
        let mr = prescription_id(&p, "0000001");
        assert_eq!(mr.len(), 18);
        assert_ne!(mr, prescription_id(&p, "0000002"));
    }

    #[test]
    fn ndcs_become_eleven_digits() {
        assert_eq!(ndc11("00000000001").as_deref(), Some("00000000001"));
        assert_eq!(ndc11("0000-0000-01").as_deref(), Some("00000000001"));
        assert_eq!(ndc11("00000-000-01").as_deref(), Some("00000000001"));
        assert_eq!(ndc11("00000-0000-1").as_deref(), Some("00000000001"));
        for bad in ["", "123", "abc-def-gh", "000000000001", "0000-0000"] {
            assert_eq!(ndc11(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_drug_is_labelled_as_the_site_labels_it() {
        let d = Drug {
            name: "EXAMPLEPRIL".into(),
            strength: Some("10 MG".into()),
            form: Some("TABLET".into()),
            ..Drug::default()
        };
        assert_eq!(d.label(), "EXAMPLEPRIL 10 MG TABLET");
        let p = Person {
            id: "p0".into(),
            key: "01".into(),
            first_name: Some("Alex".into()),
            last_name: Some("Example".into()),
            birth_date: None,
            role: None,
            member_id: None,
            signed_in: true,
        };
        assert_eq!(p.name().as_deref(), Some("Alex Example"));
    }
}
