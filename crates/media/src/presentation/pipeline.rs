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
    NANOS_PER_SECOND,
};
use crate::{Error, Result};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
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

fn require_presentation_tools() -> Result<()> {
    command_version("ffmpeg")?;
    command_version("ffprobe")?;

    return Ok(());
}

/// Export a normalized plan without modifying its immutable source evidence.
///
/// # Errors
///
/// Returns an error when the plan has no output frames, its source no longer verifies, the
/// destination exists or has no parent, presentation tools are unavailable, or rendering, encoding,
/// or publication fails.
pub fn export(
    plan: &PresentationPlan,
    destination: impl AsRef<Path>,
) -> Result<PresentationOutcome> {
    return export_with_capability_check(plan, destination.as_ref(), true);
}

pub(super) fn export_with_capability_check(
    plan: &PresentationPlan,
    destination: &Path,
    check_capabilities: bool,
) -> Result<PresentationOutcome> {
    if plan.clips.is_empty() || plan.output_frame_count == 0 {
        return Err(plan_error("presentation plan has no output frames"));
    }

    validate_planned_source(&plan.source)?;

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
    export_staged(plan, &staging)?;
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
/// visualization, the plan's source no longer verifies, the destination exists or has no parent,
/// presentation tools are unavailable, or rendering or publication fails.
pub fn preview(
    plan: &PresentationPlan,
    destination: impl AsRef<Path>,
    options: PreviewOptions,
) -> Result<PresentationPreviewOutcome> {
    validate_preview_options(options)?;
    if options.change_only && plan.change_visualization.is_none() {
        return Err(plan_error(
            "change-only preview requires change_visualization in the edit plan",
        ));
    }

    validate_planned_source(&plan.source)?;

    let destination = destination.as_ref();

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
    let rendered = export_with_capability_check(plan, &composed, false)?;
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

fn export_staged(plan: &PresentationPlan, staging: &Path) -> Result<PresentationManifest> {
    let source_video = materialize_planned_source(plan, staging)?;
    let filter_path = staging.join("filter.txt");
    let mut filters = Vec::new();
    let captioned = plan.layout.caption != CaptionLayout::None
        && plan.clips.iter().any(|clip| return clip.caption.is_some());

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

    let ffmpeg_arguments = vec![
        "-hide_banner".to_owned(),
        "-nostdin".to_owned(),
        "-threads".to_owned(),
        "1".to_owned(),
        "-y".to_owned(),
        "-i".to_owned(),
        source_video.display().to_string(),
        "-filter_complex".to_owned(),
        filter.trim().to_owned(),
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

    let process = Command::new("ffmpeg")
        .args(&ffmpeg_arguments)
        .current_dir(staging)
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
            "FFmpeg presentation export failed with {}: {}",
            process.status,
            String::from_utf8_lossy(&process.stderr)
        )));
    }

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
        source_truth_layer: presentation_source_truth_layer(&plan.source).to_owned(),
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
