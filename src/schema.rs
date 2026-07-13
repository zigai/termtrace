//! Deterministic JSON Schema generation for every public artifact envelope.

use std::path::{Path, PathBuf};

use schemars::{JsonSchema, schema_for};

use crate::{
    Error, Result,
    control::{ControlRequest, ControlResponse},
    event::{EventLogHeader, EventRecord},
    event_log::RecoveryReport,
    inspection::WaitOutcome,
    playback::PlaybackTiming,
    replay::{ReplayManifest, StateIndexEntry, StateInspection, StateRecord},
    session::{SessionCompletion, SessionManifest},
    visual::{VideoMapEntry, VisualManifest},
};

/// One generated schema file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedSchema {
    /// Repository-relative filename.
    pub filename: &'static str,
    /// Pretty JSON with a trailing newline.
    pub json: Vec<u8>,
}

/// Generate all stable public JSON schemas.
///
/// # Errors
///
/// Returns an error only if an in-memory schema cannot serialize as JSON.
pub fn generate() -> Result<Vec<GeneratedSchema>> {
    Ok(vec![
        schema::<EventLogHeader>("event-log-header.schema.json")?,
        schema::<EventRecord>("event-record.schema.json")?,
        schema::<ControlRequest>("control-request.schema.json")?,
        schema::<ControlResponse>("control-response.schema.json")?,
        schema::<SessionManifest>("session.schema.json")?,
        schema::<SessionCompletion>("completion.schema.json")?,
        schema::<StateRecord>("state-record.schema.json")?,
        schema::<StateIndexEntry>("state-index-entry.schema.json")?,
        schema::<StateInspection>("state-inspection.schema.json")?,
        schema::<ReplayManifest>("replay-manifest.schema.json")?,
        schema::<VisualManifest>("visual-manifest.schema.json")?,
        schema::<VideoMapEntry>("video-map-entry.schema.json")?,
        schema::<RecoveryReport>("recovery-report.schema.json")?,
        schema::<WaitOutcome>("wait-outcome.schema.json")?,
        schema::<PlaybackTiming>("playback-timing.schema.json")?,
    ])
}

/// Write all schemas to a directory, replacing generated files by name.
///
/// # Errors
///
/// Returns an error when the directory or any schema file cannot be written.
pub fn write_all(directory: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let directory = directory.as_ref();
    fs_err::create_dir_all(directory).map_err(|source| Error::Filesystem {
        path: directory.to_path_buf(),
        source,
    })?;
    generate()?
        .into_iter()
        .map(|schema| {
            let path = directory.join(schema.filename);
            fs_err::write(&path, schema.json).map_err(|source| Error::Filesystem {
                path: path.clone(),
                source,
            })?;
            Ok(path)
        })
        .collect()
}

fn schema<T: JsonSchema>(filename: &'static str) -> Result<GeneratedSchema> {
    let mut value = serde_json::to_value(schema_for!(T)).map_err(|source| Error::InvalidJson {
        path: PathBuf::from(filename),
        json_path: "$".to_owned(),
        source,
    })?;
    value
        .as_object_mut()
        .expect("schema root is an object")
        .insert(
            "$id".to_owned(),
            serde_json::Value::String(format!(
                "https://github.com/zigai/termtrace/schemas/{filename}"
            )),
        );
    let mut json = serde_json::to_vec_pretty(&value).map_err(|source| Error::InvalidJson {
        path: PathBuf::from(filename),
        json_path: "$".to_owned(),
        source,
    })?;
    json.push(b'\n');
    Ok(GeneratedSchema { filename, json })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_schemas_match_generated_types() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
        for generated in generate().expect("generate schemas") {
            let path = directory.join(generated.filename);
            let schema: serde_json::Value =
                serde_json::from_slice(&generated.json).expect("schema JSON");
            assert!(
                jsonschema::meta::is_valid(&schema),
                "invalid JSON Schema {}",
                generated.filename
            );
            assert_eq!(
                fs_err::read(&path).unwrap_or_else(|error| {
                    panic!("read checked-in schema {}: {error}", path.display())
                }),
                generated.json,
                "regenerate schemas with `termtrace schemas --out schemas`"
            );
        }
    }
}
