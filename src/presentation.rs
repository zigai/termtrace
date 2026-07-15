//! Marker-resolved presentation edit plans over immutable native captures.

use std::{
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    artifact::{
        ArtifactMember, collect_artifact_members, commit_directory, create_private_directory,
        finish_writer, secure_writer, verify_artifact_members, write_secure,
    },
    event::Event,
    event_log::EventLog,
    replay::StateSelection,
    session::validate_completed_log,
    timeline::{SourceRange, resolve_source_selection},
    visual::{VisualManifest, visual_directory_name_for},
};

/// Current edit-plan format version.
pub const EDIT_PLAN_VERSION: u32 = 1;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// One immutable presentation edit plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditPlan {
    /// Edit-plan format version.
    pub version: u32,
    /// Caller-ordered source clips.
    pub clips: Vec<EditClip>,
}

impl EditPlan {
    /// Parse and validate one strict edit-plan document.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs_err::read(path).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        let plan: Self = serde_json::from_slice(&bytes).map_err(|source| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        plan.validate()?;
        return Ok(plan);
    }

    fn validate(&self) -> Result<()> {
        if self.version != EDIT_PLAN_VERSION || self.clips.is_empty() {
            return Err(plan_error(
                "edit plan must use version 1 and contain at least one clip",
            ));
        }
        for clip in &self.clips {
            clip.from.validate()?;
            clip.to.validate()?;
            if clip.speed.numerator == 0 || clip.speed.denominator == 0 {
                return Err(plan_error(
                    "clip speed numerator and denominator must be nonzero",
                ));
            }
        }
        return Ok(());
    }
}

/// One source interval with rational speed and a generated final hold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditClip {
    /// Inclusive source selector.
    pub from: EditSelector,
    /// Exclusive source selector.
    pub to: EditSelector,
    /// Playback speed as an exact rational.
    #[serde(default)]
    pub speed: RationalSpeed,
    /// Generated hold duration after the final retimed source frame.
    #[serde(default)]
    pub hold_ms: u64,
    /// Optional human presentation caption.
    pub caption: Option<String>,
}

/// Unique marker or exact canonical monotonic timestamp.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditSelector {
    /// Unique canonical marker label.
    pub marker: Option<String>,
    /// Exact canonical session offset in nanoseconds.
    pub monotonic_ns: Option<u64>,
}

impl EditSelector {
    fn validate(&self) -> Result<()> {
        if self.marker.is_some() == self.monotonic_ns.is_some() {
            return Err(plan_error(
                "edit selector requires exactly one of marker or monotonicNs",
            ));
        }
        if self.marker.as_deref().is_some_and(str::is_empty) {
            return Err(plan_error("edit marker must not be empty"));
        }
        return Ok(());
    }
}

/// Exact positive playback speed numerator/denominator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RationalSpeed {
    /// Source-time multiplier numerator.
    pub numerator: u32,
    /// Source-time multiplier denominator.
    pub denominator: u32,
}

impl Default for RationalSpeed {
    fn default() -> Self {
        return Self {
            numerator: 1,
            denominator: 1,
        };
    }
}

/// Resolved clip and exact source/output frame decisions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalizedClip {
    /// Canonical half-open source interval.
    pub source_range: SourceRange,
    /// Inclusive source video frame.
    pub source_start_frame: u64,
    /// Exclusive source video frame.
    pub source_end_frame: u64,
    /// First output frame for this clip.
    pub output_start_frame: u64,
    /// Exclusive output frame after retiming and hold.
    pub output_end_frame: u64,
    /// Retimed source-derived frame count.
    pub retimed_frame_count: u64,
    /// Generated hold frame count.
    pub hold_frame_count: u64,
    /// Exact playback speed.
    pub speed: RationalSpeed,
    /// Optional caption retained as presentation content.
    pub caption: Option<String>,
}

/// Dry-run output binding a normalized edit to immutable native evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationPlan {
    /// Presentation format version.
    pub version: u32,
    /// BLAKE3 of canonical input edit-plan JSON.
    pub edit_plan_hash: String,
    /// Immutable native base video.
    pub source_video: PathBuf,
    /// BLAKE3 of the native base video.
    pub source_video_hash: String,
    /// Native capture frame rate.
    pub fps: u16,
    /// Caller-ordered resolved clips.
    pub clips: Vec<NormalizedClip>,
    /// Total output frames after retiming and holds.
    pub output_frame_count: u64,
}

/// One output-frame provenance decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationMapEntry {
    /// Zero-based presentation output frame.
    pub output_frame: u64,
    /// Caller-ordered clip index.
    pub clip_index: usize,
    /// Native base source frame used for this output.
    pub source_frame: u64,
    /// `source` for retimed frames or `hold` for generated duplicates.
    pub operation: String,
    /// Whether a caller caption alters this presentation frame.
    pub captioned: bool,
}

/// Integrity and tool provenance for an edited presentation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationManifest {
    /// Presentation format version.
    pub version: u32,
    /// Exact resolved plan.
    pub plan: PresentationPlan,
    /// First line of the exact FFmpeg build used.
    pub ffmpeg_version: String,
    /// BLAKE3 of the encoded delivery video.
    pub output_video_hash: String,
    /// Whole-file integrity metadata for every non-manifest member.
    pub members: Vec<ArtifactMember>,
}

/// Atomically published edited presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationOutcome {
    /// No-replace presentation artifact directory.
    pub directory: PathBuf,
    /// Edited H.264 MP4 delivery video.
    pub video: PathBuf,
}

/// Canonical marker with duplicate and nominal-frame context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkerRecord {
    /// Marker label.
    pub label: String,
    /// Canonical event sequence.
    pub event_seq: u64,
    /// Canonical session offset.
    pub monotonic_ns: u64,
    /// Nominal source frame at the requested rate.
    pub nominal_frame: u64,
    /// Total occurrences of this label.
    pub occurrence_count: u64,
}

/// List every canonical marker without claiming native presentation.
pub fn markers(session: impl AsRef<Path>, fps: u16) -> Result<Vec<MarkerRecord>> {
    if fps == 0 {
        return Err(plan_error("marker frame rate must be nonzero"));
    }
    let log = EventLog::read(session.as_ref().join("events.jsonl"))?;
    let mut records = Vec::new();
    for record in &log.events {
        let Event::Marker { label, .. } = &record.event else {
            continue;
        };
        let occurrence_count = u64::try_from(
            log.events
                .iter()
                .filter(|candidate| {
                    return matches!(&candidate.event, Event::Marker { label: value, .. } if value == label);
                })
                .count(),
        )
        .unwrap_or(u64::MAX);
        records.push(MarkerRecord {
            label: label.clone(),
            event_seq: record.seq,
            monotonic_ns: record.monotonic_ns,
            nominal_frame: frame_floor(record.monotonic_ns, fps),
            occurrence_count,
        });
    }
    return Ok(records);
}

/// Resolve a strict edit plan against a verified completed native capture.
pub fn plan(session: impl AsRef<Path>, edit: &EditPlan) -> Result<PresentationPlan> {
    edit.validate()?;
    let session = session.as_ref();
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
    let visual = session.join(visual_directory_name_for(None, StateSelection::All));
    let manifest_path = visual.join("manifest.json");
    let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
        return Error::Filesystem {
            path: manifest_path.clone(),
            source,
        };
    })?;
    let manifest: VisualManifest = serde_json::from_slice(&manifest_bytes).map_err(|source| {
        return Error::InvalidJson {
            path: manifest_path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    verify_artifact_members(&visual, &manifest.members)?;
    if manifest.session_id != log.header.session_id {
        return Err(plan_error(
            "native capture session identity does not match canonical evidence",
        ));
    }
    let source_video = visual.join("session.mkv");
    let source_video_hash = hash_file(&source_video)?;
    let edit_plan_hash = blake3::hash(&serde_json::to_vec(edit).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<edit-plan>"),
            json_path: "$".to_owned(),
            source,
        };
    })?)
    .to_hex()
    .to_string();
    let mut output_start_frame = 0_u64;
    let mut clips = Vec::new();
    for clip in &edit.clips {
        let range = resolve_clip(&log, clip)?;
        let capture_start = manifest
            .source_range
            .map_or(0, |capture| return capture.start_monotonic_ns);
        if manifest.source_range.is_some_and(|capture| {
            return range.start_monotonic_ns < capture.start_monotonic_ns
                || range.end_monotonic_ns > capture.end_monotonic_ns;
        }) {
            return Err(plan_error(
                "edit clip lies outside the native capture range",
            ));
        }
        let source_start_frame =
            frame_floor(range.start_monotonic_ns - capture_start, manifest.fps);
        let source_end_frame = frame_ceil(range.end_monotonic_ns - capture_start, manifest.fps);
        let source_frames = source_end_frame.saturating_sub(source_start_frame);
        let retimed_frame_count = ceil_ratio(
            source_frames.saturating_mul(u64::from(clip.speed.denominator)),
            u64::from(clip.speed.numerator),
        );
        let hold_frame_count =
            ceil_ratio(clip.hold_ms.saturating_mul(u64::from(manifest.fps)), 1000);
        let output_end_frame = output_start_frame
            .saturating_add(retimed_frame_count)
            .saturating_add(hold_frame_count);
        clips.push(NormalizedClip {
            source_range: range,
            source_start_frame,
            source_end_frame,
            output_start_frame,
            output_end_frame,
            retimed_frame_count,
            hold_frame_count,
            speed: clip.speed,
            caption: clip.caption.clone(),
        });
        output_start_frame = output_end_frame;
    }
    return Ok(PresentationPlan {
        version: EDIT_PLAN_VERSION,
        edit_plan_hash,
        source_video,
        source_video_hash,
        fps: manifest.fps,
        clips,
        output_frame_count: output_start_frame,
    });
}

/// Export a normalized plan without modifying its immutable source capture.
pub fn export(
    plan: &PresentationPlan,
    destination: impl AsRef<Path>,
) -> Result<PresentationOutcome> {
    if plan.clips.is_empty() || plan.output_frame_count == 0 {
        return Err(plan_error("presentation plan has no output frames"));
    }
    if hash_file(&plan.source_video)? != plan.source_video_hash {
        return Err(plan_error(
            "native source video changed after edit planning",
        ));
    }
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        });
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return plan_error("presentation destination must have a parent"))?;
    fs_err::create_dir_all(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;
    let staging = parent.join(format!(".presentation-{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;
    export_staged(plan, &staging)?;
    commit_directory(parent, &staging, destination)?;
    return Ok(PresentationOutcome {
        directory: destination.to_path_buf(),
        video: destination.join("presentation.mp4"),
    });
}

fn export_staged(plan: &PresentationPlan, staging: &Path) -> Result<PresentationManifest> {
    let filter_path = staging.join("filter.txt");
    let mut filters = Vec::new();
    let captioned = plan.clips.iter().any(|clip| return clip.caption.is_some());
    for (index, clip) in plan.clips.iter().enumerate() {
        let mut filter = format!(
            "[0:v]trim=start_frame={}:end_frame={},setpts=(PTS-STARTPTS)*{}/{}",
            clip.source_start_frame,
            clip.source_end_frame,
            clip.speed.denominator,
            clip.speed.numerator,
        );
        if clip.hold_frame_count > 0 {
            write!(
                &mut filter,
                ",tpad=stop_mode=clone:stop_duration={}.{:09}",
                clip.hold_frame_count / u64::from(plan.fps),
                clip.hold_frame_count % u64::from(plan.fps) * NANOS_PER_SECOND
                    / u64::from(plan.fps),
            )
            .map_err(|_error| return plan_error("could not build FFmpeg hold filter"))?;
        }
        if captioned {
            filter.push_str(",pad=iw:ih+48:0:0:color=black");
            if let Some(caption) = &clip.caption {
                let caption_path = staging.join(format!("caption-{index}.txt"));
                write_secure(&caption_path, caption.as_bytes())?;
                write!(
                    &mut filter,
                    ",drawtext=textfile='{}':fontcolor=white:fontsize=20:x=16:y=h-34",
                    filter_path_value(&caption_path)?,
                )
                .map_err(|_error| return plan_error("could not build FFmpeg caption filter"))?;
            }
        }
        write!(&mut filter, "[v{index}]")
            .map_err(|_error| return plan_error("could not build FFmpeg clip filter"))?;
        filters.push(filter);
    }
    let inputs = (0..plan.clips.len())
        .map(|index| return format!("[v{index}]"))
        .collect::<String>();
    filters.push(format!(
        "{inputs}concat=n={}:v=1:a=0[outv]",
        plan.clips.len()
    ));
    let filter = format!("{};\n", filters.join(";\n"));
    write_secure(&filter_path, filter.as_bytes())?;
    let output = staging.join("presentation.mp4");
    let process = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-y", "-i"])
        .arg(&plan.source_video)
        .args(["-filter_complex_script"])
        .arg(&filter_path)
        .args(["-map", "[outv]", "-an", "-r"])
        .arg(plan.fps.to_string())
        .args(["-frames:v"])
        .arg(plan.output_frame_count.to_string())
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ])
        .arg(&output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;
    write_secure(&staging.join("ffmpeg.log"), &process.stderr)?;
    if !process.status.success() {
        return Err(plan_error(format!(
            "FFmpeg presentation export failed with {}",
            process.status
        )));
    }
    write_presentation_map(&staging.join("presentation-map.jsonl"), plan)?;
    let normalized = serde_json::to_vec_pretty(plan).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("normalized-plan.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    write_secure(&staging.join("normalized-plan.json"), &normalized)?;
    let output_video_hash = hash_file(&output)?;
    let ffmpeg_version = command_version("ffmpeg")?;
    let members = collect_artifact_members(staging)?;
    let manifest = PresentationManifest {
        version: EDIT_PLAN_VERSION,
        plan: plan.clone(),
        ffmpeg_version,
        output_video_hash,
        members,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("manifest.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    write_secure(&staging.join("manifest.json"), &manifest_bytes)?;
    verify_artifact_members(staging, &manifest.members)?;
    return Ok(manifest);
}

fn write_presentation_map(path: &Path, plan: &PresentationPlan) -> Result<()> {
    let mut writer = secure_writer(path)?;
    for (clip_index, clip) in plan.clips.iter().enumerate() {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let offset = output_frame - clip.output_start_frame;
            let (source_frame, operation) = if offset < clip.retimed_frame_count {
                let source_offset = offset.saturating_mul(u64::from(clip.speed.numerator))
                    / u64::from(clip.speed.denominator);
                (
                    clip.source_start_frame
                        .saturating_add(source_offset)
                        .min(clip.source_end_frame.saturating_sub(1)),
                    "source",
                )
            } else {
                (clip.source_end_frame.saturating_sub(1), "hold")
            };
            let entry = PresentationMapEntry {
                output_frame,
                clip_index,
                source_frame,
                operation: operation.to_owned(),
                captioned: clip.caption.is_some(),
            };
            serde_json::to_writer(&mut writer, &entry).map_err(|source| {
                return Error::InvalidJson {
                    path: path.to_path_buf(),
                    json_path: format!("output frame {output_frame}"),
                    source,
                };
            })?;
            writer.write_all(b"\n").map_err(|source| {
                return Error::Filesystem {
                    path: path.to_path_buf(),
                    source,
                };
            })?;
        }
    }
    return finish_writer(writer, path);
}

fn command_version(command: &str) -> Result<String> {
    let output = Command::new(command)
        .arg("-version")
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from(command),
                source,
            };
        })?;
    if !output.status.success() {
        return Err(plan_error(format!("{command} -version failed")));
    }
    return String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| return plan_error(format!("{command} returned no version")));
}

fn filter_path_value(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| return plan_error("FFmpeg filter resource path is not UTF-8"))?;
    return Ok(value
        .replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'"));
}

fn resolve_clip(log: &EventLog, clip: &EditClip) -> Result<SourceRange> {
    return resolve_source_selection(
        log,
        clip.from.monotonic_ns,
        clip.to.monotonic_ns,
        clip.from.marker.as_deref(),
        clip.to.marker.as_deref(),
    )?
    .ok_or_else(|| return plan_error("edit clip must have explicit endpoints"));
}

fn frame_floor(monotonic_ns: u64, fps: u16) -> u64 {
    return monotonic_ns.saturating_mul(u64::from(fps)) / NANOS_PER_SECOND;
}

fn frame_ceil(monotonic_ns: u64, fps: u16) -> u64 {
    return ceil_ratio(
        monotonic_ns.saturating_mul(u64::from(fps)),
        NANOS_PER_SECOND,
    );
}

fn ceil_ratio(numerator: u64, denominator: u64) -> u64 {
    return numerator.saturating_add(denominator - 1) / denominator;
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

fn plan_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_retiming_and_hold_round_up_to_frames() {
        assert_eq!(frame_floor(1_500_000_000, 24), 36);
        assert_eq!(frame_ceil(1_500_000_001, 24), 37);
        assert_eq!(ceil_ratio(25, 4), 7);
        assert_eq!(ceil_ratio(801 * 24, 1000), 20);
    }

    #[test]
    fn edit_selectors_require_one_source_coordinate() {
        let selector = EditSelector {
            marker: Some("before".to_owned()),
            monotonic_ns: Some(10),
        };
        assert!(selector.validate().is_err());
    }

    #[test]
    fn presentation_map_distinguishes_retimed_and_hold_frames() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("presentation-map.jsonl");
        let plan = PresentationPlan {
            version: 1,
            edit_plan_hash: "a".repeat(64),
            source_video: PathBuf::from("source.mkv"),
            source_video_hash: "b".repeat(64),
            fps: 24,
            clips: vec![NormalizedClip {
                source_range: SourceRange {
                    start_monotonic_ns: 0,
                    end_monotonic_ns: 1_000_000_000,
                },
                source_start_frame: 0,
                source_end_frame: 24,
                output_start_frame: 0,
                output_end_frame: 8,
                retimed_frame_count: 6,
                hold_frame_count: 2,
                speed: RationalSpeed {
                    numerator: 4,
                    denominator: 1,
                },
                caption: Some("fast clip".to_owned()),
            }],
            output_frame_count: 8,
        };
        write_presentation_map(&path, &plan).expect("write frame map");
        let entries = fs_err::read_to_string(path)
            .expect("read map")
            .lines()
            .map(|line| {
                return serde_json::from_str::<PresentationMapEntry>(line).expect("map entry");
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 8);
        assert_eq!(entries[5].source_frame, 20);
        assert_eq!(entries[6].operation, "hold");
        assert!(entries.iter().all(|entry| return entry.captioned));
    }

    #[test]
    #[ignore = "requires FFmpeg with libx264"]
    fn exports_multi_clip_h264_presentation_atomically() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = temporary.path().join("source.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x64:rate=24:duration=1",
                "-c:v",
                "ffv1",
            ])
            .arg(&source)
            .status()
            .expect("run FFmpeg fixture");
        assert!(status.success());
        let plan = PresentationPlan {
            version: 1,
            edit_plan_hash: "a".repeat(64),
            source_video_hash: hash_file(&source).expect("source hash"),
            source_video: source,
            fps: 24,
            clips: vec![NormalizedClip {
                source_range: SourceRange {
                    start_monotonic_ns: 0,
                    end_monotonic_ns: 500_000_000,
                },
                source_start_frame: 0,
                source_end_frame: 12,
                output_start_frame: 0,
                output_end_frame: 8,
                retimed_frame_count: 6,
                hold_frame_count: 2,
                speed: RationalSpeed {
                    numerator: 2,
                    denominator: 1,
                },
                caption: None,
            }],
            output_frame_count: 8,
        };
        let destination = temporary.path().join("presentation");
        let outcome = export(&plan, &destination).expect("export presentation");
        assert!(outcome.video.is_file());
        assert!(outcome.directory.join("manifest.json").is_file());
        assert!(outcome.directory.join("presentation-map.jsonl").is_file());
    }
}
