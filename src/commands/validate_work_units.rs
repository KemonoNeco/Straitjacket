use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub work_units_file: std::path::PathBuf,
    #[arg(long, default_value_t = false)]
    pub normalize: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitError {
    pub unit_id: Option<String>,
    pub index: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<UnitError>,
}

/// The exact set of keys the schema declares (additionalProperties:false). Any other key
/// on a unit is an unknown-field violation.
const ALLOWED_UNIT_KEYS: &[&str] = &[
    "id",
    "target_file",
    "target_symbol",
    "kind",
    "intended_behavior",
    "preconditions",
    "inputs",
    "expected",
    "fuzzable",
    "output_file_path",
    "output_test_name",
    "target_stub_path",
    "status",
    "round",
    "source_of_unit",
];

/// Fields required to be PRESENT on every unit.
const REQUIRED_UNIT_KEYS: &[&str] = &[
    "id",
    "target_file",
    "target_symbol",
    "kind",
    "intended_behavior",
    "output_file_path",
    "output_test_name",
    "status",
    "round",
];

/// String-typed fields whose presence requires a JSON string value.
const STRING_FIELDS: &[&str] = &[
    "id",
    "target_file",
    "target_symbol",
    "intended_behavior",
    "output_file_path",
    "output_test_name",
    "preconditions",
    "inputs",
    "expected",
];

/// Validates a work-units value (bare array OR {"work_units":[...]} wrapper) against the schema's
/// structural rules. Pure.
pub fn validate_work_units(value: &Value) -> ValidationReport {
    let units = match crate::common::json_io::parse_work_units_array(value) {
        Some(units) => units,
        None => {
            return ValidationReport {
                valid: false,
                errors: vec![UnitError {
                    unit_id: None,
                    index: 0,
                    message:
                        "work-units input is neither a JSON array nor a {work_units:[...]} wrapper"
                            .into(),
                }],
            };
        }
    };

    let mut errors: Vec<UnitError> = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        validate_unit(unit, index, &mut errors);
    }

    ValidationReport {
        valid: errors.is_empty(),
        errors,
    }
}

/// Validates a single work-unit, pushing one `UnitError` per distinct violation.
fn validate_unit(unit: &Value, index: usize, errors: &mut Vec<UnitError>) {
    let obj = match unit.as_object() {
        Some(obj) => obj,
        None => {
            errors.push(UnitError {
                unit_id: None,
                index,
                message: format!("unit at index {index} is not a JSON object"),
            });
            return;
        }
    };

    let unit_id = obj.get("id").and_then(Value::as_str).map(str::to_string);
    let push = |errors: &mut Vec<UnitError>, message: String| {
        errors.push(UnitError {
            unit_id: unit_id.clone(),
            index,
            message,
        });
    };

    // additionalProperties:false — any key outside the declared set is a violation.
    for key in obj.keys() {
        if !ALLOWED_UNIT_KEYS.contains(&key.as_str()) {
            push(errors, format!("unknown field '{key}' is not permitted"));
        }
    }

    // Required fields must be present.
    for &field in REQUIRED_UNIT_KEYS {
        if !obj.contains_key(field) {
            push(errors, format!("required field '{field}' is missing"));
        }
    }

    // Per-field type/enum/constraint checks. Each is gated on presence so an absent
    // field never double-reports alongside its own missing-field error.
    for &field in STRING_FIELDS {
        if let Some(v) = obj.get(field) {
            if !v.is_string() {
                push(errors, format!("field '{field}' must be a string"));
            }
        }
    }

    // intended_behavior minLength 10 (only when present AND a string).
    if let Some(v) = obj.get("intended_behavior") {
        if let Some(s) = v.as_str() {
            if s.chars().count() < 10 {
                push(
                    errors,
                    "field 'intended_behavior' must have at least 10 characters".to_string(),
                );
            }
        }
    }

    // kind enum.
    check_string_enum(obj, "kind", &["unit", "integration"], &push, errors);
    // status enum.
    check_string_enum(
        obj,
        "status",
        &[
            "pending",
            "written",
            "implemented",
            "rejected_lint",
            "quarantined",
            "surfaced_bug",
        ],
        &push,
        errors,
    );
    // source_of_unit enum (optional field).
    check_string_enum(
        obj,
        "source_of_unit",
        &["coverage_reviewer", "adversarial_reviewer", "fuzz_runner"],
        &push,
        errors,
    );

    // fuzzable must be a boolean when present.
    if let Some(v) = obj.get("fuzzable") {
        if !v.is_boolean() {
            push(errors, "field 'fuzzable' must be a boolean".to_string());
        }
    }

    // target_stub_path must be string or null when present.
    if let Some(v) = obj.get("target_stub_path") {
        if !(v.is_string() || v.is_null()) {
            push(
                errors,
                "field 'target_stub_path' must be a string or null".to_string(),
            );
        }
    }

    // round must be an integer >= -1 when present.
    if let Some(v) = obj.get("round") {
        if !(v.is_i64() || v.is_u64()) {
            push(errors, "field 'round' must be an integer".to_string());
        } else if v.as_i64().is_some_and(|n| n < -1) {
            push(errors, "field 'round' must be >= -1".to_string());
        }
    }
}

/// Validates a string enum field: when present, must be a string in `allowed`. A non-string
/// value (already reported as a type error elsewhere if it's a STRING_FIELD, but kind/status/
/// source_of_unit are not in STRING_FIELDS) is reported here as a single enum violation.
fn check_string_enum<F: Fn(&mut Vec<UnitError>, String)>(
    obj: &serde_json::Map<String, Value>,
    field: &str,
    allowed: &[&str],
    push: &F,
    errors: &mut Vec<UnitError>,
) {
    if let Some(v) = obj.get(field) {
        match v.as_str() {
            Some(s) if allowed.contains(&s) => {}
            _ => push(
                errors,
                format!("field '{field}' must be one of {allowed:?}"),
            ),
        }
    }
}

/// Coerces array-valued hint fields (preconditions/inputs/expected) of a SINGLE work-unit value
/// into "; "-joined scalar strings; non-array values pass through unchanged. Pure.
pub fn normalize_hint_fields(unit: &Value) -> Value {
    let mut out = unit.clone();
    if let Some(obj) = out.as_object_mut() {
        for field in ["preconditions", "inputs", "expected"] {
            if let Some(Value::Array(elements)) = obj.get(field) {
                let joined = elements
                    .iter()
                    .map(|e| {
                        e.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| e.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                obj.insert(field.to_string(), Value::String(joined));
            }
        }
    }
    out
}

/// Normalizes the hint fields of every work unit while preserving the input's top-level shape:
/// a `{"work_units":[...], ...}` wrapper keeps its sibling metadata (e.g. `scope_summary`) and a
/// bare array stays a bare array. Returns `None` if the input is neither shape. Pure.
pub fn normalize_preserving_shape(value: &Value) -> Option<Value> {
    let mut out = value.clone();
    match &mut out {
        Value::Array(units) => {
            for unit in units.iter_mut() {
                *unit = normalize_hint_fields(unit);
            }
            Some(out)
        }
        Value::Object(obj) => match obj.get_mut("work_units") {
            Some(Value::Array(units)) => {
                for unit in units.iter_mut() {
                    *unit = normalize_hint_fields(unit);
                }
                Some(out)
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let value: Value = crate::common::json_io::read_json_file(&args.work_units_file)?;

    if args.normalize {
        let normalized = normalize_preserving_shape(&value).ok_or_else(|| {
            anyhow::anyhow!(
                "work-units input is neither a JSON array nor a {{work_units:[...]}} wrapper"
            )
        })?;
        println!("{}", serde_json::to_string_pretty(&normalized)?);
        Ok(())
    } else {
        let report = validate_work_units(&value);
        println!("{}", serde_json::to_string_pretty(&report)?);
        if report.valid {
            Ok(())
        } else {
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_unit() -> Value {
        json!({
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "target_file": "src/commands/foo.rs",
            "target_symbol": "foo::bar",
            "kind": "unit",
            "intended_behavior": "a behavior long enough",
            "output_file_path": "src/commands/foo_tests.rs",
            "output_test_name": "test_foo_bar",
            "status": "pending",
            "round": 0
        })
    }

    // --- validate_work_units tests ---

    #[test]
    fn test_fully_valid_unit_validates_with_no_errors() {
        let input = json!([valid_unit()]);
        let report = validate_work_units(&input);
        assert!(report.valid);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn test_missing_required_field_is_invalid_and_names_field() {
        let mut unit = valid_unit();
        unit.as_object_mut().unwrap().remove("intended_behavior");
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1, "exactly one error expected, got: {:?}", report.errors);
        assert!(
            report.errors.iter().any(|e| e.message.contains("intended_behavior")),
            "expected an error referencing 'intended_behavior', got: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_array_hint_field_is_invalid_without_normalize() {
        let mut unit = valid_unit();
        unit["inputs"] = json!(["a", "b"]);
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1, "exactly one error expected, got: {:?}", report.errors);
        assert!(
            report.errors.iter().any(|e| e.message.contains("inputs")),
            "expected an error referencing 'inputs', got: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_invalid_kind_enum_is_invalid() {
        let mut unit = valid_unit();
        unit["kind"] = json!("bogus");
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1, "exactly one error expected, got: {:?}", report.errors);
        assert!(
            report.errors.iter().any(|e| e.message.contains("kind")),
            "expected an error referencing 'kind', got: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_invalid_status_enum_is_invalid() {
        let mut unit = valid_unit();
        unit["status"] = json!("done");
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1, "exactly one error expected, got: {:?}", report.errors);
        assert!(
            report.errors.iter().any(|e| e.message.contains("status")),
            "expected an error referencing 'status', got: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_invalid_source_of_unit_enum_is_invalid() {
        let mut unit = valid_unit();
        unit["source_of_unit"] = json!("orchestrator");
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1, "exactly one error expected, got: {:?}", report.errors);
        assert!(
            report.errors.iter().any(|e| e.message.contains("source_of_unit")),
            "expected an error referencing 'source_of_unit', got: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_round_below_minus_one_is_invalid_boundary() {
        // round -2 → invalid
        let mut unit_neg2 = valid_unit();
        unit_neg2["round"] = json!(-2);
        let report_neg2 = validate_work_units(&json!([unit_neg2]));
        assert!(!report_neg2.valid, "round -2 must be invalid");

        // round -1 → valid
        let mut unit_neg1 = valid_unit();
        unit_neg1["round"] = json!(-1);
        let report_neg1 = validate_work_units(&json!([unit_neg1]));
        assert!(report_neg1.valid, "round -1 must be valid");
        assert!(report_neg1.errors.is_empty(), "round -1 must have no errors");

        // round 0 → valid
        let report_zero = validate_work_units(&json!([valid_unit()]));
        assert!(report_zero.valid, "round 0 must be valid");
        assert!(report_zero.errors.is_empty(), "round 0 must have no errors");
    }

    #[test]
    fn test_intended_behavior_min_length_boundary() {
        // len 9 → invalid
        let mut unit_len9 = valid_unit();
        unit_len9["intended_behavior"] = json!("123456789");
        let report_len9 = validate_work_units(&json!([unit_len9]));
        assert!(!report_len9.valid, "intended_behavior of length 9 must be invalid");

        // len 0 → invalid
        let mut unit_empty = valid_unit();
        unit_empty["intended_behavior"] = json!("");
        let report_empty = validate_work_units(&json!([unit_empty]));
        assert!(!report_empty.valid, "empty intended_behavior must be invalid");

        // len 10 → valid
        let mut unit_len10 = valid_unit();
        unit_len10["intended_behavior"] = json!("1234567890");
        let report_len10 = validate_work_units(&json!([unit_len10]));
        assert!(report_len10.valid, "intended_behavior of length 10 must be valid");
        assert!(report_len10.errors.is_empty(), "intended_behavior of length 10 must have no errors");
    }

    #[test]
    fn test_unknown_field_is_invalid_additional_properties_false() {
        let mut unit = valid_unit();
        unit["extra_field"] = json!(true);
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1, "exactly one error expected, got: {:?}", report.errors);
        assert!(
            report.errors.iter().any(|e| e.message.contains("extra_field")),
            "expected an error referencing 'extra_field', got: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_accepts_both_bare_array_and_wrapper_object() {
        let unit = valid_unit();

        // bare array
        let bare = json!([unit.clone()]);
        let report_bare = validate_work_units(&bare);
        assert!(report_bare.valid, "bare array must be valid");
        assert!(report_bare.errors.is_empty(), "bare array must have no errors");

        // wrapper object
        let wrapper = json!({"work_units": [unit], "scope_summary": "x"});
        let report_wrapper = validate_work_units(&wrapper);
        assert!(report_wrapper.valid, "wrapper object must be valid");
        assert!(report_wrapper.errors.is_empty(), "wrapper object must have no errors");
    }

    #[test]
    fn test_empty_array_is_valid_zero_units() {
        // bare empty array
        let report_bare = validate_work_units(&json!([]));
        assert!(report_bare.valid);
        assert!(report_bare.errors.is_empty());

        // wrapper with empty inner array
        let report_wrapper = validate_work_units(&json!({"work_units": []}));
        assert!(report_wrapper.valid);
        assert!(report_wrapper.errors.is_empty());
    }

    #[test]
    fn test_multiple_defective_units_each_get_own_error() {
        let mut unit_a = valid_unit();
        unit_a.as_object_mut().unwrap().remove("intended_behavior");

        let mut unit_b = valid_unit();
        unit_b["status"] = json!("done");

        let input = json!([unit_a, unit_b]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert!(report.errors.len() >= 2, "expected at least 2 errors, got: {:?}", report.errors);

        // must have distinct index values
        let indices: Vec<usize> = report.errors.iter().map(|e| e.index).collect();
        assert!(
            indices.contains(&0),
            "must have an error for unit at index 0"
        );
        assert!(
            indices.contains(&1),
            "must have an error for unit at index 1"
        );

        // unit at index 0 had intended_behavior removed → its error must name that field
        assert!(
            report.errors.iter().any(|e| e.index == 0 && e.message.contains("intended_behavior")),
            "error for index 0 must reference 'intended_behavior', got: {:?}",
            report.errors
        );
        // unit at index 1 had status set to invalid value → its error must name that field
        assert!(
            report.errors.iter().any(|e| e.index == 1 && e.message.contains("status")),
            "error for index 1 must reference 'status', got: {:?}",
            report.errors
        );
    }

    // --- normalize_hint_fields tests ---

    #[test]
    fn test_normalize_joins_two_element_array_with_semicolon_space() {
        let mut unit = valid_unit();
        unit["inputs"] = json!(["a", "b"]);
        let result = normalize_hint_fields(&unit);
        assert_eq!(result["inputs"], json!("a; b"));
    }

    #[test]
    fn test_normalize_single_element_array_has_no_separator() {
        let mut unit = valid_unit();
        unit["inputs"] = json!(["a"]);
        let result = normalize_hint_fields(&unit);
        assert_eq!(result["inputs"], json!("a"));
    }

    #[test]
    fn test_normalize_empty_array_becomes_empty_string() {
        let mut unit = valid_unit();
        unit["inputs"] = json!([]);
        let result = normalize_hint_fields(&unit);
        assert_eq!(result["inputs"], json!(""));
    }

    #[test]
    fn test_normalize_scalar_string_passes_through_unchanged() {
        let mut unit = valid_unit();
        unit["inputs"] = json!("already a string");
        let result = normalize_hint_fields(&unit);
        assert_eq!(result["inputs"], json!("already a string"));
    }

    #[test]
    fn test_normalize_non_array_non_string_passes_through_unchanged() {
        let mut unit = valid_unit();
        unit["inputs"] = json!(42);
        let result = normalize_hint_fields(&unit);
        assert_eq!(result["inputs"], json!(42));
    }

    // --- normalize_hint_fields: new tests (6–8) ---

    #[test]
    fn test_normalize_coerces_all_three_hint_fields_and_preserves_other_fields() {
        let mut unit = valid_unit();
        unit["preconditions"] = json!(["p1", "p2"]);
        unit["inputs"] = json!(["a", "b"]);
        unit["expected"] = json!(["e1", "e2"]);
        let original_id = unit["id"].clone();
        let result = normalize_hint_fields(&unit);
        assert_eq!(result["preconditions"], json!("p1; p2"));
        assert_eq!(result["inputs"], json!("a; b"));
        assert_eq!(result["expected"], json!("e1; e2"));
        assert_eq!(result["id"], original_id, "non-hint field 'id' must survive unchanged");
    }

    #[test]
    fn test_normalize_preconditions_array_joined_independently() {
        let mut unit = valid_unit();
        unit["preconditions"] = json!(["x", "y"]);
        let result = normalize_hint_fields(&unit);
        assert_eq!(result["preconditions"], json!("x; y"));
    }

    #[test]
    fn test_normalize_expected_array_joined_independently() {
        let mut unit = valid_unit();
        unit["expected"] = json!(["x", "y"]);
        let result = normalize_hint_fields(&unit);
        assert_eq!(result["expected"], json!("x; y"));
    }

    // --- normalize_preserving_shape tests ---

    #[test]
    fn test_normalize_preserving_shape_keeps_wrapper_metadata() {
        let mut unit = valid_unit();
        unit["inputs"] = json!(["a", "b"]);
        let input = json!({ "work_units": [unit], "scope_summary": "diff against HEAD~3" });
        let result = normalize_preserving_shape(&input).unwrap();
        assert_eq!(result["scope_summary"], json!("diff against HEAD~3"));
        assert_eq!(result["work_units"][0]["inputs"], json!("a; b"));
    }

    #[test]
    fn test_normalize_preserving_shape_bare_array_stays_array() {
        let mut unit = valid_unit();
        unit["inputs"] = json!(["a", "b"]);
        let input = json!([unit]);
        let result = normalize_preserving_shape(&input).unwrap();
        assert!(result.is_array());
        assert_eq!(result[0]["inputs"], json!("a; b"));
    }

    #[test]
    fn test_normalize_preserving_shape_rejects_unsupported_shape() {
        assert!(normalize_preserving_shape(&json!({ "nope": 1 })).is_none());
        assert!(normalize_preserving_shape(&json!("scalar")).is_none());
    }

    // --- validate_work_units: wrong-type tests (9–11) ---

    #[test]
    fn test_wrong_type_round_string_is_invalid_naming_field() {
        let mut unit = valid_unit();
        unit["round"] = json!("0"); // string instead of integer
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert!(
            report.errors.iter().any(|e| e.message.contains("round")),
            "expected an error referencing 'round', got: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_wrong_type_id_number_is_invalid_naming_field() {
        let mut unit = valid_unit();
        unit["id"] = json!(42); // number instead of string
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert!(
            report.errors.iter().any(|e| e.message.contains("id")),
            "expected an error referencing 'id', got: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_wrong_type_fuzzable_string_is_invalid_naming_field() {
        let mut unit = valid_unit();
        unit["fuzzable"] = json!("yes"); // string instead of boolean
        let input = json!([unit]);
        let report = validate_work_units(&input);
        assert!(!report.valid);
        assert!(
            report.errors.iter().any(|e| e.message.contains("fuzzable")),
            "expected an error referencing 'fuzzable', got: {:?}",
            report.errors
        );
    }

    // --- validate_work_units: non-conforming top-level tests (12–13) ---

    #[test]
    fn test_top_level_non_array_non_wrapper_is_invalid_no_panic() {
        // object without "work_units" key
        let report_obj = validate_work_units(&json!({"foo": 1}));
        assert!(!report_obj.valid);

        // bare number
        let report_num = validate_work_units(&json!(7));
        assert!(!report_num.valid);

        // bare string
        let report_str = validate_work_units(&json!("x"));
        assert!(!report_str.valid);
    }

    #[test]
    fn test_malformed_wrapper_non_array_inner_is_invalid_no_panic() {
        // wrapper whose inner value is not an array
        let report = validate_work_units(&json!({"work_units": 42}));
        assert!(!report.valid);
    }

    #[test]
    fn test_target_stub_path_accepts_string_or_null_rejects_other() {
        // sub-case 1: string value — valid, no errors
        let mut unit_str = valid_unit();
        unit_str["target_stub_path"] = serde_json::json!("src/foo.rs");
        let report_str = validate_work_units(&json!([unit_str]));
        assert!(report_str.valid);
        assert!(report_str.errors.is_empty());

        // sub-case 2: JSON null present — valid, no errors
        let mut unit_null = valid_unit();
        unit_null["target_stub_path"] = serde_json::json!(null);
        let report_null = validate_work_units(&json!([unit_null]));
        assert!(report_null.valid);
        assert!(report_null.errors.is_empty());

        // sub-case 3: number — wrong type, must be invalid and name the field
        let mut unit_num = valid_unit();
        unit_num["target_stub_path"] = serde_json::json!(42);
        let report_num = validate_work_units(&json!([unit_num]));
        assert!(!report_num.valid);
        assert!(
            report_num.errors.iter().any(|e| e.message.contains("target_stub_path")),
            "expected an error referencing 'target_stub_path', got: {:?}",
            report_num.errors
        );
    }
}
