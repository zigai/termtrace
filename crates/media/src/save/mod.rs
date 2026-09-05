//! Atomic multi-format publication for one semantic observation.

use crate::{Error, Result};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read as _, Write as _},
    os::unix::{ffi::OsStrExt as _, fs::FileTypeExt as _},
    path::{Component, Path, PathBuf},
};

use termtrace_engine::{
    observation::state_text,
    replay::{inspect_many, inspect_with_formats},
};
use termtrace_evidence::{
    artifact::{
        BudgetWriter, commit_directory, create_private_directory, hash_regular_file, secure_file,
        sync_directory, write_secure,
    },
    event_log::{CanonicalBoundary, EventLog},
    session::{read_manifest, validate_completed_log},
};
use termtrace_model::{
    artifact::save::{
        SaveFormat, SaveManifest, SaveOutcome, SavedFile, ScreenshotBatchEntry,
        ScreenshotBatchManifest, SemanticImageResource, SemanticRasterProfile,
    },
    execution::event::Event,
    execution::session::{SessionCompletion, SessionManifest},
    terminal::observation::{
        CaptureQuality, Observation, ObservationFrame, ObservationIntegrity, ObservationResolution,
        ObservationSourceKind, StatePoint, TruthLayer,
    },
    terminal::replay::{StateInspection, StateSelector},
    value::ByteString,
};
use termtrace_render::{
    Error as RenderError, SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH, SEMANTIC_FONT_SIZE,
    SEMANTIC_PNG_RENDERER, SEMANTIC_SVG_RENDERER, SEMANTIC_SYMBOLS_BASELINE,
    SEMANTIC_SYMBOLS_FONT_SIZE, SEMANTIC_TEXT_BASELINE, raster::write_png, render_png, render_svg,
    semantic_font_set,
};
use termtrace_terminal::Format;

/// Re-read and verify one saved semantic artifact against its trusted creation result.
///
/// # Errors
///
/// Returns an error for a changed manifest, missing or extra members, unsafe paths,
/// non-regular files, or any byte length or digest mismatch.
pub fn verify_saved_artifact(directory: &Path, expected: &SaveManifest) -> Result<SaveManifest> {
    const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

    let manifest_path = directory.join("save-manifest.json");
    let metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|source| return filesystem_error(&manifest_path, source))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(save_error("saved artifact manifest is not a regular file"));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(save_error("saved artifact manifest exceeds 1 MiB"));
    }
    let manifest_file = fs::File::open(&manifest_path)
        .map_err(|source| return filesystem_error(&manifest_path, source))?;
    let mut manifest_bytes: Vec<u8> = Vec::new();
    manifest_file
        .take(MAX_MANIFEST_BYTES.saturating_add(1))
        .read_to_end(&mut manifest_bytes)
        .map_err(|source| return filesystem_error(&manifest_path, source))?;
    if u64::try_from(manifest_bytes.len()).unwrap_or(u64::MAX) > MAX_MANIFEST_BYTES {
        return Err(save_error("saved artifact manifest exceeds 1 MiB"));
    }
    let manifest: SaveManifest = serde_json::from_slice(&manifest_bytes).map_err(|source| {
        return Error::InvalidJson {
            path: manifest_path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    if &manifest != expected {
        return Err(save_error(
            "saved artifact manifest does not match its trusted creation result",
        ));
    }

    let mut expected_members: BTreeSet<String> = BTreeSet::new();
    for member in &manifest.files {
        let path = Path::new(&member.path);
        if path.components().count() != 1
            || !matches!(path.components().next(), Some(Component::Normal(_)))
            || !expected_members.insert(member.path.clone())
        {
            return Err(save_error("saved artifact contains an unsafe member path"));
        }
        let (byte_count, digest) = hash_regular_file(&directory.join(path))?;
        if byte_count != member.byte_count || digest != member.blake3 {
            return Err(save_error(format!(
                "saved artifact member {} failed integrity verification",
                member.path
            )));
        }
    }

    let mut observed_members: BTreeSet<String> = BTreeSet::new();
    for entry in
        fs::read_dir(directory).map_err(|source| return filesystem_error(directory, source))?
    {
        let entry = entry.map_err(|source| return filesystem_error(directory, source))?;
        let file_type = entry
            .file_type()
            .map_err(|source| return filesystem_error(entry.path(), source))?;
        if file_type.is_dir()
            || file_type.is_symlink()
            || file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
        {
            return Err(save_error("saved artifact contains a non-regular member"));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| return save_error("saved artifact contains a non-UTF-8 member name"))?;
        if name != "save-manifest.json" {
            observed_members.insert(name);
        }
    }
    if observed_members != expected_members {
        return Err(save_error(
            "saved artifact member inventory does not match its manifest",
        ));
    }
    return Ok(manifest);
}

/// Resolve one state and atomically publish selected representations.
///
/// # Errors
///
/// Returns an error for invalid/incomplete evidence, unsupported hidden state,
/// existing destination, filesystem failure, or inconsistent state resolution.
pub fn save(
    session: &Path,
    destination: &Path,
    point: StatePoint,
    formats: &[SaveFormat],
    include_hidden: bool,
    max_bytes: Option<u64>,
) -> Result<SaveOutcome> {
    validate_formats(formats)?;
    let canonical = session
        .canonicalize()
        .map_err(|source| return filesystem_error(session, source))?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let source_manifest = read_manifest(&canonical)?;
    let selector = state_selector(&log, &point)?;
    let engine_formats: Vec<Format> = formats
        .iter()
        .filter_map(|format| match format {
            SaveFormat::Vt => return Some(Format::Vt),
            SaveFormat::Html => return Some(Format::Html),
            _ => return None,
        })
        .collect();
    let (inspection, rendered) = inspect_with_formats(&log, selector, &engine_formats)?;
    if !inspection.record.presented && !include_hidden {
        return Err(save_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    let observation = observation_from_inspection(
        &canonical,
        &source_manifest,
        point.clone(),
        &completion,
        &inspection,
    )?;

    let parent = destination
        .parent()
        .ok_or_else(|| return save_error("save path has no parent"))?;
    let staging = staging_path(destination, "save")?;
    create_private_directory(&staging)?;
    let publish = (|| -> Result<SaveManifest> {
        let mut files: Vec<SavedFile> = Vec::with_capacity(formats.len());
        let mut remaining_bytes = max_bytes.unwrap_or(u64::MAX);
        for format in formats {
            files.push(save_rendered_file(
                *format,
                &observation,
                &inspection,
                &rendered,
                &staging,
                &mut remaining_bytes,
            )?);
        }
        let save_manifest = SaveManifest {
            format: "termtrace-save".to_owned(),
            version: 4,
            session_id: source_manifest.session_id,
            canonical_session: ByteString::from(canonical.as_os_str().as_bytes()),
            state_point: point,
            state_id: inspection.record.state_id,
            state_hash: inspection.record.state_hash,
            source_final_event_hash: completion.final_event_hash,
            sensitivity: "terminal_screen_may_contain_secrets".to_owned(),
            files,
            semantic_image_resources: if formats
                .iter()
                .any(|format| matches!(format, SaveFormat::Svg | SaveFormat::Png))
            {
                semantic_image_resources()?
            } else {
                Vec::new()
            },
        };
        let manifest_path = staging.join("save-manifest.json");
        let mut output = secure_file(&manifest_path)?;
        {
            let mut writer = BudgetWriter::new(&mut output, remaining_bytes);
            serde_json::to_writer_pretty(&mut writer, &save_manifest).map_err(|source| {
                return Error::InvalidJson {
                    path: manifest_path.clone(),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;
            writer
                .write_all(b"\n")
                .map_err(|source| return filesystem_error(&manifest_path, source))?;
        }
        output
            .sync_all()
            .map_err(|source| return filesystem_error(&manifest_path, source))?;
        sync_directory(&staging)?;
        return Ok(save_manifest);
    })();
    let save_manifest = match publish {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    return Ok(SaveOutcome {
        directory: destination.to_path_buf(),
        manifest: save_manifest,
    });
}

/// Atomically publish several exact semantic states under one batch directory.
///
/// # Errors
///
/// Returns the same errors as [`save`] and rejects empty or non-image batches.
pub fn save_screenshot_batch(
    session: &Path,
    destination: &Path,
    points: &[StatePoint],
    format: SaveFormat,
    include_hidden: bool,
) -> Result<ScreenshotBatchManifest> {
    if points.is_empty() || !matches!(format, SaveFormat::Svg | SaveFormat::Png) {
        return Err(save_error(
            "screenshot batches require at least one point and an SVG or PNG format",
        ));
    }
    let canonical = session
        .canonicalize()
        .map_err(|source| return filesystem_error(session, source))?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let source_manifest = read_manifest(&canonical)?;
    let selectors: Vec<StateSelector> = points
        .iter()
        .map(|point| return state_selector(&log, point))
        .collect::<Result<Vec<_>>>()?;
    let inspections = inspect_many(&log, &selectors)?;
    if !include_hidden
        && inspections
            .iter()
            .any(|inspection| return !inspection.record.presented)
    {
        return Err(save_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }

    let parent = destination
        .parent()
        .ok_or_else(|| return save_error("batch path has no parent"))?;
    let staging = staging_path(destination, "batch")?;
    create_private_directory(&staging)?;
    let publish = (|| -> Result<ScreenshotBatchManifest> {
        let (filename, media_type, renderer) = match format {
            SaveFormat::Svg => ("screen.svg", "image/svg+xml", SEMANTIC_SVG_RENDERER),
            SaveFormat::Png => ("screen.png", "image/png", SEMANTIC_PNG_RENDERER),
            _ => return Err(save_error("batch format must be SVG or PNG")),
        };
        let mut entries: Vec<ScreenshotBatchEntry> = Vec::with_capacity(points.len());
        for (index, (point, inspection)) in points.iter().zip(inspections).enumerate() {
            let directory = format!("state-{index:04}");
            let entry_directory = staging.join(&directory);
            create_private_directory(&entry_directory)?;
            let bytes = match format {
                SaveFormat::Svg => render_svg(&inspection.state).map_err(render_error)?,
                SaveFormat::Png => render_png(&inspection.state).map_err(render_error)?,
                _ => return Err(save_error("batch format must be SVG or PNG")),
            };
            write_secure(&entry_directory.join(filename), &bytes)?;
            let save_manifest = SaveManifest {
                format: "termtrace-save".to_owned(),
                version: 4,
                session_id: source_manifest.session_id,
                canonical_session: ByteString::from(canonical.as_os_str().as_bytes()),
                state_point: point.clone(),
                state_id: inspection.record.state_id,
                state_hash: inspection.record.state_hash,
                source_final_event_hash: completion.final_event_hash.clone(),
                sensitivity: "terminal_screen_may_contain_secrets".to_owned(),
                files: vec![SavedFile {
                    path: filename.to_owned(),
                    media_type: media_type.to_owned(),
                    byte_count: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                    blake3: blake3::hash(&bytes).to_hex().to_string(),
                    truth_layer: Some("semantic_rendering".to_owned()),
                    renderer: Some(renderer.to_owned()),
                }],
                semantic_image_resources: semantic_image_resources()?,
            };
            let manifest_path = entry_directory.join("save-manifest.json");
            write_secure(
                &manifest_path,
                &pretty_json(&save_manifest, &manifest_path)?,
            )?;
            sync_directory(&entry_directory)?;
            entries.push(ScreenshotBatchEntry {
                image: format!("{directory}/{filename}"),
                directory,
                manifest: save_manifest,
            });
        }
        let manifest = ScreenshotBatchManifest {
            format: "termtrace-screenshot-batch".to_owned(),
            version: 3,
            image_format: format,
            entries,
        };
        let manifest_path = staging.join("batch-manifest.json");
        write_secure(&manifest_path, &pretty_json(&manifest, &manifest_path)?)?;
        sync_directory(&staging)?;
        return Ok(manifest);
    })();
    let manifest = match publish {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    return Ok(manifest);
}
fn observation_from_inspection(
    session: &Path,
    source_manifest: &SessionManifest,
    point: StatePoint,
    completion: &SessionCompletion,
    inspection: &StateInspection,
) -> Result<Observation> {
    let frame = ObservationFrame {
        viewport_text: state_text(&inspection.state),
        cols: inspection.state.cols,
        rows: inspection.state.rows,
        cursor_column: inspection.state.cursor.column,
        cursor_row: inspection.state.cursor.row,
        cursor_visible: inspection.state.cursor.visible,
        modes: inspection.state.modes.clone(),
    };
    let frame_bytes = serde_json::to_vec(&frame).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<observation-frame>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return Ok(Observation {
        source_kind: ObservationSourceKind::CompletedSession,
        session_id: Some(source_manifest.session_id),
        canonical_session: Some(ByteString::from(session.as_os_str().as_bytes())),
        source_completed: true,
        state_point: point,
        state_id: Some(inspection.record.state_id),
        state_hash: inspection.record.state_hash.clone(),
        event_seq: inspection.record.event_seq,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        presented: inspection.record.presented,
        boundary: CanonicalBoundary {
            next_event_seq: completion.event_count,
            event_hash: completion.final_event_hash.clone(),
            output_bytes: completion.output_bytes,
        },
        capture_quality: CaptureQuality::Completed,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::CanonicalChained,
        resolution: ObservationResolution::CanonicalReplay,
        frame,
        payload_hash: blake3::hash(&frame_bytes).to_hex().to_string(),
    });
}

fn save_rendered_file(
    format: SaveFormat,
    observation: &Observation,
    inspection: &StateInspection,
    rendered: &[(Format, Vec<u8>)],
    staging: &Path,
    remaining_bytes: &mut u64,
) -> Result<SavedFile> {
    let (path, media_type, truth_layer, renderer) = match format {
        SaveFormat::Txt => ("screen.txt", "text/plain; charset=utf-8", None, None),
        SaveFormat::Json => ("observation.json", "application/json", None, None),
        SaveFormat::DeepJson => ("semantic-state.json", "application/json", None, None),
        SaveFormat::Vt => (
            "screen.vt",
            "application/vnd.termtrace.vt",
            Some("semantic"),
            Some("pinned-libghostty-vt-formatter"),
        ),
        SaveFormat::Html => (
            "screen.html",
            "text/html; charset=utf-8",
            Some("semantic"),
            Some("pinned-libghostty-vt-formatter"),
        ),
        SaveFormat::Svg => (
            "screen.svg",
            "image/svg+xml",
            Some("semantic_rendering"),
            Some(SEMANTIC_SVG_RENDERER),
        ),
        SaveFormat::Png => (
            "screen.png",
            "image/png",
            Some("semantic_rendering"),
            Some(SEMANTIC_PNG_RENDERER),
        ),
    };
    let target = staging.join(path);
    let mut output = secure_file(&target)?;
    {
        let mut writer = BudgetWriter::new(&mut output, *remaining_bytes);
        match format {
            SaveFormat::Txt => writer
                .write_all(observation.frame.viewport_text.as_bytes())
                .map_err(|source| return filesystem_error(&target, source))?,
            SaveFormat::Json => {
                serde_json::to_writer_pretty(&mut writer, observation).map_err(|source| {
                    return Error::InvalidJson {
                        path: target.clone(),
                        json_path: "$".to_owned(),
                        source,
                    };
                })?;
                writer
                    .write_all(b"\n")
                    .map_err(|source| return filesystem_error(&target, source))?;
            }
            SaveFormat::DeepJson => {
                serde_json::to_writer_pretty(&mut writer, inspection).map_err(|source| {
                    return Error::InvalidJson {
                        path: target.clone(),
                        json_path: "$".to_owned(),
                        source,
                    };
                })?;
                writer
                    .write_all(b"\n")
                    .map_err(|source| return filesystem_error(&target, source))?;
            }
            SaveFormat::Vt => writer
                .write_all(rendered_format(rendered, Format::Vt)?)
                .map_err(|source| return filesystem_error(&target, source))?,
            SaveFormat::Html => writer
                .write_all(rendered_format(rendered, Format::Html)?)
                .map_err(|source| return filesystem_error(&target, source))?,
            SaveFormat::Svg => writer
                .write_all(&render_svg(&inspection.state).map_err(render_error)?)
                .map_err(|source| return filesystem_error(&target, source))?,
            SaveFormat::Png => write_png(
                &inspection.state,
                SemanticRasterProfile::Supersampled2x,
                &mut writer,
            )
            .map_err(render_error)?,
        }
        *remaining_bytes = writer.remaining();
    }
    output
        .sync_all()
        .map_err(|source| return filesystem_error(&target, source))?;
    let (byte_count, blake3) = hash_regular_file(&target)?;
    return Ok(SavedFile {
        path: path.to_owned(),
        media_type: media_type.to_owned(),
        byte_count,
        blake3,
        truth_layer: truth_layer.map(str::to_owned),
        renderer: renderer.map(str::to_owned),
    });
}

fn validate_formats(formats: &[SaveFormat]) -> Result<()> {
    if formats.is_empty() {
        return Err(save_error("at least one save format is required"));
    }
    if formats.iter().enumerate().any(|(index, format)| {
        return formats
            .iter()
            .skip(index + 1)
            .any(|later| return later == format);
    }) {
        return Err(save_error("save formats must not be repeated"));
    }
    return Ok(());
}

fn state_selector(log: &EventLog, point: &StatePoint) -> Result<StateSelector> {
    match point {
        StatePoint::LatestPresented => return Ok(StateSelector::LatestPresented),
        StatePoint::StateId { state_id } => return Ok(StateSelector::StateId(*state_id)),
        StatePoint::StateHash { state_hash } => {
            return Ok(StateSelector::StateHash(state_hash.clone()));
        }
        StatePoint::EventSeq { event_seq } => return Ok(StateSelector::EventSeq(*event_seq)),
        StatePoint::MonotonicNs { monotonic_ns } => {
            return Ok(StateSelector::MonotonicNs(*monotonic_ns));
        }
        StatePoint::Marker { label } => {
            let mut matches = log.events.iter().filter(|record| {
                return matches!(&record.event, Event::Marker { label: value, .. } if value == label)
            });
            let event_seq = matches
                .next()
                .map(|record| return record.seq)
                .ok_or_else(|| return save_error("marker does not exist"))?;
            if matches.next().is_some() {
                return Err(save_error("marker label is ambiguous"));
            }
            return Ok(StateSelector::EventSeq(event_seq));
        }
    }
}

fn semantic_image_resources() -> Result<Vec<SemanticImageResource>> {
    let font_set = semantic_font_set().map_err(render_error)?;
    return Ok(vec![SemanticImageResource {
        fonts: font_set.resources().cloned().collect(),
        font_set_blake3: font_set.font_set_blake3().to_owned(),
        fallback_policy: "style_face_then_symbols_nerd_font_then_missing_glyph_box".to_owned(),
        raster_profile: SemanticRasterProfile::Supersampled2x,
        font_size: SEMANTIC_FONT_SIZE,
        text_baseline: SEMANTIC_TEXT_BASELINE,
        symbols_font_size: SEMANTIC_SYMBOLS_FONT_SIZE,
        symbols_baseline: SEMANTIC_SYMBOLS_BASELINE,
        cell_width: SEMANTIC_CELL_WIDTH,
        cell_height: SEMANTIC_CELL_HEIGHT,
        padding: 0,
        background_policy: "opaque_terminal_background".to_owned(),
    }]);
}

fn pretty_json(value: &impl serde::Serialize, path: &Path) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    bytes.push(b'\n');
    return Ok(bytes);
}

fn rendered_format(rendered: &[(Format, Vec<u8>)], format: Format) -> Result<&[u8]> {
    return rendered
        .iter()
        .find(|(candidate, _)| return *candidate == format)
        .map(|(_, bytes)| return bytes.as_slice())
        .ok_or_else(|| return save_error("requested semantic format was not captured"));
}

fn staging_path(destination: &Path, kind: &str) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| return save_error(format!("{kind} path has no parent")))?;
    let name = destination
        .file_name()
        .ok_or_else(|| return save_error(format!("{kind} path has no basename")))?;
    return Ok(parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::now_v7()
    )));
}

fn render_error(error: RenderError) -> Error {
    return save_error(format!("semantic rendering failed: {error}"));
}

fn filesystem_error(path: impl Into<PathBuf>, source: io::Error) -> Error {
    return Error::Filesystem {
        path: path.into(),
        source,
    };
}

fn save_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
