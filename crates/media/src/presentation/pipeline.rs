use super::{
    ffmpeg::{
        append_change_visualization, append_pixel_transforms, command_version,
        prepare_subtitle_resources, probe_delivery_video, render_contact_sheet,
        render_low_resolution_preview, write_source_frame_hashes,
    },
    filter_path_value,
    mapping::{read_semantic_frame_spans, write_presentation_map},
    plan_error,
    planning::{materialize_planned_source, validate_planned_source},
    validation::{
        presentation_source_identity_hash, presentation_source_truth_layer,
        validate_preview_options,
    },
};
use crate::{Error, Result};
use std::{
    fmt::Write as _,
    path::{self, Path, PathBuf},
    sync::atomic::AtomicBool,
    thread,
    time::Duration,
};
use termtrace_evidence::artifact::{
    collect_artifact_members, commit_directory, create_private_directory, verify_artifact_members,
    write_secure,
};
use termtrace_model::artifact::presentation::{
    CaptionLayout, ChangeSource, PresentationManifest, PresentationOutcome, PresentationPlan,
    PresentationPreviewManifest, PresentationPreviewOutcome, PresentationSource, PreviewOptions,
    PRESENTATION_FORMAT_VERSION,
};
use termtrace_model::artifact::sdk::ArtifactSourceKind;

fn require_presentation_tools() -> Result<()> {
    command_version("ffmpeg")?;
    command_version("ffprobe")?;

    return Ok(());
}

/// Export a legacy MP4 without modifying source evidence.
///
/// This legacy API intentionally retains destination replacement compatibility.
/// New GIF/WebM `export_derivatives` publication is strictly no-replace.
///
/// # Errors
///
/// Returns an error when the plan has no output frames, its source no longer verifies or does
/// not match `source_kind`, the destination has no parent, presentation tools are unavailable,
/// or rendering, encoding, or publication fails.
pub fn export(
    plan: &PresentationPlan,
    destination: impl AsRef<Path>,
    source_kind: ArtifactSourceKind,
) -> Result<PresentationOutcome> {
    return export_with_capability_check(plan, destination.as_ref(), source_kind, true);
}

pub(super) fn export_with_capability_check(
    plan: &PresentationPlan,
    destination: &Path,
    source_kind: ArtifactSourceKind,
    check_capabilities: bool,
) -> Result<PresentationOutcome> {
    if plan.clips.is_empty() || plan.output_frame_count == 0 {
        return Err(plan_error("presentation plan has no output frames"));
    }

    validate_planned_source(&plan.source)?;
    let source_truth_layer = presentation_source_truth_layer(&plan.source, source_kind)?;

    let parent = destination
        .parent()
        .ok_or_else(|| return plan_error("presentation destination must have a parent"))?;

    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };

    if check_capabilities {
        require_presentation_tools()?;
    }

    fs_err::create_dir_all(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;

    let staging = parent.join(format!(".presentation-{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;
    let _staging_cleanup = DirectoryCleanup(staging.clone());
    export_staged(plan, &staging, source_truth_layer)?;
    commit_directory(parent, &staging, destination, true)?;

    return Ok(PresentationOutcome {
        directory: destination.to_path_buf(),
        video: destination.join("presentation.mp4"),
    });
}

/// Render an atomic contact sheet and low-resolution preview of a normalized edit.
///
/// # Errors
///
/// Returns an error when the preview options are invalid, a change-only preview lacks change
/// visualization, the plan's source no longer verifies or does not match `source_kind`, the
/// destination exists or has no parent, presentation tools are unavailable, or rendering or
/// publication fails.
pub fn preview(
    plan: &PresentationPlan,
    destination: impl AsRef<Path>,
    source_kind: ArtifactSourceKind,
    options: PreviewOptions,
) -> Result<PresentationPreviewOutcome> {
    validate_preview_options(options)?;
    if options.change_only && plan.change_visualization.is_none() {
        return Err(plan_error(
            "change-only preview requires change_visualization in the edit plan",
        ));
    }

    validate_planned_source(&plan.source)?;

    // FFmpeg runs inside the staging directory, so no path it reads may be relative.
    let destination = path::absolute(destination.as_ref()).map_err(|source| {
        return Error::Filesystem {
            path: destination.as_ref().to_path_buf(),
            source,
        };
    })?;

    let destination = destination.as_path();
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        });
    }

    let parent = destination
        .parent()
        .ok_or_else(|| return plan_error("preview destination must have a parent"))?;

    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    require_presentation_tools()?;
    fs_err::create_dir_all(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;

    let composed = parent.join(format!(".preview-source-{}.tmp", uuid::Uuid::now_v7()));
    let _composed_cleanup = DirectoryCleanup(composed.clone());
    let rendered = export_with_capability_check(plan, &composed, source_kind, false)?;
    let staging = parent.join(format!(".preview-{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;
    let _staging_cleanup = DirectoryCleanup(staging.clone());
    let selected_frames = select_preview_frames(plan, options)?;
    write_secure(
        &staging.join("selected-frames.json"),
        &serde_json::to_vec_pretty(&selected_frames).map_err(|source| {
            return Error::InvalidJson {
                path: staging.join("selected-frames.json"),
                json_path: "$".to_owned(),
                source,
            };
        })?,
    )?;

    let normalized = serde_json::to_vec_pretty(plan).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("normalized-plan.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    write_secure(&staging.join("normalized-plan.json"), &normalized)?;

    let source_manifest =
        fs_err::read(rendered.directory.join("manifest.json")).map_err(|source| {
            return Error::Filesystem {
                path: rendered.directory.join("manifest.json"),
                source,
            };
        })?;

    write_secure(
        &staging.join("source-presentation-manifest.json"),
        &source_manifest,
    )?;

    thread::scope(|scope| -> Result<()> {
        let contact_sheet = scope.spawn(|| {
            return render_contact_sheet(&rendered.video, &staging, &selected_frames, options);
        });
        let preview_video = scope.spawn(|| {
            return render_low_resolution_preview(&rendered.video, &staging, options.preview_width);
        });

        contact_sheet
            .join()
            .map_err(|_panic| return plan_error("contact-sheet rendering worker panicked"))??;

        preview_video
            .join()
            .map_err(|_panic| return plan_error("preview-video rendering worker panicked"))??;

        return Ok(());
    })?;

    let members = collect_artifact_members(&staging)?;
    let manifest = PresentationPreviewManifest {
        version: PRESENTATION_FORMAT_VERSION,
        edit_plan_hash: plan.edit_plan_hash.clone(),
        source_identity_hash: presentation_source_identity_hash(&plan.source)?,
        truth_layer: "presentation_preview".to_owned(),
        options,
        selected_frames,
        ffmpeg_version: command_version("ffmpeg")?,
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
    verify_artifact_members(&staging, &manifest.members)?;
    commit_directory(parent, &staging, destination, false)?;

    return Ok(PresentationPreviewOutcome {
        directory: destination.to_path_buf(),
        contact_sheet: destination.join("contact-sheet.png"),
        video: destination.join("preview.mp4"),
    });
}

struct DirectoryCleanup(PathBuf);

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        let _result =
            if fs_err::symlink_metadata(&self.0).is_ok_and(|metadata| return metadata.is_dir()) {
                fs_err::remove_dir_all(&self.0)
            } else {
                fs_err::remove_file(&self.0)
            };
    }
}

pub(super) fn select_preview_frames(
    plan: &PresentationPlan,
    options: PreviewOptions,
) -> Result<Vec<u64>> {
    let mut intervals = if !options.change_only {
        vec![(0, plan.output_frame_count)]
    } else {
        match plan.change_visualization.as_ref() {
            Some(change) if change.source == ChangeSource::SemanticCells => {
                let hold = u64::from(change.hold_frames);
                let mut intervals = plan
                    .change_frames
                    .iter()
                    .flat_map(|change| {
                        return change.output_frames.iter().map(|frame| {
                            return (
                                *frame,
                                frame
                                    .saturating_add(hold)
                                    .saturating_add(1)
                                    .min(plan.output_frame_count),
                            );
                        });
                    })
                    .filter(|(start, end)| return start < end)
                    .collect::<Vec<_>>();
                intervals.sort_unstable();
                intervals
            }
            Some(_) => vec![(1, plan.output_frame_count)],
            None => Vec::new(),
        }
    };

    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals
        .drain(..)
        .filter(|(start, end)| return start < end)
    {
        if let Some(prior) = merged.last_mut()
            && start <= prior.1
        {
            prior.1 = prior.1.max(end);
        } else {
            merged.push((start, end));
        }
    }

    let candidates: u64 = merged.iter().map(|(start, end)| return end - start).sum();
    let count = u64::from(options.max_frames).min(candidates);

    if count == 0 {
        return Err(plan_error("preview selection contains no frames"));
    }

    let mut selected = Vec::with_capacity(usize::from(options.max_frames));
    let mut intervals = merged.into_iter();
    let mut interval = intervals.next();

    let mut preceding = 0;
    for index in 0..count {
        let candidate_index = if count == 1 {
            0
        } else {
            u64::try_from(u128::from(index) * u128::from(candidates - 1) / u128::from(count - 1))
                .map_err(|_error| return plan_error("preview sample index overflowed"))?
        };

        while let Some((start, end)) = interval {
            let length = end - start;
            if candidate_index - preceding < length {
                selected.push(start + candidate_index - preceding);
                break;
            }

            preceding += length;
            interval = intervals.next();
        }
    }

    return Ok(selected);
}

/// Shared composition path. Derivatives use FFV1 RGB, never the documentation MP4.
pub(super) fn compose_pixels(
    plan: &PresentationPlan,
    staging: &Path,
    lossless: bool,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<String>> {
    if plan.fps == 0 || plan.output_frame_count == 0 || plan.clips.is_empty() {
        return Err(plan_error(
            "composition requires positive CFR FPS and frames",
        ));
    }

    validate_normalized_frames(plan)?;
    let source_video = materialize_planned_source(plan, staging)?;
    let filter_path = staging.join("filter.txt");
    let mut filters = Vec::new();
    let captioned = plan.layout.caption != CaptionLayout::None
        && plan.clips.iter().any(|clip| return clip.caption.is_some());

    for (index, clip) in plan.clips.iter().enumerate() {
        // Integer source-frame mapping is the same floor(offset*num/den) used by
        // presentation-map.jsonl. Round input boundaries upward, so each output
        // uses the last source frame whose exact rational boundary has arrived.
        filters.push(format!(
            "[0:v]trim=start_frame={}:end_frame={},settb=expr=1/({}*{}),setpts=N*{},fps=fps={}:start_time=0:round=up,tpad=stop_mode=clone:stop={},trim=end_frame={},setpts=N/({}*TB)[retimed{index}]",
            clip.source_start_frame, clip.source_end_frame, plan.fps, clip.speed.numerator,
            clip.speed.denominator, plan.fps, clip.retimed_frame_count, clip.retimed_frame_count, plan.fps,
        ));
        let mut filter = if clip.hold_frame_count > 0 {
            filters.push(format!(
                "[0:v]trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop={},trim=end_frame={},setpts=N/({}*TB)[hold{index}]",
                clip.source_end_frame - 1, clip.source_end_frame, clip.hold_frame_count,
                clip.hold_frame_count, plan.fps,
            ));

            format!("[retimed{index}][hold{index}]concat=n=2:v=1:a=0")
        } else {
            format!("[retimed{index}]null")
        };

        if captioned && plan.layout.caption == CaptionLayout::Footer {
            filter.push_str(",pad=iw:ih+48:0:0:color=black");
        }

        if captioned && let Some(caption) = &clip.caption {
            let caption_name = format!("caption-{index}.txt");
            let caption_path = staging.join(&caption_name);
            write_secure(&caption_path, caption.as_bytes())?;
            write!(
                &mut filter,
                ",drawtext=textfile='{}':fontcolor=white:fontsize=20:box=1:boxcolor=black@0.65:x=16:y=h-34",
                filter_path_value(Path::new(&caption_name))?,
            )
            .map_err(|_error| return plan_error("could not build FFmpeg caption filter"))?;
        }

        write!(&mut filter, "[v{index}]")
            .map_err(|_error| return plan_error("could not build FFmpeg clip filter"))?;
        filters.push(filter);
    }

    let inputs = (0..plan.clips.len())
        .map(|index| return format!("[v{index}]"))
        .collect::<String>();
    filters.push(format!(
        "{inputs}concat=n={}:v=1:a=0[joined]",
        plan.clips.len()
    ));

    let subtitle_resources = prepare_subtitle_resources(plan, staging)?;
    let changed = append_change_visualization(&mut filters, plan, "joined")?;
    let mut prior = append_pixel_transforms(&mut filters, plan, staging, &changed)?;

    for (index, resource) in subtitle_resources.iter().enumerate() {
        let next = format!("subtitle{index}");
        filters.push(format!(
            "[{}]ass=filename='{}':fontsdir='{}'[{}]",
            prior,
            filter_path_value(resource.file_name().map(Path::new).ok_or_else(|| {
                return plan_error("subtitle resource has no filename");
            })?)?,
            filter_path_value(Path::new("fonts"))?,
            next,
        ));

        prior = next;
    }

    filters.push(format!("[{prior}]null[outv]"));
    let filter = format!("{};\n", filters.join(";\n"));
    write_secure(&filter_path, filter.as_bytes())?;

    if plan.change_visualization.as_ref().is_some_and(|change| {
        return change.source == ChangeSource::EncodedPixels;
    }) {
        write_source_frame_hashes(&source_video, staging)?;
    }

    let encoder = match plan.delivery.codec.as_str() {
        "h264" => "libx264",
        "h264_rgb" => "libx264rgb",
        codec => {
            return Err(plan_error(format!(
                "unsupported presentation delivery codec `{codec}`"
            )));
        }
    };

    let mut ffmpeg_arguments = vec![
        "-hide_banner".to_owned(),
        "-nostdin".to_owned(),
        "-threads".to_owned(),
        "1".to_owned(),
        "-filter_complex_threads".to_owned(),
        "1".to_owned(),
        "-y".to_owned(),
        "-i".to_owned(),
        source_video
            .to_str()
            .ok_or_else(|| return plan_error("FFmpeg source path must be UTF-8"))?
            .to_owned(),
        // A per-cell overlay graph exceeds the kernel's 128 KiB limit for one argument.
        filter_script_option()?.to_owned(),
        "filter.txt".to_owned(),
        "-map".to_owned(),
        "[outv]".to_owned(),
        "-an".to_owned(),
        "-r".to_owned(),
        plan.fps.to_string(),
        "-frames:v".to_owned(),
        plan.output_frame_count.to_string(),
        "-c:v".to_owned(),
        encoder.to_owned(),
        "-threads".to_owned(),
        "1".to_owned(),
        "-preset".to_owned(),
        plan.delivery.encoder_preset.clone(),
        "-crf".to_owned(),
        plan.delivery.crf.to_string(),
        "-pix_fmt".to_owned(),
        plan.delivery.pixel_format.clone(),
        "-movflags".to_owned(),
        "+faststart".to_owned(),
        "presentation.mp4".to_owned(),
    ];

    if lossless {
        // Replace only delivery settings; graph, source and map are shared.
        let delivery_start = ffmpeg_arguments
            .iter()
            .position(|v| return v == "-c:v")
            .ok_or_else(|| return plan_error("composition encoder argument missing"))?;
        ffmpeg_arguments.truncate(delivery_start);
        ffmpeg_arguments.extend(
            [
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-threads",
                "1",
                "-pix_fmt",
                "bgr0",
                "composed.mkv",
            ]
            .map(str::to_owned),
        );
    }

    super::ffmpeg::run_logged_command(
        "ffmpeg",
        &ffmpeg_arguments,
        staging,
        "composition",
        cancelled,
        Duration::from_secs(86_400),
    )?;

    validate_planned_source(&plan.source)?;

    return Ok(ffmpeg_arguments);
}

// FFmpeg 7 reads option values from files with `-/option FILE`; older releases only have
// `-filter_complex_script FILE`, which FFmpeg 8 removed.
fn filter_script_option() -> Result<&'static str> {
    let version = command_version("ffmpeg")?;
    let major = version
        .strip_prefix("ffmpeg version ")
        .map(|number| return number.trim_start_matches('n'))
        .and_then(|number| return number.split(['.', '-']).next())
        .and_then(|major| return major.parse::<u32>().ok());

    return Ok(match major {
        Some(major) if major < 7 => "-filter_complex_script",
        _ => "-/filter_complex",
    });
}

/// Check frame arithmetic before trusting a caller-supplied mutable normalized plan.
pub(super) fn validate_normalized_frames(plan: &PresentationPlan) -> Result<()> {
    if plan.output_width == 0
        || plan.output_height == 0
        || !plan.output_width.is_multiple_of(2)
        || !plan.output_height.is_multiple_of(2)
    {
        return Err(plan_error("composition requires a positive even canvas"));
    }

    let mut next = 0u64;
    for clip in &plan.clips {
        let frames = clip
            .source_end_frame
            .checked_sub(clip.source_start_frame)
            .filter(|n| return *n > 0)
            .ok_or_else(|| return plan_error("empty or reversed source frames"))?;

        if clip.speed.numerator == 0 || clip.speed.denominator == 0 {
            return Err(plan_error("zero rational speed"));
        }

        let retimed = u128::from(frames) * u128::from(clip.speed.denominator);
        let retimed = retimed.div_ceil(u128::from(clip.speed.numerator));

        if retimed != u128::from(clip.retimed_frame_count) || clip.output_start_frame != next {
            return Err(plan_error("normalized retiming/map boundaries differ"));
        }

        next = next
            .checked_add(clip.retimed_frame_count)
            .and_then(|n| return n.checked_add(clip.hold_frame_count))
            .ok_or_else(|| return plan_error("normalized output frame overflow"))?;
        if clip.output_end_frame != next {
            return Err(plan_error("normalized clip end differs"));
        }
    }

    if next != plan.output_frame_count {
        return Err(plan_error("normalized total frame count differs"));
    }
    return Ok(());
}

fn export_staged(
    plan: &PresentationPlan,
    staging: &Path,
    source_truth_layer: &str,
) -> Result<PresentationManifest> {
    let ffmpeg_arguments = compose_pixels(plan, staging, false, None)?;
    let (output_probe, ffmpeg_version) =
        thread::scope(|scope| -> Result<(serde_json::Value, String)> {
            let probe = scope.spawn(|| -> Result<serde_json::Value> {
                let output_probe = probe_delivery_video(staging)?;
                write_secure(
                    &staging.join("output-probe.json"),
                    &serde_json::to_vec_pretty(&output_probe).map_err(|source| {
                        return Error::InvalidJson {
                            path: staging.join("output-probe.json"),
                            json_path: "$".to_owned(),
                            source,
                        };
                    })?,
                )?;

                return Ok(output_probe);
            });
            let frame_map = scope.spawn(|| -> Result<()> {
                let semantic_spans = if matches!(plan.source, PresentationSource::Semantic { .. }) {
                    read_semantic_frame_spans(&staging.join("semantic-source-map.jsonl"))?
                } else {
                    Vec::new()
                };

                return write_presentation_map(
                    &staging.join("presentation-map.jsonl"),
                    plan,
                    &semantic_spans,
                );
            });
            let normalized_plan = scope.spawn(|| -> Result<()> {
                let normalized = serde_json::to_vec_pretty(plan).map_err(|source| {
                    return Error::InvalidJson {
                        path: staging.join("normalized-plan.json"),
                        json_path: "$".to_owned(),
                        source,
                    };
                })?;

                return Ok(write_secure(
                    &staging.join("normalized-plan.json"),
                    &normalized,
                )?);
            });
            let version = scope.spawn(|| return command_version("ffmpeg"));

            let output_probe = probe
                .join()
                .map_err(|_panic| return plan_error("output-probe worker panicked"))??;

            frame_map
                .join()
                .map_err(|_panic| return plan_error("presentation-map worker panicked"))??;

            normalized_plan
                .join()
                .map_err(|_panic| return plan_error("normalized-plan worker panicked"))??;

            let ffmpeg_version = version
                .join()
                .map_err(|_panic| return plan_error("FFmpeg-version worker panicked"))??;

            return Ok((output_probe, ffmpeg_version));
        })?;

    let members = collect_artifact_members(staging)?;
    let output_video_hash = members
        .iter()
        .find(|member| return member.path == "presentation.mp4")
        .map(|member| return member.blake3.clone())
        .ok_or_else(|| return plan_error("presentation member list has no output video"))?;

    let manifest = PresentationManifest {
        version: PRESENTATION_FORMAT_VERSION,
        plan: plan.clone(),
        ffmpeg_version,
        ffmpeg_arguments,
        output_probe,
        truth_layer: "presentation_output".to_owned(),
        source_truth_layer: source_truth_layer.to_owned(),
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
