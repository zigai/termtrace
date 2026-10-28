//! One normalized composition, multiple opaque deliveries, immutable evidence.

use super::{
    ffmpeg::{command_version, probe_video_file, run_logged_command},
    mapping::{read_semantic_frame_spans, write_presentation_map},
    pipeline::compose_pixels,
    plan_error,
    planning::{hash_file, validate_planned_source},
    validation::presentation_source_identity_hash,
    PresentationPlan, PresentationSource,
};
use crate::{
    capability::derivative_capability, error::finish_with_cleanup,
    native_capture::validate_visual_manifest_binding, Error, Result,
};
use serde::Serialize;
#[cfg(test)]
use std::{env, fs::Permissions, path::PathBuf, sync::Barrier, thread};
use std::{
    io::{self, BufReader, ErrorKind as IoErrorKind, Read, Write},
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use termtrace_evidence::{
    artifact::{
        collect_artifact_members, commit_directory, create_private_directory, finish_writer,
        secure_writer, verify_artifact_members, write_secure,
    },
    event_log::EventLogCursor,
    session::{read_manifest, validate_completed_session},
    Error as EvidenceError,
};
use termtrace_model::{
    artifact::{
        derivative::{
            gif_boundary_centiseconds, DerivativeExportOptions, DerivativeFormat,
            DerivativeManifest, DerivativeOutput, GifFrameTiming, GifLoop,
        },
        sdk::{
            ArtifactManifestRef, ArtifactProductKind, ArtifactProvenance, ArtifactSourceKind,
            ArtifactSourceRef, DecimalU64, MediaArtifactResult, SdkArtifactMember,
            SdkCanonicalBoundary,
        },
        visual::VisualManifest,
    },
    execution::layout::SessionLayout,
    protocol::capability::CapabilityStatus,
    value::ByteString,
};

fn filesystem(path: &Path, source: io::Error) -> Error {
    return Error::Filesystem {
        path: path.into(),
        source,
    };
}

fn json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.into(),
            json_path: "$".into(),
            source,
        };
    })?;

    return Ok(write_secure(path, &bytes)?);
}

/// Verify actual pixels against explicit source truth and completed canonical identity.
///
/// # Errors
/// No original/replay/semantic substitution, mismatched capture, VFR, or changed evidence is accepted.
pub fn verify_source_selection(
    plan: &PresentationPlan,
    session: &Path,
    kind: ArtifactSourceKind,
) -> Result<ArtifactSourceRef> {
    let completion = validate_completed_session(session)?;
    validate_planned_source(&plan.source)?;
    match (&plan.source, kind) {
        (
            PresentationSource::Semantic { plan: semantic },
            ArtifactSourceKind::SemanticRendering,
        ) => {
            if semantic.session_id != completion.session_id
                || semantic.source_event_hash != completion.final_event_hash
                || semantic.fps != plan.fps
                || semantic
                    .session
                    .canonicalize()
                    .map_err(|e| return filesystem(&semantic.session, e))?
                    != session
                        .canonicalize()
                        .map_err(|e| return filesystem(session, e))?
            {
                return Err(plan_error(
                    "semantic source does not match selected canonical session",
                ));
            }
        }
        (
            PresentationSource::Native { video, video_hash },
            ArtifactSourceKind::NativeReplay | ArtifactSourceKind::OriginalExecution,
        ) => {
            let directory = video
                .parent()
                .ok_or_else(|| return plan_error("native source has no capture directory"))?;

            let path = directory.join("manifest.json");
            let bytes = fs_err::read(&path).map_err(|e| return filesystem(&path, e))?;
            let manifest: VisualManifest = serde_json::from_slice(&bytes).map_err(|source| {
                return Error::InvalidJson {
                    path,
                    json_path: "$".into(),
                    source,
                };
            })?;

            verify_artifact_members(directory, &manifest.members)?;

            let canonical = read_manifest(session)?;
            let header = EventLogCursor::open(SessionLayout::new(session).event_log())?;
            validate_visual_manifest_binding(
                &manifest,
                canonical
                    .ghostty_visual
                    .as_ref()
                    .map(|p| return p.effective_config_hash.as_str()),
                &header.header().colors.source,
            )?;

            let origin =
                super::planning::native_capture_clock_origin(&manifest, &canonical, directory)?;

            let frames = super::planning::native_cfr_frame_count(&manifest)?;
            for clip in &plan.clips {
                let elapsed_start = u128::try_from(
                    (i128::from(clip.source_range.start_monotonic_ns) - origin).max(0),
                )
                .map_err(|_error| return plan_error("source clock overflow"))?;
                let elapsed_end = u128::try_from(
                    (i128::from(clip.source_range.end_monotonic_ns) - origin).max(0),
                )
                .map_err(|_error| return plan_error("source clock overflow"))?;

                let start = u64::try_from(elapsed_start * u128::from(plan.fps) / 1_000_000_000)
                    .map_err(|_error| return plan_error("source frame overflow"))?;
                let end =
                    u64::try_from((elapsed_end * u128::from(plan.fps)).div_ceil(1_000_000_000))
                        .map_err(|_error| return plan_error("source frame overflow"))?;

                // A raw native-frame exclusive selector remains exact even when its
                // rational boundary rounds upward to an integer canonical nanosecond.
                let exact_native_boundary = (elapsed_end * u128::from(plan.fps))
                    .abs_diff(u128::from(clip.source_end_frame) * 1_000_000_000)
                    <= u128::from(plan.fps);
                if clip.source_start_frame != start
                    || (clip.source_end_frame != end && !exact_native_boundary)
                    || clip.source_end_frame > frames
                {
                    return Err(plan_error(
                        "normalized source frames do not match verified capture clock",
                    ));
                }
            }

            let expected = if kind == ArtifactSourceKind::NativeReplay {
                "video"
            } else {
                "original_execution"
            };

            if manifest.capture_kind != expected {
                return Err(plan_error(format!(
                    "explicit {kind:?} requires {expected} pixels, found {}",
                    manifest.capture_kind
                )));
            }

            if manifest.fps == 0 {
                return Err(plan_error("variable-rate original pixels are unsupported; a verified CFR capture is required"));
            }

            if manifest.session_id != completion.session_id
                || manifest.source_event_hash.as_deref()
                    != Some(completion.final_event_hash.as_str())
                || manifest.fps != plan.fps
                || manifest.width != plan.source_width
                || manifest.height != plan.source_height
                || video
                    .file_name()
                    .is_none_or(|name| return name != "session.mkv")
                || !manifest
                    .members
                    .iter()
                    .any(|m| return m.path == "session.mkv" && m.blake3 == *video_hash)
            {
                return Err(plan_error("native source identity, timing or geometry does not match canonical source and normalized plan"));
            }
        }
        _ => {
            return Err(plan_error(
                "explicit source selection does not match planned pixels; no implicit fallback",
            ))
        }
    }

    return Ok(ArtifactSourceRef {
        session_id: completion.session_id,
        session_manifest_hash: completion.session_manifest_hash,
        boundary: SdkCanonicalBoundary {
            next_event_seq: DecimalU64(completion.event_count),
            event_hash: completion.final_event_hash,
            output_bytes: DecimalU64(completion.output_bytes),
        },
    });
}

/// Export GIF/VP9 `WebM` through a single private lossless RGB composition.
///
/// New destinations are always no-replace. The legacy MP4 API remains replacing.
/// The returned task-01 envelope references the authoritative derivative manifest.
///
/// # Errors
/// Rejects invalid options, unavailable encoders, changed sources, invalid timing,
/// occupied paths, encoding/probe/decode failures, and publication/cleanup failures.
pub fn export_derivatives(
    plan: &PresentationPlan,
    session: &Path,
    destination: impl AsRef<Path>,
    options: &DerivativeExportOptions,
) -> Result<MediaArtifactResult> {
    return export_derivatives_with_cancel(
        plan,
        session,
        destination.as_ref(),
        options,
        &AtomicBool::new(false),
    );
}

/// Cancellable derivative export. Cancellation kills/reaps an active delivery encoder group.
/// Existing semantic-source materialization completes before cancellation is checked again;
/// cancellation is not a claim of bounded interruption of that source adapter.
///
/// # Errors
/// Same as `export_derivatives`; cancellation before publication leaves no published directory.
pub fn export_derivatives_with_cancel(
    plan: &PresentationPlan,
    session: &Path,
    destination: &Path,
    options: &DerivativeExportOptions,
    cancelled: &AtomicBool,
) -> Result<MediaArtifactResult> {
    options.validate().map_err(plan_error)?;

    super::pipeline::validate_normalized_frames(plan)?;
    if plan.fps == 0
        || plan.output_frame_count == 0
        || plan.clips.is_empty()
        || plan.output_frame_count > u64::from(plan.fps) * 86_400
    {
        return Err(plan_error(
            "derivatives require a nonempty CFR plan of at most 24 hours",
        ));
    }

    if options
        .formats
        .iter()
        .any(|f| matches!(f, DerivativeFormat::Gif { .. }))
    {
        gif_boundary_centiseconds(plan.output_frame_count, plan.fps).map_err(plan_error)?;
    }

    let source = verify_source_selection(plan, session, options.source_kind)?;

    if fs_err::symlink_metadata(destination).is_ok() {
        return Err(Error::ArtifactExists {
            path: destination.into(),
        });
    }

    if cancelled.load(Ordering::Acquire) {
        return Err(plan_error("derivative export cancelled"));
    }

    for format in &options.formats {
        let capability = derivative_capability(
            *format,
            !plan.subtitles.is_empty(),
            !plan.annotations.is_empty()
                || plan.clips.iter().any(|c| return c.caption.is_some())
                || plan.layout.branding.is_some(),
        );
        if capability.status != CapabilityStatus::Ready {
            return Err(Error::CapabilityBlocked {
                id: capability.id,
                code: capability.code,
                detail: capability.detail,
                remediation: capability.remediation.unwrap_or_default(),
            });
        }
    }

    let parent = destination
        .parent()
        .filter(|p| return !p.as_os_str().is_empty())
        .unwrap_or_else(|| return Path::new("."));

    // create_private_directory walks/creates parents descriptor-relatively with
    // no symlink traversal; do not precede it with following create_dir_all.
    let staging = parent.join(format!(".derivative-{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;

    let primary = (|| {
        let result = export_staged(
            plan,
            session,
            &staging,
            destination,
            options,
            &source,
            cancelled,
        )?;

        if cancelled.load(Ordering::Acquire) {
            return Err(plan_error("derivative export cancelled before publication"));
        }

        commit_directory(parent, &staging, destination, false).map_err(|error|match error {
            EvidenceError::DestinationExists{path}=>return Error::ArtifactExists{path},
            error=>return plan_error(format!("derivative publication at {} failed (if rename succeeded, artifact may already be published): {error}",destination.display())),
        })?;

        return Ok(result);
    })();

    let cleanup = cleanup_staging(&staging);

    return finish_with_cleanup(primary, cleanup);
}

fn cleanup_staging(staging: &Path) -> Result<()> {
    match fs_err::symlink_metadata(staging) {
        Ok(_) => return fs_err::remove_dir_all(staging).map_err(|e| return filesystem(staging, e)),
        Err(error) if error.kind() == IoErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(filesystem(staging, error)),
    }
}

fn export_staged(
    plan: &PresentationPlan,
    session: &Path,
    staging: &Path,
    destination: &Path,
    options: &DerivativeExportOptions,
    source: &ArtifactSourceRef,
    cancelled: &AtomicBool,
) -> Result<MediaArtifactResult> {
    let source_provenance_path = match &plan.source {
        PresentationSource::Native { video, .. } => {
            let source_path = video
                .parent()
                .ok_or_else(|| return plan_error("capture source directory missing"))?
                .join("manifest.json");
            let bytes =
                fs_err::read(&source_path).map_err(|e| return filesystem(&source_path, e))?;
            write_secure(&staging.join("source-capture-manifest.json"), &bytes)?;
            "source-capture-manifest.json"
        }
        PresentationSource::Semantic { .. } => "semantic-source-manifest.json",
    };

    let mut commands = vec![compose_pixels(plan, staging, true, Some(cancelled))?];
    let composition_blake3 = hash_file(&staging.join("composed.mkv"))?;
    let semantic_spans = if matches!(plan.source, PresentationSource::Semantic { .. }) {
        read_semantic_frame_spans(&staging.join("semantic-source-map.jsonl"))?
    } else {
        vec![]
    };

    write_presentation_map(
        &staging.join("presentation-map.jsonl"),
        plan,
        &semantic_spans,
    )?;

    json(&staging.join("normalized-plan.json"), plan)?;
    let composed_probe = probe_video_file(staging, "composed.mkv", false)?;
    validate_probe(&composed_probe, plan, "ffv1")?;
    json(&staging.join("composition-probe.json"), &composed_probe)?;

    let mut outputs = Vec::new();
    for delivery in &options.formats {
        let filename = delivery.filename();
        match delivery {
            DerivativeFormat::Gif { loop_policy } => {
                write_schedule(staging, plan)?;

                let palette = vec![
                    "-vf",
                    "palettegen=stats_mode=full:reserve_transparent=0",
                    "-frames:v",
                    "1",
                    "-threads",
                    "1",
                    "palette.png",
                ];
                encode(
                    staging,
                    "palette",
                    &mut commands,
                    palette.into_iter().map(str::to_owned).collect(),
                    cancelled,
                )?;

                let final_delay = gif_boundary_centiseconds(plan.output_frame_count, plan.fps)
                    .map_err(plan_error)?
                    - gif_boundary_centiseconds(plan.output_frame_count - 1, plan.fps)
                        .map_err(plan_error)?;

                let filter = format!("[0:v][1:v]paletteuse=dither=bayer:bayer_scale=5:new=0:alpha_threshold=0,settb=1/100,setpts='floor((N*200+{})/(2*{}))'[out]", plan.fps, plan.fps);
                let args = vec![
                    "-i".into(),
                    "palette.png".into(),
                    "-filter_complex".into(),
                    filter,
                    "-map".into(),
                    "[out]".into(),
                    "-fps_mode".into(),
                    "passthrough".into(),
                    "-enc_time_base".into(),
                    "1:100".into(),
                    "-c:v".into(),
                    "gif".into(),
                    "-gifflags".into(),
                    "0".into(),
                    "-loop".into(),
                    if *loop_policy == GifLoop::Infinite { "0" } else { "-1" }.into(),
                    "-final_delay".into(),
                    final_delay.to_string(),
                    filename.clone(),
                ];
                encode(staging, "gif", &mut commands, args, cancelled)?;
                verify_gif(&staging.join(&filename), plan, *loop_policy)?;
            }
            DerivativeFormat::Webm {} => {
                let args: Vec<String> = [
                    "-vf",
                    "scale=in_range=pc:out_range=tv:out_color_matrix=bt709,format=yuv420p",
                    "-c:v",
                    "libvpx-vp9",
                    "-crf",
                    "32",
                    "-b:v",
                    "0",
                    "-threads",
                    "1",
                    "-deadline",
                    "good",
                    "-cpu-used",
                    "2",
                    "-row-mt",
                    "0",
                    "-color_range",
                    "tv",
                    "-colorspace",
                    "bt709",
                    "-fps_mode",
                    "passthrough",
                ]
                .map(str::to_owned)
                .into_iter()
                .chain([filename.clone()])
                .collect();
                encode(staging, "webm", &mut commands, args, cancelled)?;
            }
        }

        let probe = probe_video_file(
            staging,
            &filename,
            matches!(delivery, DerivativeFormat::Gif { .. }),
        )?;

        validate_probe(
            &probe,
            plan,
            if matches!(delivery, DerivativeFormat::Webm {}) { "vp9" } else { "gif" },
        )?;

        if matches!(delivery, DerivativeFormat::Webm {}) {
            verify_webm_timestamps(staging, &filename, plan, &probe, cancelled)?;
        }

        json(&staging.join(format!("{filename}-probe.json")), &probe)?;
        outputs.push(DerivativeOutput {
            delivery: *delivery,
            path: filename.clone(),
            blake3: hash_file(&staging.join(&filename))?,
            frame_count: DecimalU64(plan.output_frame_count),
            probe,
        });
    }

    if verify_source_selection(plan, session, options.source_kind)? != *source {
        return Err(plan_error(
            "canonical source changed during derivative export",
        ));
    }

    if let PresentationSource::Native { video, .. } = &plan.source {
        let original = video
            .parent()
            .ok_or_else(|| return plan_error("capture source directory missing"))?
            .join("manifest.json");

        if hash_file(&original)? != hash_file(&staging.join(source_provenance_path))? {
            return Err(plan_error(
                "source capture provenance changed during derivative export",
            ));
        }
    }

    // Never publish private pixel intermediates (semantic maps/resources remain manifested).
    for filename in ["composed.mkv", "semantic-source.mkv"] {
        let path = staging.join(filename);
        if path.exists() {
            fs_err::remove_file(&path).map_err(|e| return filesystem(&path, e))?;
        }
    }

    let members = collect_artifact_members(staging)?;
    let mut sdk_members: Vec<SdkArtifactMember> = members.iter().cloned().map(Into::into).collect();

    for member in &mut sdk_members {
        match member.path.as_str() {
            "presentation.gif" => {
                member.media_type = "image/gif".into();
                member.relationship = "presentation_delivery".into();
            }
            "presentation.webm" => {
                member.media_type = "video/webm".into();
                member.relationship = "presentation_delivery".into();
            }
            "gif-timing.jsonl" => {
                member.schema_type = Some("gif-frame-timing.schema.json".into());
                member.relationship = "presentation_timing".into();
            }
            "presentation-map.jsonl" => {
                member.schema_type = Some("presentation-map-entry.schema.json".into());
                member.relationship = "presentation_mapping".into();
            }
            "normalized-plan.json" => {
                member.schema_type = Some("presentation-plan.schema.json".into());
            }
            _ => {}
        }
    }

    let provenance = ArtifactProvenance::PresentationOutput {
        source_kind: options.source_kind,
    };
    let manifest = DerivativeManifest {
        version: 1,
        source: source.clone(),
        provenance: provenance.clone(),
        edit_plan_hash: plan.edit_plan_hash.clone(),
        source_identity_hash: presentation_source_identity_hash(&plan.source)?,
        source_provenance_path: source_provenance_path.into(),
        source_provenance_blake3: hash_file(&staging.join(source_provenance_path))?,
        composition_blake3,
        normalized_plan_blake3: hash_file(&staging.join("normalized-plan.json"))?,
        presentation_map_blake3: hash_file(&staging.join("presentation-map.jsonl"))?,
        fps: plan.fps,
        width: plan.output_width,
        height: plan.output_height,
        frame_count: DecimalU64(plan.output_frame_count),
        color_policy: concat!(
            "Native CFR source mapping uses signed first-acquisition minus execution/replay wall-clock origin, including pre-roll; floor/ceil nominal CFR selection is acquisition-clock approximate, not semantic presentation proof. ",
            "Source RGB policy/profile retained in normalized plan and capture/semantic provenance. ",
            "Full-range RGB assumed for matrix conversion; transfer and primaries unverified, not relabeled. ",
            "WebM: explicit BT.709 matrix, limited-range opaque yuv420p, lossy VP9 CRF32 b=0, good cpu-used=2 threads=1. ",
            "GIF: lossy opaque RGB quantization; no downscale."
        ).into(),
        gif_policy: concat!(
            "Global 256-color palette; palettegen stats_mode=full reserve_transparent=0; two disk-backed passes; ",
            "paletteuse Bayer scale5 new=0; full opaque frames gifflags=0. ",
            "q(i)=floor((200*i+fps)/(2*fps)) centiseconds, last delay explicit; boundary error <=5ms. ",
            "60FPS contains 1cs delays which viewers may clamp; no exact viewer cadence guarantee."
        ).into(),
        ffmpeg_version: command_version("ffmpeg")?,
        ffprobe_version: command_version("ffprobe")?,
        commands,
        outputs,
        members: sdk_members,
    };

    json(&staging.join("manifest.json"), &manifest)?;
    verify_artifact_members(staging, &members)?;
    let result = MediaArtifactResult {
        version: 1,
        directory_bytes: ByteString::from(destination.as_os_str().as_bytes()),
        manifest: ArtifactManifestRef {
            path: "manifest.json".into(),
            blake3: hash_file(&staging.join("manifest.json"))?,
        },
        source: source.clone(),
        provenance,
        product_kind: ArtifactProductKind::Presentation,
        members: manifest.members,
    };

    result.validate().map_err(plan_error)?;

    return Ok(result);
}

fn encode(
    staging: &Path,
    stem: &str,
    commands: &mut Vec<Vec<String>>,
    tail: Vec<String>,
    cancelled: &AtomicBool,
) -> Result<()> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-threads",
        "1",
        "-filter_complex_threads",
        "1",
        "-filter_threads",
        "1",
        "-i",
        "composed.mkv",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend(tail);
    run_logged_command(
        "ffmpeg",
        &args,
        staging,
        stem,
        Some(cancelled),
        Duration::from_secs(86_400),
    )?;

    commands.push(args);

    return Ok(());
}

fn write_schedule(staging: &Path, plan: &PresentationPlan) -> Result<()> {
    let path = staging.join("gif-timing.jsonl");
    let mut writer = secure_writer(&path)?;

    for i in 0..plan.output_frame_count {
        let start = gif_boundary_centiseconds(i, plan.fps).map_err(plan_error)?;
        let end = gif_boundary_centiseconds(i + 1, plan.fps).map_err(plan_error)?;
        let entry = GifFrameTiming {
            frame: DecimalU64(i),
            start_centiseconds: DecimalU64(start),
            delay_centiseconds: u16::try_from(end - start)
                .map_err(|_error| return plan_error("GIF delay overflow"))?,
        };
        serde_json::to_writer(&mut writer, &entry).map_err(|source| {
            return Error::InvalidJson {
                path: path.clone(),
                json_path: "$".into(),
                source,
            };
        })?;
        writer
            .write_all(b"\n")
            .map_err(|e| return filesystem(&path, e))?;
    }

    return Ok(finish_writer(writer, &path)?);
}

fn validate_probe(probe: &serde_json::Value, plan: &PresentationPlan, codec: &str) -> Result<()> {
    let streams = probe["streams"]
        .as_array()
        .ok_or_else(|| return plan_error("probe streams missing"))?;
    let stream = streams
        .first()
        .filter(|_| return streams.len() == 1)
        .ok_or_else(|| return plan_error("probe requires one video stream"))?;

    if stream["codec_name"] != codec
        || stream["width"].as_u64() != Some(u64::from(plan.output_width))
        || stream["height"].as_u64() != Some(u64::from(plan.output_height))
        || stream["nb_read_frames"]
            .as_str()
            .and_then(|s| return s.parse::<u64>().ok())
            != Some(plan.output_frame_count)
    {
        return Err(plan_error(format!(
            "decoded {codec} stream facts/count do not match normalized plan: {stream}"
        )));
    }

    if codec == "vp9"
        && (stream["pix_fmt"] != "yuv420p"
            || stream["color_range"] != "tv"
            || stream["color_space"] != "bt709")
    {
        return Err(plan_error("VP9 pixel/color policy mismatch"));
    }
    return Ok(());
}

#[expect(
    clippy::cast_precision_loss,
    reason = "24h at u16 FPS is below 2^53; muxer timestamp tolerance is explicitly validated"
)]
fn verify_webm_timestamps(
    staging: &Path,
    filename: &str,
    plan: &PresentationPlan,
    probe: &serde_json::Value,
    cancelled: &AtomicBool,
) -> Result<()> {
    use std::io::BufRead;
    let time_base = probe
        .get("streams")
        .and_then(|v| return v.get(0))
        .and_then(|v| return v.get("time_base"))
        .and_then(|v| return v.as_str())
        .ok_or_else(|| return plan_error("WebM time base missing"))?;
    let (num, den) = time_base
        .split_once('/')
        .ok_or_else(|| return plan_error("malformed WebM time base"))?;
    let num: f64 = num
        .parse()
        .map_err(|_error| return plan_error("malformed WebM time base numerator"))?;

    let den: f64 = den
        .parse()
        .map_err(|_error| return plan_error("malformed WebM time base denominator"))?;

    let tick = num / den;
    if !tick.is_finite() || tick <= 0.0 || tick > 1.0 / f64::from(plan.fps) {
        return Err(plan_error("WebM time base cannot represent requested FPS"));
    }

    let args = [
        "-v",
        "error",
        "-i",
        filename,
        "-select_streams",
        "v:0",
        "-show_entries",
        "frame=best_effort_timestamp_time",
        "-of",
        "csv=p=0",
    ]
    .map(str::to_owned);

    run_logged_command(
        "ffprobe",
        &args,
        staging,
        "webm-timestamps",
        Some(cancelled),
        Duration::from_secs(3600),
    )?;

    let path = staging.join("webm-timestamps.stdout");
    let reader =
        BufReader::new(fs_err::File::open(&path).map_err(|e| return filesystem(&path, e))?);

    let mut count = 0u64;
    for line in reader.lines() {
        let line = line.map_err(|e| return filesystem(&path, e))?;
        if line.trim().is_empty() {
            continue;
        }

        let pts: f64 = line
            .trim()
            .parse()
            .map_err(|_error| return plan_error("malformed WebM frame timestamp"))?;
        if !pts.is_finite() || (pts - count as f64 / f64::from(plan.fps)).abs() > tick + 0.000_001 {
            return Err(plan_error("WebM frame timestamp exceeds muxer tick error"));
        }

        count += 1;
    }

    let duration: f64 = probe
        .get("format")
        .and_then(|v| return v.get("duration"))
        .and_then(|v| return v.as_str())
        .and_then(|s| return s.parse().ok())
        .ok_or_else(|| return plan_error("WebM duration missing"))?;

    if count != plan.output_frame_count
        || !duration.is_finite()
        || (duration - count as f64 / f64::from(plan.fps)).abs() > tick + 1.0 / f64::from(plan.fps)
    {
        return Err(plan_error("WebM count/duration does not match CFR plan"));
    }
    return Ok(());
}

/// Streaming GIF structural/timing verification independent of viewer minimum-delay rules.
#[expect(
    clippy::indexing_slicing,
    reason = "read_exact(n) returns exactly n bytes or an error; all indices are bounded constants"
)]
fn verify_gif(path: &Path, plan: &PresentationPlan, looping: GifLoop) -> Result<()> {
    let mut reader =
        BufReader::new(fs_err::File::open(path).map_err(|e| return filesystem(path, e))?);

    let mut read = |n: usize| -> Result<Vec<u8>> {
        let mut b = vec![0; n];
        reader
            .read_exact(&mut b)
            .map_err(|e| return filesystem(path, e))?;

        return Ok(b);
    };
    let header = read(13)?;

    if &header[..6] != b"GIF89a" || header[10] & 0x80 == 0 || header[10] & 7 != 7 {
        return Err(plan_error("GIF missing global palette"));
    }

    read(3 * (1usize << ((header[10] & 7) + 1)))?;
    let mut count = 0u64;
    let mut delay = None;
    let mut loop_found = false;

    loop {
        match read(1)?[0] {
            0x3b => break,
            0x21 => {
                let label = read(1)?[0];
                if label == 0xf9 {
                    if delay.is_some() {
                        return Err(plan_error("duplicate GIF timing extension"));
                    }

                    let gce = read(6)?;
                    if gce[0] != 4 || gce[5] != 0 || gce[1] & 1 != 0 {
                        return Err(plan_error("GIF frame is not opaque"));
                    }

                    delay = Some(u16::from_le_bytes([gce[2], gce[3]]));
                } else {
                    let mut first = true;
                    let mut netscape = false;

                    loop {
                        let n = usize::from(read(1)?[0]);
                        if n == 0 {
                            break;
                        }

                        let bytes = read(n)?;

                        if first {
                            netscape = label == 0xff && bytes == b"NETSCAPE2.0";
                            first = false;
                        } else if netscape {
                            if bytes != [1, 0, 0] {
                                return Err(plan_error("GIF loop count is not infinite"));
                            }

                            loop_found = true;
                        }
                    }
                }
            }
            0x2c => {
                let descriptor = read(9)?;
                if descriptor[8] & 0x80 != 0 {
                    return Err(plan_error("GIF contains per-frame palettes"));
                }

                if descriptor[..4] != [0; 4]
                    || u16::from_le_bytes([descriptor[4], descriptor[5]]) != plan.output_width
                    || u16::from_le_bytes([descriptor[6], descriptor[7]]) != plan.output_height
                {
                    return Err(plan_error("GIF frame is not full canvas"));
                }

                let expected = gif_boundary_centiseconds(count + 1, plan.fps)
                    .map_err(plan_error)?
                    - gif_boundary_centiseconds(count, plan.fps).map_err(plan_error)?;
                if delay.take().map(u64::from) != Some(expected) || count >= plan.output_frame_count
                {
                    return Err(plan_error(
                        "GIF delay/count differs from manifested schedule",
                    ));
                }

                read(1)?; // LZW code size; ffprobe independently decodes the image data.

                loop {
                    let n = usize::from(read(1)?[0]);
                    if n == 0 {
                        break;
                    }

                    read(n)?;
                }

                count += 1;
            }
            _ => return Err(plan_error("malformed GIF block")),
        }
    }

    if count != plan.output_frame_count || loop_found != (looping == GifLoop::Infinite) {
        return Err(plan_error("GIF final count/loop policy mismatch"));
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_probes_fail_closed() {
        let plan = PresentationPlan {
            output_width: 16,
            output_height: 16,
            output_frame_count: 1,
            ..Default::default()
        };

        for probe in [
            serde_json::json!({}),
            serde_json::json!({"streams":[]}),
            serde_json::json!({"streams":[{"codec_name":"vp8","width":16,"height":16,"nb_read_frames":"1"}]}),
        ] {
            assert!(validate_probe(&probe, &plan, "vp9").is_err());
        }
    }
}

#[cfg(test)]
mod encoder_tests {
    use super::*;
    use std::{io, os::unix::fs::symlink};
    use termtrace_engine::recording::{record_with_output, RecordOptions};
    use termtrace_model::{
        artifact::{presentation::*, save::SemanticRasterProfile},
        execution::session::RecordingFailurePolicy,
    };

    #[test]
    #[expect(
        clippy::option_if_let_else,
        reason = "explicit opt-in retention branch keeps native test setup readable"
    )]
    #[expect(
        clippy::print_stderr,
        reason = "report retained verification evidence path"
    )]
    fn real_semantic_gif_webm_share_composition_map_and_exact_timing() {
        let temp = if let Ok(parent) = env::var("TERMTRACE_MEDIA_TEST_OUTPUT") {
            fs_err::create_dir_all(&parent).unwrap();

            tempfile::Builder::new()
                .prefix("semantic-")
                .tempdir_in(parent)
                .unwrap()
        } else {
            tempfile::tempdir().unwrap()
        };

        let session = temp.path().join("session");
        let sentinel = temp.path().join("app-runs");
        let options = RecordOptions {
            failure_policy: RecordingFailurePolicy::Stop, output_directory:session.clone(), creation_alias:None,lineage:None,import_provenance:None,tmux_pane:None,
            command: vec!["/bin/sh".into(),"-c".into(),format!("printf x >> '{}'; printf '\\033[41mRED\\033[0m'; sleep 0.15; printf '\\r\\033[42mGREEN\\033[0m'; sleep 0.15", sentinel.display()).into()],
            working_directory:None,cols:12,rows:3,cell_width_px:8,cell_height_px:16,geometry_provenance:Default::default(),ghostty_profile:None,visual_profile:None,
            term:"xterm-256color".into(),environment:Default::default(),colors:Default::default(),forward_stdin:false,continue_after_output_mirror_closes:false,
            durability_policy:Default::default(),provenance_policy:Default::default(),resource_limits:Default::default(),
        };

        record_with_output(&options, &mut io::sink()).unwrap();
        let before = canonical_hashes(&session);
        let ass = temp.path().join("subtitles.ass");
        fs_err::write(&ass, "[Script Info]\nScriptType: v4.00+\nPlayResX: 80\nPlayResY: 40\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,8,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,2,2,2,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,SUB\n").unwrap();
        for fps in [24, 30, 60] {
            let edit = EditPlan {
                version: EDIT_PLAN_VERSION,
                clips: vec![EditClip {
                    from: EditSelector {
                        monotonic_ns: Some(0),
                        ..Default::default()
                    },
                    to: EditSelector {
                        monotonic_ns: Some(200_000_000),
                        ..Default::default()
                    },
                    speed: RationalSpeed {
                        numerator: 3,
                        denominator: 2,
                    },
                    hold_ms: 125,
                    caption: None,
                }],
                annotations: vec![TextAnnotation {
                    text: Some("ANNOTATION".into()),
                    template: None,
                    scope: AnnotationScope::WholeVideo,
                    position: AnnotationPosition::default(),
                    style: AnnotationStyle::default(),
                    z_order: 0,
                }],
                subtitles: vec![SubtitleTrack { ass: ass.clone() }],
                transforms: vec![PixelTransform::Crop {
                    x: 0,
                    y: 0,
                    width: 79,
                    height: 39,
                }],
                ..Default::default()
            };
            let plan = super::super::plan_with_source(
                &session,
                &edit,
                PresentationSourceRequest::Semantic {
                    fps,
                    raster_profile: SemanticRasterProfile::Direct,
                },
            )
            .unwrap();

            let dest = temp.path().join(format!("both-{fps}"));
            let options = DerivativeExportOptions {
                formats: vec![
                    DerivativeFormat::Gif {
                        loop_policy: GifLoop::Infinite,
                    },
                    DerivativeFormat::Webm {},
                ],
                source_kind: ArtifactSourceKind::SemanticRendering,
            };
            let outcome = export_derivatives(&plan, &session, &dest, &options).unwrap();
            outcome.validate().unwrap();
            verify_derivative_artifact(&dest, &outcome).unwrap();

            assert_eq!(
                outcome.provenance,
                ArtifactProvenance::PresentationOutput {
                    source_kind: ArtifactSourceKind::SemanticRendering
                }
            );
            assert_eq!(canonical_hashes(&session), before);
            assert_eq!(fs_err::read(&sentinel).unwrap(), b"x");
            assert_eq!((plan.output_width, plan.output_height), (80, 40));
            assert!(!dest.join("composed.mkv").exists());

            let manifest: DerivativeManifest =
                serde_json::from_slice(&fs_err::read(dest.join("manifest.json")).unwrap()).unwrap();
            assert_eq!(manifest.outputs.len(), 2);
            assert_eq!(manifest.commands.len(), 4);
            assert_eq!(
                manifest.presentation_map_blake3,
                hash_file(&dest.join("presentation-map.jsonl")).unwrap()
            );

            let mp4 = temp.path().join(format!("mp4-{fps}"));
            super::super::export(&plan, &mp4).unwrap();

            assert_eq!(
                fs_err::read(dest.join("presentation-map.jsonl")).unwrap(),
                fs_err::read(mp4.join("presentation-map.jsonl")).unwrap()
            );

            let cancelled = AtomicBool::new(true);
            assert!(export_derivatives_with_cancel(
                &plan,
                &session,
                &temp.path().join(format!("cancelled-{fps}")),
                &options,
                &cancelled
            )
            .is_err());
            assert!(verify_source_selection(
                &plan,
                &session,
                ArtifactSourceKind::OriginalExecution
            )
            .is_err());
            assert!(
                verify_source_selection(&plan, &session, ArtifactSourceKind::NativeReplay).is_err()
            );

            let original = fs_err::read(dest.join("presentation.gif")).unwrap();

            assert!(export_derivatives(&plan, &session, &dest, &options).is_err());
            assert_eq!(
                fs_err::read(dest.join("presentation.gif")).unwrap(),
                original
            );

            if fps == 30 {
                let once = temp.path().join("new-parent").join("once");
                export_derivatives(
                    &plan,
                    &session,
                    &once,
                    &DerivativeExportOptions {
                        formats: vec![DerivativeFormat::Gif {
                            loop_policy: GifLoop::Once,
                        }],
                        ..options.clone()
                    },
                )
                .unwrap();

                let target = temp.path().join("symlink-target");
                fs_err::create_dir(&target).unwrap();
                let link = temp.path().join("symlink-parent");
                symlink(&target, &link).unwrap();

                assert!(export_derivatives(
                    &plan,
                    &session,
                    link.join("must-not-create").join("output"),
                    &options
                )
                .is_err());
                assert!(
                    !target.join("must-not-create").exists(),
                    "unsafe parent traversal must have no directory creation side effect"
                );
                // Both encodes use identical palette bytes within this FFmpeg build.
                assert_eq!(
                    fs_err::read(dest.join("palette.png")).unwrap(),
                    fs_err::read(once.join("palette.png")).unwrap()
                );
            }
        }

        assert_eq!(canonical_hashes(&session), before);
        assert_eq!(fs_err::read(sentinel).unwrap(), b"x");

        if env::var_os("TERMTRACE_MEDIA_TEST_OUTPUT").is_some() {
            eprintln!("retained semantic evidence: {}", temp.keep().display());
        }
    }

    fn canonical_hashes(session: &Path) -> Vec<String> {
        return ["session.json", "events.jsonl", "completion.json"]
            .into_iter()
            .map(|f| return hash_file(&session.join(f)).unwrap())
            .collect();
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use std::{sync::Arc, thread};

    #[test]
    fn process_failure_full_disk_and_cancellation_are_not_success() {
        let temp = tempfile::tempdir().unwrap();
        let args = vec!["-c".into(), "printf primary-failure >&2; exit 9".into()];
        let failure = run_logged_command(
            "/bin/sh",
            &args,
            temp.path(),
            "failure",
            None,
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(failure.to_string().contains("failed"));
        assert_eq!(
            fs_err::read(temp.path().join("failure.log")).unwrap(),
            b"primary-failure"
        );

        let disk: Vec<String> = [
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=black:s=16x16:r=30:d=0.1",
            "-frames:v",
            "3",
            "-c:v",
            "gif",
            "-f",
            "gif",
            "/dev/full",
        ]
        .map(str::to_owned)
        .to_vec();
        assert!(run_logged_command(
            "ffmpeg",
            &disk,
            temp.path(),
            "disk-full",
            None,
            Duration::from_secs(5)
        )
        .is_err());
        assert!(
            String::from_utf8_lossy(&fs_err::read(temp.path().join("disk-full.log")).unwrap())
                .contains("No space left")
        );

        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));

            flag.store(true, Ordering::Release);
        });

        let args = vec!["-c".into(), "echo $$ > child.pid; sleep 30 & wait".into()];
        let failure = run_logged_command(
            "/bin/sh",
            &args,
            temp.path(),
            "cancel",
            Some(&cancelled),
            Duration::from_secs(5),
        )
        .unwrap_err();

        setter.join().unwrap();

        assert!(failure.to_string().contains("cancelled"));

        let pid = fs_err::read_to_string(temp.path().join("child.pid")).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists());
        let cleanup = Err(filesystem(
            temp.path(),
            io::Error::new(IoErrorKind::PermissionDenied, "cleanup denied"),
        ));

        let combined: Result<()> = finish_with_cleanup(Err(failure), cleanup);
        let message = combined.unwrap_err().to_string();
        assert!(message.contains("cancelled") && message.contains("cleanup denied"));

        let args = vec!["-c".into(), "sleep 30".into()];

        assert!(run_logged_command(
            "/bin/sh",
            &args,
            temp.path(),
            "timeout",
            None,
            Duration::from_millis(100)
        )
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    }

    #[test]
    fn actual_cleanup_failure_preserves_primary_failure() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("private");
        fs_err::create_dir(&parent).unwrap();
        let staging = parent.join("staging");
        fs_err::create_dir(&staging).unwrap();
        fs_err::write(staging.join("payload"), b"payload").unwrap();

        fs_err::set_permissions(&parent, Permissions::from_mode(0o500)).unwrap();
        let cleanup = cleanup_staging(&staging);
        fs_err::set_permissions(&parent, Permissions::from_mode(0o700)).unwrap();

        assert!(
            cleanup.is_err(),
            "the actual filesystem cleanup must fail, not skip"
        );

        let result: Result<()> =
            finish_with_cleanup(Err(plan_error("primary encoder failure")), cleanup);
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("primary encoder failure") && message.contains("cleanup also failed")
        );
    }
}

#[cfg(test)]
mod mapping_pixel_tests {
    use super::super::mapping::mapped_source_frame;
    use super::*;
    use std::process::{Command, Stdio};
    use termtrace_model::{
        artifact::presentation::{NormalizedClip, RationalSpeed},
        terminal::replay::SourceRange,
    };

    #[test]
    fn composition_pixels_match_exact_rational_cut_and_hold_map() {
        let temp = tempfile::tempdir().unwrap();

        for fps in [24, 30, 60] {
            let source = temp.path().join(format!("source-{fps}.mkv"));
            let mut child = Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-f",
                    "rawvideo",
                    "-pixel_format",
                    "rgb24",
                    "-video_size",
                    "16x16",
                    "-framerate",
                    &fps.to_string(),
                    "-i",
                    "-",
                    "-c:v",
                    "ffv1",
                    "-pix_fmt",
                    "bgr0",
                ])
                .arg(&source)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();

            for i in 0u8..12 {
                input.write_all(&vec![i * 10; 16 * 16 * 3]).unwrap();
            }

            drop(input);
            assert!(child.wait().unwrap().success());

            for (num, den) in [(2, 1), (3, 2), (2, 3), (1, 1)] {
                let n = (7u64 * den).div_ceil(num);
                let clip = |start, end, out_start, retimed, hold| {
                    return NormalizedClip {
                        source_range: SourceRange {
                            start_monotonic_ns: start * 1_000_000_000 / u64::from(fps),
                            end_monotonic_ns: end * 1_000_000_000 / u64::from(fps),
                        },
                        source_start_frame: start,
                        source_end_frame: end,
                        output_start_frame: out_start,
                        output_end_frame: out_start + retimed + hold,
                        retimed_frame_count: retimed,
                        hold_frame_count: hold,
                        speed: RationalSpeed {
                            numerator: u32::try_from(num).unwrap(),
                            denominator: u32::try_from(den).unwrap(),
                        },
                        caption: None,
                    };
                };
                let mut last = clip(10, 11, n + 2, 1, 2);
                last.speed = RationalSpeed::default();
                let plan = PresentationPlan {
                    source: PresentationSource::Native {
                        video: source.clone(),
                        video_hash: hash_file(&source).unwrap(),
                    },
                    fps,
                    source_width: 16,
                    source_height: 16,
                    output_width: 16,
                    output_height: 16,
                    clips: vec![clip(0, 7, 0, n, 2), last],
                    output_frame_count: n + 5,
                    ..Default::default()
                };

                let directory = temp.path().join(format!("compose-{fps}-{num}-{den}"));
                create_private_directory(&directory).unwrap();
                compose_pixels(&plan, &directory, true, None).unwrap();
                let output = Command::new("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(directory.join("composed.mkv"))
                    .args([
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        "rgb24",
                        "-fps_mode",
                        "passthrough",
                        "-",
                    ])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert_eq!(
                    output.stdout.len(),
                    usize::try_from(plan.output_frame_count).unwrap() * 16 * 16 * 3
                );

                for (index, pixels) in output
                    .stdout
                    .as_chunks::<{ 16 * 16 * 3 }>()
                    .0
                    .iter()
                    .enumerate()
                {
                    let clip = plan
                        .clips
                        .iter()
                        .find(|c| {
                            return index as u64 >= c.output_start_frame
                                && (index as u64) < c.output_end_frame;
                        })
                        .unwrap();

                    let (source_frame, _) =
                        mapped_source_frame(clip, u64::try_from(index).unwrap());
                    assert!(pixels.iter().all(|p|return u64::from(*p)==source_frame*10),"fps={fps} speed={num}/{den} frame={index} expected source {source_frame}, got {}",pixels[0]);
                }
            }
        }
    }

    pub(super) fn decode_rgb(path: &Path, gif: bool) -> Vec<u8> {
        let mut command = Command::new("ffmpeg");
        command.args(["-v", "error"]);

        if gif {
            command.args(["-min_delay", "0"]);
        }

        let output = command
            .arg("-i")
            .arg(path)
            .args([
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
                "-",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        return output.stdout;
    }
}

/// Verify the envelope, exact manifest bytes, and complete payload hashes/lengths.
/// Metadata is derivative-owned; the legacy member classifier is not rewritten.
///
/// # Errors
/// Rejects tampered bytes, missing/extra files, invalid manifests or contradictory envelopes.
pub fn verify_derivative_artifact(
    directory: &Path,
    result: &MediaArtifactResult,
) -> Result<DerivativeManifest> {
    result.validate().map_err(plan_error)?;
    if result.directory_bytes.as_bytes() != directory.as_os_str().as_bytes() {
        return Err(plan_error(
            "artifact directory bytes differ from verification path",
        ));
    }

    let manifest_path = directory.join(&result.manifest.path);
    if result.manifest.path != "manifest.json"
        || hash_file(&manifest_path)? != result.manifest.blake3
    {
        return Err(plan_error("derivative manifest hash differs from result"));
    }

    let bytes = fs_err::read(&manifest_path).map_err(|e| return filesystem(&manifest_path, e))?;
    let manifest: DerivativeManifest = serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: manifest_path,
            json_path: "$".into(),
            source,
        };
    })?;

    if manifest.version != 1
        || manifest.source != result.source
        || manifest.provenance != result.provenance
        || manifest.members != result.members
    {
        return Err(plan_error(
            "derivative manifest contradicts artifact result",
        ));
    }

    let actual = collect_artifact_members(directory)?;
    verify_artifact_members(directory, &actual)?;
    if actual.len() != manifest.members.len() {
        return Err(plan_error("derivative payload inventory length differs"));
    }

    for (actual, expected) in actual.iter().zip(&manifest.members) {
        if actual.path != expected.path
            || actual.blake3 != expected.blake3
            || actual.byte_length != expected.byte_length.0
        {
            return Err(plan_error("derivative member bytes differ from manifest"));
        }
    }

    if hash_file(&directory.join("normalized-plan.json"))? != manifest.normalized_plan_blake3
        || hash_file(&directory.join("presentation-map.jsonl"))? != manifest.presentation_map_blake3
    {
        return Err(plan_error("common plan/map hashes differ"));
    }

    if !matches!(
        manifest.source_provenance_path.as_str(),
        "source-capture-manifest.json" | "semantic-source-manifest.json"
    ) || hash_file(&directory.join(&manifest.source_provenance_path))?
        != manifest.source_provenance_blake3
    {
        return Err(plan_error("source provenance snapshot differs"));
    }

    for output in &manifest.outputs {
        if output.path != output.delivery.filename()
            || hash_file(&directory.join(&output.path))? != output.blake3
            || output.frame_count != manifest.frame_count
        {
            return Err(plan_error("derivative output identity differs"));
        }
    }

    return Ok(manifest);
}

#[cfg(test)]
mod native_encoder_tests {
    use super::super::{export, plan_with_capture_directory};
    use super::mapping_pixel_tests::decode_rgb;
    use super::*;
    use crate::native_capture::{capture, native_capture_target};
    use std::process::Command;
    use termtrace_engine::replay::index_session_for_video;
    use termtrace_model::{
        artifact::{
            presentation::{
                EditClip, EditPlan, EditSelector, PixelTransform, PresentationSourceRequest,
                RationalSpeed, EDIT_PLAN_VERSION,
            },
            visual::{CaptureKind, CaptureOptions},
        },
        terminal::replay::StateSelection,
    };

    #[test]
    #[expect(
        clippy::option_if_let_else,
        reason = "explicit opt-in retention branch keeps native test setup readable"
    )]
    #[expect(
        clippy::print_stderr,
        reason = "report retained verification evidence path"
    )]
    #[expect(
        clippy::cast_precision_loss,
        reason = "test frame dimensions bound color error sums well below 2^53"
    )]
    #[ignore = "requires real Ghostty/Xvfb/FFmpeg and built TERMTRACE_BIN; run explicitly, never a skipped completion gate"]
    fn real_ghostty_replay_gif_webm_preserve_source_and_do_not_rerun_application() {
        let helper = env::var_os("TERMTRACE_BIN")
            .map(PathBuf::from)
            .expect("set TERMTRACE_BIN to built helper");

        let temp = if let Ok(parent) = env::var("TERMTRACE_MEDIA_TEST_OUTPUT") {
            fs_err::create_dir_all(&parent).unwrap();

            tempfile::Builder::new()
                .prefix("native-")
                .tempdir_in(parent)
                .unwrap()
        } else {
            tempfile::tempdir().unwrap()
        };

        let session = temp.path().join("session");
        let sentinel = temp.path().join("app-runs");
        let status=Command::new(&helper).args(["record","--no-input","--cols","12","--rows","3","--out"]).arg(&session).args(["--","/bin/sh","-c"]).arg(format!("printf x >> '{}'; printf '\\033[41mRED\\033[0m'; sleep 0.15; printf '\\r\\033[42mGREEN\\033[0m'; sleep 0.15",sentinel.display())).status().unwrap();
        assert!(status.success());

        let before: Vec<_> = ["session.json", "events.jsonl", "completion.json"]
            .map(|f| return hash_file(&session.join(f)).unwrap())
            .into();

        index_session_for_video(&session, 30).unwrap();
        let capture_directory = temp.path().join("replay-pixels");
        let (target, geometry_solver) = native_capture_target(&helper, &session, None).unwrap();
        capture(
            &helper,
            &CaptureOptions {
                session_directory: session.clone(),
                width: target.width,
                height: target.height,
                padding: target.padding,
                fps: 30,
                start_monotonic_ns: None,
                end_monotonic_ns: None,
                state_selection: StateSelection::All,
                geometry_solver,
                kind: CaptureKind::Video,
                destination: Some(capture_directory.clone()),
                scale: None,
                dpi: None,
            },
        )
        .unwrap();

        let capture_members = collect_artifact_members(&capture_directory).unwrap();
        let manifest_hash = hash_file(&capture_directory.join("manifest.json")).unwrap();
        let capture_manifest: VisualManifest =
            serde_json::from_slice(&fs_err::read(capture_directory.join("manifest.json")).unwrap())
                .unwrap();
        // The native selector adapter is canonical-time-bounded, not unrestricted
        // raw capture selection. Reject pre-roll instead of fabricating time zero.
        assert!(capture_manifest.capture_started_unix_ns < capture_manifest.replay_origin_unix_ns);

        let pre_roll_ns =
            capture_manifest.replay_origin_unix_ns - capture_manifest.capture_started_unix_ns;

        let positive_ordinal =
            u64::try_from((pre_roll_ns * 30).div_ceil(1_000_000_000)).unwrap() + 1;

        let authored_native = EditPlan {
            version: EDIT_PLAN_VERSION,
            clips: vec![EditClip {
                from: EditSelector {
                    native_frame: Some(positive_ordinal),
                    ..Default::default()
                },
                to: EditSelector {
                    native_frame: Some(positive_ordinal + 1),
                    ..Default::default()
                },
                speed: RationalSpeed::default(),
                hold_ms: 0,
                caption: None,
            }],
            ..Default::default()
        };
        let authored_bytes = serde_json::to_vec(&authored_native).unwrap();

        let authored_identity =
            serde_json::json!({"edit": &authored_native, "subtitle_hashes": []});

        let expected_authored_hash = blake3::hash(&serde_json::to_vec(&authored_identity).unwrap())
            .to_hex()
            .to_string();

        let native_ordinal_plan = plan_with_capture_directory(
            &session,
            &authored_native,
            PresentationSourceRequest::Native,
            Some(&capture_directory),
        )
        .unwrap();
        assert_eq!(native_ordinal_plan.edit_plan_hash, expected_authored_hash);
        assert_eq!(
            native_ordinal_plan
                .clips
                .first()
                .unwrap()
                .source_start_frame,
            positive_ordinal
        );
        assert_eq!(
            native_ordinal_plan.clips.first().unwrap().source_end_frame,
            positive_ordinal + 1
        );
        assert_eq!(
            serde_json::to_vec(&authored_native).unwrap(),
            authored_bytes
        );

        json(
            &temp.path().join("authored-native-edit.json"),
            &authored_native,
        )
        .unwrap();
        json(
            &temp.path().join("native-ordinal-plan.json"),
            &native_ordinal_plan,
        )
        .unwrap();

        let mut pre_roll_edit = authored_native;
        pre_roll_edit.clips.first_mut().unwrap().from.native_frame = Some(0);
        let pre_roll_authored_bytes = serde_json::to_vec(&pre_roll_edit).unwrap();
        let error = plan_with_capture_directory(
            &session,
            &pre_roll_edit,
            PresentationSourceRequest::Native,
            Some(&capture_directory),
        )
        .expect_err("nativeFrame0 pre-roll must fail");
        assert!(error.to_string().contains("nativeFrame 0"));
        assert!(error.to_string().contains("before time zero"));
        assert!(error.to_string().contains("monotonicNs"));
        assert_eq!(
            serde_json::to_vec(&pre_roll_edit).unwrap(),
            pre_roll_authored_bytes
        );

        json(
            &temp.path().join("pre-roll-selector-error.json"),
            &serde_json::json!({"diagnostic": error.to_string(), "authored_edit_preserved": true}),
        )
        .unwrap();

        let crop_x = capture_manifest
            .width
            .saturating_sub(capture_manifest.geometry_report.text_width)
            / 2;

        let crop_y = capture_manifest
            .height
            .saturating_sub(capture_manifest.geometry_report.text_height)
            / 2;

        let edit = EditPlan {
            version: EDIT_PLAN_VERSION,
            clips: vec![EditClip {
                from: EditSelector {
                    monotonic_ns: Some(0),
                    ..Default::default()
                },
                to: EditSelector {
                    monotonic_ns: Some(200_000_000),
                    ..Default::default()
                },
                speed: RationalSpeed {
                    numerator: 3,
                    denominator: 2,
                },
                hold_ms: 125,
                caption: None,
            }],
            transforms: vec![PixelTransform::Crop {
                x: crop_x,
                y: crop_y,
                width: 79,
                height: 39,
            }],
            ..Default::default()
        };
        let plan = plan_with_capture_directory(
            &session,
            &edit,
            PresentationSourceRequest::Native,
            Some(&capture_directory),
        )
        .unwrap();
        assert!(
            verify_source_selection(&plan, &session, ArtifactSourceKind::OriginalExecution)
                .is_err()
        );

        let options = DerivativeExportOptions {
            formats: vec![
                DerivativeFormat::Gif {
                    loop_policy: GifLoop::Infinite,
                },
                DerivativeFormat::Webm {},
            ],
            source_kind: ArtifactSourceKind::NativeReplay,
        };

        let directory = temp.path().join("both");
        let result = export_derivatives(&plan, &session, &directory, &options).unwrap();
        verify_derivative_artifact(&directory, &result).unwrap();
        json(&temp.path().join("result.json"), &result).unwrap();
        let mp4 = temp.path().join("mp4");
        export(&plan, &mp4).unwrap();

        assert_eq!(
            fs_err::read(directory.join("presentation-map.jsonl")).unwrap(),
            fs_err::read(mp4.join("presentation-map.jsonl")).unwrap()
        );

        // Decode the actual deliveries against the exact shared RGB composition.
        let reference = temp.path().join("reference-composition");
        create_private_directory(&reference).unwrap();
        compose_pixels(&plan, &reference, true, None).unwrap();
        let rgb = decode_rgb(&reference.join("composed.mkv"), false);
        assert!(
            rgb.as_chunks::<3>()
                .0
                .iter()
                .any(|p| return p[0].abs_diff(p[1]) > 64 || p[1].abs_diff(p[2]) > 64),
            "color-error fixture must contain real colored application pixels, not a black crop"
        );

        let mut metrics = Vec::new();
        let frame_bytes = usize::from(plan.output_width) * usize::from(plan.output_height) * 3;

        for (format, gif) in [("gif", true), ("webm", false)] {
            let actual = decode_rgb(&directory.join(format!("presentation.{format}")), gif);
            assert_eq!(actual.len(), rgb.len());

            for (frame, (original, encoded)) in rgb
                .chunks_exact(frame_bytes)
                .zip(actual.chunks_exact(frame_bytes))
                .enumerate()
            {
                let max = original
                    .iter()
                    .zip(encoded)
                    .map(|(a, b)| return a.abs_diff(*b))
                    .max()
                    .unwrap();

                let mean = original
                    .iter()
                    .zip(encoded)
                    .map(|(a, b)| return u64::from(a.abs_diff(*b)))
                    .sum::<u64>() as f64
                    / frame_bytes as f64;

                assert!(
                    mean < 20.0,
                    "{format} frame {frame} excessive color error: {mean}"
                );

                metrics.push(serde_json::json!({"format":format,"frame":frame,"max_absolute_channel_error":max,"mean_absolute_channel_error":mean,"bound_mean_absolute_channel_error":20}));
            }
        }

        json(&temp.path().join("decoded-color-errors.json"), &metrics).unwrap();
        // One-frame encodes and explicit final holds retain every opaque frame.
        let mut single = plan.clone();
        single.clips.truncate(1);
        single.clips[0].source_end_frame = single.clips[0].source_start_frame + 1;
        single.clips[0].source_range.end_monotonic_ns =
            single.clips[0].source_range.start_monotonic_ns + 1;

        single.clips[0].speed = RationalSpeed::default();
        single.clips[0].retimed_frame_count = 1;

        for hold in [0, 3] {
            single.clips[0].hold_frame_count = hold;
            single.clips[0].output_end_frame = 1 + hold;
            single.output_frame_count = 1 + hold;
            let out = temp.path().join(format!("one-frame-hold-{hold}"));
            let result = export_derivatives(&single, &session, &out, &options).unwrap();
            verify_derivative_artifact(&out, &result).unwrap();
        }

        let race = temp.path().join("racing-publishers");
        let barrier = Barrier::new(2);
        let race_options = DerivativeExportOptions {
            formats: vec![DerivativeFormat::Gif {
                loop_policy: GifLoop::Once,
            }],
            source_kind: ArtifactSourceKind::NativeReplay,
        };

        let results = thread::scope(|scope| {
            let first = scope.spawn(|| {
                barrier.wait();

                return export_derivatives(&single, &session, &race, &race_options);
            });
            let second = scope.spawn(|| {
                barrier.wait();

                return export_derivatives(&single, &session, &race, &race_options);
            });

            return vec![first.join().unwrap(), second.join().unwrap()];
        });
        assert_eq!(results.iter().filter(|r| return r.is_ok()).count(), 1);

        let winner = results.into_iter().find_map(Result::ok).unwrap();
        verify_derivative_artifact(&race, &winner).unwrap();

        verify_artifact_members(&capture_directory, &capture_members).unwrap();

        assert_eq!(
            hash_file(&capture_directory.join("manifest.json")).unwrap(),
            manifest_hash
        );
        assert_eq!(fs_err::read(&sentinel).unwrap(), b"x");

        let after: Vec<_> = ["session.json", "events.jsonl", "completion.json"]
            .map(|f| return hash_file(&session.join(f)).unwrap())
            .into();
        assert_eq!(before, after);

        // Source tampering after planning must fail before encoding/publication.
        let source_bytes = fs_err::read(capture_directory.join("session.mkv")).unwrap();
        fs_err::write(capture_directory.join("session.mkv"), b"tampered pixels").unwrap();
        let tampered = temp.path().join("tampered-export");
        assert!(export_derivatives(&plan, &session, &tampered, &options).is_err());
        assert!(!tampered.exists());

        // Restore only this isolated test-created fixture for retained positive evidence.
        fs_err::write(capture_directory.join("session.mkv"), source_bytes).unwrap();
        verify_artifact_members(&capture_directory, &capture_members).unwrap();

        if env::var_os("TERMTRACE_MEDIA_TEST_OUTPUT").is_some() {
            eprintln!("retained native evidence: {}", temp.keep().display());
        }
    }
}
