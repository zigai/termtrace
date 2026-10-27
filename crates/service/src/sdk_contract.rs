//! Schema-owned SDK codecs, separate from canonical evidence serialization.
use std::time::Duration;
use termtrace_model::{artifact::sdk::MediaArtifactResult, terminal::screen::ScreenRegion};

pub mod control;
pub mod diagnostics;
pub mod types;

/// Decode only the versioned portable artifact schema, never arbitrary evidence JSON.
/// # Errors
/// Rejects malformed JSON, decimal numbers, `Base64`, and envelope invariants.
pub fn decode_media_artifact(json: &str) -> Result<MediaArtifactResult, String> {
    let result: MediaArtifactResult =
        serde_json::from_str(json).map_err(|e| return e.to_string())?;
    result.validate()?;

    return Ok(result);
}

/// Encode without rewriting canonical manifests or member bytes.
/// # Errors
/// Rejects invalid envelope invariants.
pub fn encode_media_artifact(result: &MediaArtifactResult) -> Result<String, String> {
    result.validate()?;

    return serde_json::to_string(result).map_err(|e| return e.to_string());
}

/// Convert explicitly named milliseconds. Fractions are allowed, negatives/non-finite values are not.
/// # Errors
/// Rejects invalid or unrepresentable durations.
pub fn timeout_from_ms(timeout_ms: f64) -> Result<Duration, String> {
    return Duration::try_from_secs_f64(timeout_ms / 1000.0).map_err(|e| return e.to_string());
}

/// Validate half-open bounds without silently saturating reversed regions.
/// # Errors
/// Rejects empty, reversed, or out-of-viewport regions.
pub fn validate_region(region: ScreenRegion, rows: u16, cols: u16) -> Result<ScreenRegion, String> {
    if region.top >= region.bottom
        || region.left >= region.right
        || region.bottom > rows
        || region.right > cols
    {
        return Err("invalid half-open screen region".into());
    }
    return Ok(region);
}

#[cfg(test)]
mod tests {
    use super::*;
    use termtrace_model::artifact::sdk::{DecimalU128, DecimalU64};

    #[test]
    fn exact_numbers_and_units() {
        for n in [9_007_199_254_740_993, u64::MAX] {
            let json = serde_json::to_string(&DecimalU64(n)).unwrap();
            assert_eq!(serde_json::from_str::<DecimalU64>(&json).unwrap().0, n);
        }

        let max = serde_json::to_string(&DecimalU128(u128::MAX)).unwrap();
        assert_eq!(
            serde_json::from_str::<DecimalU128>(&max).unwrap().0,
            u128::MAX
        );

        for bad in [
            "-1",
            "1.5",
            "9007199254740993",
            "\"01\"",
            "\"18446744073709551616\"",
            "\"-1\"",
        ] {
            assert!(serde_json::from_str::<DecimalU64>(bad).is_err());
        }

        assert_eq!(
            timeout_from_ms(1250.0).unwrap(),
            Duration::from_millis(1250)
        );

        for ms in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(timeout_from_ms(ms).is_err());
        }

        assert!(validate_region(
            ScreenRegion {
                top: 2,
                bottom: 1,
                left: 0,
                right: 1
            },
            24,
            80
        )
        .is_err());
    }

    #[test]
    fn shared_negative_vectors() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/conformance/sdk/cases/portable-invalid.json"
        ))
        .unwrap();

        let original: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/conformance/sdk/cases/portable.json"
        ))
        .unwrap();

        for case in fixture["cases"].as_array().unwrap() {
            let mut value = original.clone();
            let pointer = case["path"].as_str().unwrap();
            let (parent, key) = pointer.rsplit_once('/').unwrap();

            let object = value.pointer_mut(parent).unwrap().as_object_mut().unwrap();
            if case["absent"] == true {
                object.remove(key);
            } else {
                object.insert(key.into(), case["value"].clone());
            }

            assert!(
                serde_json::from_value::<MediaArtifactResult>(value.clone()).is_err(),
                "plain serde accepted {}",
                case["id"]
            );
            assert!(
                decode_media_artifact(&value.to_string()).is_err(),
                "negative vector {}",
                case["id"]
            );
        }
    }

    #[test]
    fn envelope_vectors() {
        let json = include_str!("../../../fixtures/conformance/sdk/cases/portable.json");
        let value = decode_media_artifact(json).unwrap();
        assert_eq!(
            decode_media_artifact(&encode_media_artifact(&value).unwrap()).unwrap(),
            value
        );

        for (from, to) in [
            ("\"version\": 1", "\"version\": 2"),
            ("manifest.json", "../manifest.json"),
            ("L3RtcC9kZW1v", "?"),
            ("9007199254740993", "18446744073709551616"),
            ("native_replay", "presentation_output"),
        ] {
            assert!(
                decode_media_artifact(&json.replace(from, to)).is_err(),
                "{from} -> {to}"
            );
        }
    }
}
