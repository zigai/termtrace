use serde::{de::DeserializeOwned, Deserialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    path::Path,
};
use termtrace_sdk::contracts::{decode_media_artifact, encode_media_artifact};

const LANGUAGES: [&str; 3] = ["python", "rust", "typescript"];
const CASE_KINDS: [&str; 3] = ["lifecycle", "negative", "positive"];
const FOUNDATION_CASE: &str = "foundation";

#[derive(Deserialize)]
struct Matrix {
    rows: Vec<MatrixRow>,
}

#[derive(Deserialize)]
struct MatrixRow {
    id: String,
    core: CoreReference,
    canonical_model: String,
    rust_feature: String,
    units: String,
    availability: Vec<String>,
    closure_owner: String,
    languages: BTreeMap<String, LanguageEntry>,
    cases: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct CoreReference {
    path: String,
    symbol: String,
}

#[derive(Deserialize)]
struct LanguageEntry {
    status: ImplementationStatus,
    reason: Option<String>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ImplementationStatus {
    Verified,
    ExposedUnverified,
    Partial,
    Missing,
    NotApplicable,
}

#[derive(Deserialize)]
struct CaseList<T> {
    cases: Vec<T>,
}

#[derive(Deserialize)]
struct PendingCase {
    id: String,
    row_id: String,
    kind: String,
}

#[derive(Deserialize)]
struct InvalidCase {
    id: String,
}

fn fixture<T: DeserializeOwned>(relative: &str) -> Result<T, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/conformance/sdk")
        .join(relative);

    let text = fs_err::read_to_string(&path)?;

    return serde_json::from_str(&text)
        .map_err(|error| return format!("{}: {error}", path.display()).into());
}

fn is_row_id(id: &str) -> bool {
    let segment = |part: &str| {
        return !part.is_empty()
            && part
                .bytes()
                .all(|b| return b.is_ascii_lowercase() || b == b'_');
    };

    return id
        .split_once('.')
        .is_some_and(|(family, name)| return segment(family) && segment(name));
}

fn check_row(
    row: &MatrixRow,
    fixture_ids: &BTreeSet<String>,
    pending: &BTreeMap<&str, &PendingCase>,
    errors: &mut Vec<String>,
) {
    let id = &row.id;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    if !is_row_id(id) {
        errors.push(format!("{id}: invalid row ID"));
    }

    if !row.languages.keys().map(String::as_str).eq(LANGUAGES) {
        errors.push(format!("{id}: languages must be exactly {LANGUAGES:?}"));
    }

    if !root.join(&row.core.path).is_file()
        || row.core.symbol.is_empty()
        || row.canonical_model.is_empty()
    {
        errors.push(format!("{id}: stale core source reference"));
    }

    let known_owner = (1..=11).any(|n| return format!("{n:02}") == row.closure_owner);
    if !known_owner
        || row.availability.is_empty()
        || row.rust_feature.is_empty()
        || row.units.is_empty()
    {
        errors.push(format!("{id}: incomplete ownership or requirements"));
    }

    for (language, entry) in &row.languages {
        let has_reason = entry
            .reason
            .as_deref()
            .is_some_and(|r| return !r.is_empty());

        if entry.status == ImplementationStatus::NotApplicable && !has_reason {
            errors.push(format!("{id}/{language}: not_applicable needs a reason"));
        }
    }

    if !row.cases.keys().map(String::as_str).eq(CASE_KINDS) {
        errors.push(format!("{id}: cases must be exactly {CASE_KINDS:?}"));
    }

    let foundation = row
        .cases
        .values()
        .filter(|c| return *c == FOUNDATION_CASE)
        .count();

    if foundation != 0 && foundation != row.cases.len() {
        errors.push(format!(
            "{id}: foundation rows use the foundation case for every kind"
        ));
    }

    for (kind, case_id) in &row.cases {
        if !fixture_ids.contains(case_id) {
            errors.push(format!("{id}: unknown fixture case {case_id}"));
            continue;
        }

        if case_id == FOUNDATION_CASE {
            continue;
        }

        match pending.get(case_id.as_str()) {
            Some(case) if case.row_id == *id && case.kind == *kind => {}
            _ => errors.push(format!(
                "{id}: {kind} case {case_id} belongs to another row or kind"
            )),
        }
    }
}

#[test]
fn sdk_contract_inventory_is_complete_and_consistent() {
    let matrix: Matrix = fixture("matrix.json").unwrap();
    let pending: CaseList<PendingCase> = fixture("cases/pending.json").unwrap();
    let invalid: CaseList<InvalidCase> = fixture("cases/portable-invalid.json").unwrap();
    let mut errors = Vec::new();
    let mut fixture_ids = BTreeSet::from([FOUNDATION_CASE.to_owned(), "portable".to_owned()]);
    let candidates = invalid
        .cases
        .iter()
        .map(|case| return format!("portable.{}", case.id))
        .chain(pending.cases.iter().map(|case| return case.id.clone()));

    for case_id in candidates {
        if !fixture_ids.insert(case_id.clone()) {
            errors.push(format!("duplicate fixture case ID {case_id}"));
        }
    }

    let by_id: BTreeMap<&str, &PendingCase> = pending
        .cases
        .iter()
        .map(|case| return (case.id.as_str(), case))
        .collect();

    let mut row_ids = BTreeSet::new();
    for row in &matrix.rows {
        if !row_ids.insert(row.id.as_str()) {
            errors.push(format!("duplicate row ID {}", row.id));
        }

        check_row(row, &fixture_ids, &by_id, &mut errors);
    }

    for case in &pending.cases {
        if !row_ids.contains(case.row_id.as_str()) {
            errors.push(format!("{}: unknown row {}", case.id, case.row_id));
        }
    }

    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn sdk_contract_portable_vector_preserves_exact_values() {
    let wire = include_str!("../../../fixtures/conformance/sdk/cases/portable.json");
    let value = decode_media_artifact(wire).unwrap();

    assert_eq!(value.members[0].byte_length.0, 9_007_199_254_740_993);
    assert_eq!(value.source.boundary.next_event_seq.0, u64::MAX);
    assert_eq!(
        decode_media_artifact(&encode_media_artifact(&value).unwrap()).unwrap(),
        value
    );
}
