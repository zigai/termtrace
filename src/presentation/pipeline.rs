use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

use crate::{
    Error, Result,
    artifact::{
        collect_artifact_members, commit_directory, create_private_directory,
        verify_artifact_members, write_secure,
    },
    capability::{
        PRESENTATION_CAPABILITY, SEMANTIC_VIDEO_CAPABILITY, require_capabilities,
        require_capability,
    },
};

use super::{
    NANOS_PER_SECOND,
    ffmpeg::{
        append_change_visualization, append_pixel_transforms, command_version, filter_path_value,
        prepare_subtitle_resources, probe_delivery_video, render_contact_sheet,
        render_low_resolution_preview, write_source_frame_hashes,
    },
    mapping::{read_semantic_frame_spans, write_presentation_map},
    model::*,
    plan_error,
    planning::{materialize_planned_source, validate_planned_source},
};

fn require_presentation_capabilities(plan: &PresentationPlan) -> Result<()> {
    return match &plan.source {
        PresentationSource::Native { .. } => require_capability(PRESENTATION_CAPABILITY),
        PresentationSource::Semantic { .. } => {
            require_capabilities(&[SEMANTIC_VIDEO_CAPABILITY, PRESENTATION_CAPABILITY])
        }
    };
}

/// Export a normalized plan without modifying its immutable source evidence.
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
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        });
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return plan_error("presentation destination must have a parent"))?;
    if check_capabilities {
        require_presentation_capabilities(plan)?;
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
    commit_directory(parent, &staging, destination)?;
    return Ok(PresentationOutcome {
        directory: destination.to_path_buf(),
        video: destination.join("presentation.mp4"),
    });
}

/// Render an atomic contact sheet and low-resolution preview of a normalized edit.
pub fn preview(
    plan: &PresentationPlan,
    destination: impl AsRef<Path>,
    options: PreviewOptions,
) -> Result<PresentationPreviewOutcome> {
    options.validate()?;
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
    require_presentation_capabilities(plan)?;
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
    thread::scope(|scope| {
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
        source_identity_hash: plan.source.identity_hash()?,
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
    commit_directory(parent, &staging, destination)?;
    return Ok(PresentationPreviewOutcome {
        directory: destination.to_path_buf(),
        contact_sheet: destination.join("contact-sheet.png"),
        video: destination.join("preview.mp4"),
    });
}

struct DirectoryCleanup(PathBuf);

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        let _result = fs_err::remove_dir_all(&self.0);
    }
}

pub(super) fn select_preview_frames(
    plan: &PresentationPlan,
    options: PreviewOptions,
) -> Result<Vec<u64>> {
    let candidates = if options.change_only {
        let mut frames = BTreeSet::new();
        match plan
            .change_visualization
            .as_ref()
            .map(|change| return change.source)
        {
            Some(ChangeSource::SemanticCells) => {
                let hold = plan.change_visualization.as_ref().map_or(0, |change| {
                    return u64::from(change.hold_frames);
                });
                for change in &plan.change_frames {
                    for frame in &change.output_frames {
                        for held in *frame
                            ..=frame
                                .saturating_add(hold)
                                .min(plan.output_frame_count.saturating_sub(1))
                        {
                            frames.insert(held);
                        }
                    }
                }
            }
            Some(ChangeSource::EncodedPixels) => {
                frames.extend(1..plan.output_frame_count);
            }
            None => {}
        }
        frames.into_iter().collect::<Vec<_>>()
    } else {
        (0..plan.output_frame_count).collect::<Vec<_>>()
    };
    if candidates.is_empty() {
        return Err(plan_error("preview selection contains no frames"));
    }
    let count = usize::from(options.max_frames).min(candidates.len());
    if count == 1 {
        return candidates
            .first()
            .copied()
            .map(|frame| return vec![frame])
            .ok_or_else(|| return plan_error("preview selection contains no frames"));
    }
    let mut selected = Vec::with_capacity(count);
    for index in 0..count {
        let candidate_index =
            index.saturating_mul(candidates.len().saturating_sub(1)) / count.saturating_sub(1);
        if let Some(frame) = candidates.get(candidate_index) {
            selected.push(*frame);
        }
    }
    selected.dedup();
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
        write_source_frame_hashes(plan, &source_video, staging)?;
    }
    let encoder = match plan.delivery.codec.as_str() {
        "h264_rgb" => "libx264rgb",
        _ => "libx264",
    };
    let ffmpeg_arguments = vec![
        "-hide_banner".to_owned(),
        "-nostdin".to_owned(),
        "-y".to_owned(),
        "-i".to_owned(),
        source_video.display().to_string(),
        "-/filter_complex".to_owned(),
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
            "FFmpeg presentation export failed with {}",
            process.status
        )));
    }
    let (output_probe, ffmpeg_version) = thread::scope(|scope| {
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
            return write_secure(&staging.join("normalized-plan.json"), &normalized);
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
        source_truth_layer: plan.source.truth_layer().to_owned(),
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
