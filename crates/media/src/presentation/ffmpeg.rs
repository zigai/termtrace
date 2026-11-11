use super::{
    filter_path_value, mapping::mapped_source_frame, plan_error, validation::validate_ass_resource,
};
use crate::{Error, Result};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str,
    sync::atomic::AtomicBool,
    thread,
    time::Duration,
};
use termtrace_evidence::artifact::{secure_file, write_secure};
use termtrace_model::artifact::presentation::{
    AnnotationAnchor, AnnotationPosition, CaptionLayout, ChangeMode, ChangeRegion, ChangeSource,
    ChangeVisualization, CursorMaskFrame, NormalizedAnnotation, PixelTransform, PresentationPlan,
    PreviewOptions,
};
use termtrace_render::semantic_font_set;

pub(super) fn render_contact_sheet(
    source: &Path,
    staging: &Path,
    selected_frames: &[u64],
    options: PreviewOptions,
) -> Result<()> {
    let selection = selected_frames
        .iter()
        .map(|frame| return format!("eq(n\\,{frame})"))
        .collect::<Vec<_>>()
        .join("+");
    let rows = selected_frames.len().div_ceil(usize::from(options.columns));
    let filter = format!(
        "select={selection},scale={}:-2:flags=lanczos,tile={}x{}:nb_frames={}:padding=4:margin=4:color=black",
        options.tile_width,
        options.columns,
        rows,
        selected_frames.len(),
    );

    let process = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-y", "-i"])
        .arg(source)
        .args(["-vf", &filter, "-frames:v", "1", "contact-sheet.png"])
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

    write_secure(&staging.join("contact-sheet.log"), &process.stderr)?;

    if !process.status.success() {
        return Err(plan_error(format!(
            "FFmpeg contact-sheet rendering failed with {}",
            process.status
        )));
    }
    return Ok(());
}

pub(super) fn render_low_resolution_preview(
    source: &Path,
    staging: &Path,
    width: u16,
) -> Result<()> {
    let filter = format!("scale=w='min(iw,{width})':h=-2:flags=lanczos");
    let process = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-y", "-threads", "1", "-i"])
        .arg(source)
        .args([
            "-threads",
            "1",
            "-vf",
            &filter,
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "28",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "preview.mp4",
        ])
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

    write_secure(&staging.join("preview.log"), &process.stderr)?;

    if !process.status.success() {
        return Err(plan_error(format!(
            "FFmpeg low-resolution preview failed with {}",
            process.status
        )));
    }
    return Ok(());
}

#[derive(Clone, Copy)]
struct ScheduledBox {
    region: ChangeRegion,
    start_frame: u64,
    end_frame: u64,
}

pub(super) fn append_change_visualization(
    filters: &mut Vec<String>,
    plan: &PresentationPlan,
    input: &str,
) -> Result<String> {
    let Some(change) = &plan.change_visualization else {
        return Ok(input.to_owned());
    };

    return match change.source {
        ChangeSource::SemanticCells => {
            append_semantic_change_visualization(filters, plan, change, input)
        }
        ChangeSource::EncodedPixels => append_encoded_change_visualization(filters, change, input),
    };
}

fn append_semantic_change_visualization(
    filters: &mut Vec<String>,
    plan: &PresentationPlan,
    change: &ChangeVisualization,
    input: &str,
) -> Result<String> {
    let boxes = scheduled_semantic_boxes(plan, change);
    if boxes.len() > 100_000 {
        return Err(plan_error(
            "semantic change visualization exceeds 100000 scheduled regions",
        ));
    }

    let (base, mut prior) = if change.mode == ChangeMode::SideDiff {
        filters.push(format!("[{input}]split=2[changebase][changeinput]"));
        (Some("changebase"), "changeinput".to_owned())
    } else {
        (None, input.to_owned())
    };

    for (index, scheduled_box) in boxes.iter().enumerate() {
        let next = format!("change{index}");
        let (color, thickness) = match change.mode {
            ChangeMode::DirtyRows => ("orange@0.28", "fill"),
            ChangeMode::Heatmap => ("red@0.42", "fill"),
            ChangeMode::Trail => ("magenta@0.30", "fill"),
            ChangeMode::BoundingBox => ("cyan@0.90", "3"),
            ChangeMode::Outline | ChangeMode::SideDiff => ("yellow@0.90", "2"),
        };
        filters.push(format!(
            "[{prior}]drawbox=x={}:y={}:w={}:h={}:color={color}:t={thickness}:enable='between(n,{},{})'[{next}]",
            scheduled_box.region.x,
            scheduled_box.region.y,
            scheduled_box.region.width,
            scheduled_box.region.height,
            scheduled_box.start_frame,
            scheduled_box.end_frame,
        ));

        prior = next;
    }

    if let Some(base) = base {
        filters.push(format!("[{base}][{prior}]hstack=inputs=2[changeoutput]"));

        return Ok("changeoutput".to_owned());
    }
    return Ok(prior);
}

fn scheduled_semantic_boxes(
    plan: &PresentationPlan,
    change: &ChangeVisualization,
) -> Vec<ScheduledBox> {
    let mut boxes = Vec::new();
    let geometry = plan.source_geometry;
    let x_origin = plan.source_width.saturating_sub(geometry.text_width) / 2;
    let y_origin = plan.source_height.saturating_sub(geometry.text_height) / 2;

    for frame in &plan.change_frames {
        let regions = match change.mode {
            ChangeMode::DirtyRows => frame
                .dirty_rows
                .iter()
                .map(|row| {
                    return ChangeRegion {
                        x: x_origin,
                        y: y_origin.saturating_add(row.saturating_mul(geometry.cell_height)),
                        width: geometry.text_width,
                        height: geometry.cell_height,
                    };
                })
                .collect(),
            ChangeMode::BoundingBox => bounding_region(&frame.changed_regions)
                .into_iter()
                .collect(),
            _ => frame.changed_regions.clone(),
        };

        for output_frame in &frame.output_frames {
            for region in &regions {
                boxes.push(ScheduledBox {
                    region: *region,
                    start_frame: *output_frame,
                    end_frame: output_frame
                        .saturating_add(u64::from(change.hold_frames))
                        .min(plan.output_frame_count.saturating_sub(1)),
                });
            }
        }
    }

    return boxes;
}

fn bounding_region(regions: &[ChangeRegion]) -> Option<ChangeRegion> {
    let first = regions.first()?;
    let mut left = first.x;
    let mut top = first.y;

    let mut right = first.x.saturating_add(first.width);
    let mut bottom = first.y.saturating_add(first.height);

    for region in regions.iter().skip(1) {
        left = left.min(region.x);
        top = top.min(region.y);
        right = right.max(region.x.saturating_add(region.width));
        bottom = bottom.max(region.y.saturating_add(region.height));
    }

    return Some(ChangeRegion {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    });
}

fn append_encoded_change_visualization(
    filters: &mut Vec<String>,
    change: &ChangeVisualization,
    input: &str,
) -> Result<String> {
    let threshold = change.pixel_threshold.unwrap_or(16);
    filters.push(format!("[{input}]split=2[encodedbase][encodedinput]"));

    let mut diff_filter = format!(
        "[encodedinput]tblend=all_mode=difference,lutrgb=r='if(gte(val,{threshold}),val,0)':g='if(gte(val,{threshold}),val,0)':b='if(gte(val,{threshold}),val,0)'"
    );
    match change.mode {
        ChangeMode::Outline => diff_filter.push_str(",edgedetect=mode=colormix:high=0.4"),
        ChangeMode::Trail if change.hold_frames > 0 => {
            write!(
                &mut diff_filter,
                ",tmix=frames={}",
                u64::from(change.hold_frames) + 1,
            )
            .map_err(|_error| return plan_error("could not build encoded trail filter"))?;
        }
        ChangeMode::Heatmap | ChangeMode::Trail | ChangeMode::SideDiff => {}
        ChangeMode::DirtyRows | ChangeMode::BoundingBox => {
            return Err(plan_error("unsupported encoded-pixel change mode"));
        }
    }

    diff_filter.push_str("[encodeddiff]");
    filters.push(diff_filter);

    if change.mode == ChangeMode::SideDiff {
        filters.push("[encodedbase][encodeddiff]hstack=inputs=2[changeoutput]".to_owned());
    } else {
        filters.push(
            "[encodedbase][encodeddiff]blend=all_mode=screen:all_opacity=0.65[changeoutput]"
                .to_owned(),
        );
    }

    return Ok("changeoutput".to_owned());
}

pub(super) fn append_pixel_transforms(
    filters: &mut Vec<String>,
    plan: &PresentationPlan,
    staging: &Path,
    input: &str,
) -> Result<String> {
    let mut prior = input.to_owned();
    let mut width = presentation_canvas_width_before_transforms(plan);
    let mut height =
        u32::from(plan.source_height) + if presentation_has_footer(plan) { 48 } else { 0 };

    let mut sequence = 0_usize;
    for transform in &plan.transforms {
        match transform {
            PixelTransform::CursorMask => {
                for (region, start_frame, end_frame) in cursor_mask_ranges(&plan.cursor_masks) {
                    let next = format!("transform{sequence}");
                    filters.push(format!(
                        "[{prior}]drawbox=x={}:y={}:w={}:h={}:color=black@1:t=fill:enable='between(n,{start_frame},{end_frame})'[{next}]",
                        region.x,
                        region.y,
                        region.width,
                        region.height,
                    ));

                    prior = next;
                    sequence += 1;

                    if plan.change_visualization.as_ref().is_some_and(|change| {
                        return change.mode == ChangeMode::SideDiff;
                    }) {
                        let next = format!("transform{sequence}");
                        filters.push(format!(
                            "[{prior}]drawbox=x={}:y={}:w={}:h={}:color=black@1:t=fill:enable='between(n,{start_frame},{end_frame})'[{next}]",
                            u32::from(region.x) + u32::from(plan.source_width),
                            region.y,
                            region.width,
                            region.height,
                        ));

                        prior = next;
                        sequence += 1;
                    }
                }
            }
            PixelTransform::Crop {
                x,
                y,
                width: crop_width,
                height: crop_height,
            } => {
                let next = format!("transform{sequence}");
                filters.push(format!(
                    "[{prior}]crop={crop_width}:{crop_height}:{x}:{y}[{next}]"
                ));

                prior = next;
                width = u32::from(*crop_width);
                height = u32::from(*crop_height);
                sequence += 1;
            }
            PixelTransform::Zoom {
                numerator,
                denominator,
            } => {
                width = scaled_even_u32(width, *numerator, *denominator);
                height = scaled_even_u32(height, *numerator, *denominator);

                let next = format!("transform{sequence}");
                filters.push(format!(
                    "[{prior}]scale={width}:{height}:flags=lanczos[{next}]"
                ));

                prior = next;
                sequence += 1;
            }
            PixelTransform::Resize {
                width: resize_width,
                height: resize_height,
            } => {
                width = u32::from(*resize_width);
                height = u32::from(*resize_height);

                let next = format!("transform{sequence}");
                filters.push(format!(
                    "[{prior}]scale={width}:{height}:flags=lanczos[{next}]"
                ));

                prior = next;
                sequence += 1;
            }
            PixelTransform::Callout {
                x,
                y,
                width: callout_width,
                height: callout_height,
                label,
            } => {
                let next = format!("transform{sequence}");
                filters.push(format!(
                    "[{prior}]drawbox=x={x}:y={y}:w={callout_width}:h={callout_height}:color=cyan@0.95:t=3[{next}]"
                ));

                prior = next;
                sequence += 1;

                if let Some(label) = label {
                    ensure_presentation_font(staging)?;

                    let name = format!("callout-{sequence}.txt");
                    write_secure(&staging.join(&name), label.as_bytes())?;

                    let next = format!("transform{sequence}");
                    filters.push(format!(
                        "[{prior}]drawtext=fontfile='fonts/JetBrainsMono-Regular.ttf':textfile='{}':fontcolor=cyan:fontsize=18:box=1:boxcolor=black@0.7:x={}:y={}[{next}]",
                        filter_path_value(Path::new(&name))?,
                        x.saturating_add(4),
                        y.saturating_sub(24),
                    ));

                    prior = next;
                    sequence += 1;
                }
            }
        }
    }

    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        filters.push(format!(
            "[{prior}]pad={}:{}:0:0:color=black[evened]",
            width.next_multiple_of(2),
            height.next_multiple_of(2)
        ));

        "evened".clone_into(&mut prior);
    }

    if let Some(branding) = &plan.layout.branding {
        ensure_presentation_font(staging)?;
        write_secure(&staging.join("branding.txt"), branding.as_bytes())?;
        filters.push(format!(
            "[{prior}]drawtext=fontfile='fonts/JetBrainsMono-Regular.ttf':textfile='branding.txt':fontcolor=white@0.8:fontsize=16:box=1:boxcolor=black@0.55:x=w-tw-12:y=12[branded]"
        ));

        "branded".clone_into(&mut prior);
    }

    return Ok(prior);
}

fn presentation_canvas_width_before_transforms(plan: &PresentationPlan) -> u32 {
    if plan.change_visualization.as_ref().is_some_and(|change| {
        return change.mode == ChangeMode::SideDiff;
    }) {
        return u32::from(plan.source_width) * 2;
    }
    return u32::from(plan.source_width);
}

fn scaled_even_u32(value: u32, numerator: u16, denominator: u16) -> u32 {
    return value.saturating_mul(u32::from(numerator)) / u32::from(denominator) / 2 * 2;
}

pub(super) fn cursor_mask_ranges(masks: &[CursorMaskFrame]) -> Vec<(ChangeRegion, u64, u64)> {
    let mut ranges: Vec<(ChangeRegion, u64, u64)> = Vec::new();
    for mask in masks {
        match ranges.last_mut() {
            Some((region, _, end))
                if *region == mask.region && end.saturating_add(1) == mask.output_frame =>
            {
                *end = mask.output_frame;
            }
            _ => ranges.push((mask.region, mask.output_frame, mask.output_frame)),
        }
    }

    return ranges;
}

fn ensure_presentation_font(staging: &Path) -> Result<()> {
    let fonts = staging.join("fonts");
    fs_err::create_dir_all(&fonts).map_err(|source| {
        return Error::Filesystem {
            path: fonts.clone(),
            source,
        };
    })?;

    let font = fonts.join("JetBrainsMono-Regular.ttf");
    let font_bytes = semantic_font_set()
        .map_err(|error| return plan_error(format!("load presentation font: {error}")))?
        .regular_face_bytes();

    if font.exists() {
        let bytes = fs_err::read(&font).map_err(|source| {
            return Error::Filesystem {
                path: font.clone(),
                source,
            };
        })?;

        if bytes != font_bytes {
            return Err(plan_error("staged presentation font changed unexpectedly"));
        }
    } else {
        write_secure(&font, font_bytes)?;
    }

    return Ok(());
}

pub(super) fn prepare_subtitle_resources(
    plan: &PresentationPlan,
    staging: &Path,
) -> Result<Vec<PathBuf>> {
    if plan.annotations.is_empty() && plan.subtitles.is_empty() {
        return Ok(Vec::new());
    }

    require_ass_filter()?;

    ensure_presentation_font(staging)?;

    let mut resources = Vec::new();

    if !plan.annotations.is_empty() {
        let generated = staging.join("annotations.ass");
        write_secure(&generated, generated_ass(plan)?.as_bytes())?;
        resources.push(generated);
    }

    for (index, subtitle) in plan.subtitles.iter().enumerate() {
        validate_ass_resource(&subtitle.source_path)?;

        let destination = staging.join(format!("subtitle-{index}.ass"));
        let bytes = fs_err::read(&subtitle.source_path).map_err(|source| {
            return Error::Filesystem {
                path: subtitle.source_path.clone(),
                source,
            };
        })?;

        if blake3::hash(&bytes).to_hex().as_str() != subtitle.resource_hash {
            return Err(plan_error(format!(
                "ASS subtitle resource {} changed after planning",
                subtitle.source_path.display()
            )));
        }

        write_secure(&destination, &bytes)?;
        resources.push(destination);
    }

    return Ok(resources);
}

fn require_ass_filter() -> Result<()> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-filters"])
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;

    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| return line.split_whitespace().any(|field| return field == "ass"))
    {
        return Err(plan_error(
            "FFmpeg ASS/libass filter is required for annotations or subtitle tracks",
        ));
    }
    return Ok(());
}

fn generated_ass(plan: &PresentationPlan) -> Result<String> {
    let canvas_height = if plan.output_height > 0 {
        u64::from(plan.output_height)
    } else {
        u64::from(plan.source_height) + if presentation_has_footer(plan) { 48 } else { 0 }
    };

    let mut ass = format!(
        "[Script Info]\nScriptType: v4.00+\nWrapStyle: 2\nScaledBorderAndShadow: yes\nPlayResX: {}\nPlayResY: {}\nYCbCr Matrix: None\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
        presentation_canvas_width(plan),
        canvas_height
    );

    for (index, annotation) in plan.annotations.iter().enumerate() {
        if plan
            .annotations
            .iter()
            .take(index)
            .any(|prior| return prior.annotation_index == annotation.annotation_index)
        {
            continue;
        }

        let foreground = ass_color(&annotation.style.foreground, 255)?;
        let background = ass_color(
            &annotation.style.background,
            annotation.style.background_opacity,
        )?;
        writeln!(
            &mut ass,
            "Style: A{},Fira Code,{},{foreground},&H00000000,{background},{background},0,0,0,0,100,100,0,0,3,{},0,7,0,0,0,1",
            annotation.annotation_index, annotation.style.font_size, annotation.style.border,
        )
        .map_err(|_error| return plan_error("could not generate ASS style"))?;
    }

    ass.push_str("\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");

    let dynamic_event_count: u64 = plan
        .annotations
        .iter()
        .filter(|annotation| return annotation.template.is_some())
        .map(|annotation| return annotation.output_end_frame - annotation.output_start_frame)
        .sum();

    if dynamic_event_count > 1_000_000 {
        return Err(plan_error(
            "dynamic annotation expansion exceeds one million ASS events",
        ));
    }

    for annotation in &plan.annotations {
        if let Some(text) = &annotation.text {
            write_ass_dialogue(
                &mut ass,
                plan,
                annotation,
                annotation.output_start_frame,
                annotation.output_end_frame,
                text,
            )?;
            continue;
        }

        for output_frame in annotation.output_start_frame..annotation.output_end_frame {
            let template = annotation.template.as_deref().unwrap_or_default();
            let text = expand_annotation_template(plan, output_frame, template)?;
            write_ass_dialogue(
                &mut ass,
                plan,
                annotation,
                output_frame,
                output_frame + 1,
                &text,
            )?;
        }
    }

    return Ok(ass);
}

fn write_ass_dialogue(
    ass: &mut String,
    plan: &PresentationPlan,
    annotation: &NormalizedAnnotation,
    start_frame: u64,
    end_frame: u64,
    text: &str,
) -> Result<()> {
    let (alignment, x, y) = annotation_position(
        &annotation.position,
        i64::from(presentation_canvas_width(plan)),
        i64::from(plan.source_height) + if presentation_has_footer(plan) { 48 } else { 0 },
    );
    writeln!(
        ass,
        "Dialogue: {},{},{},A{},,0,0,0,,{{\\an{}\\pos({x},{y})}}{}",
        i32::from(annotation.z_order) + 32_768,
        ass_time(start_frame, plan.fps, false),
        ass_time(end_frame, plan.fps, true),
        annotation.annotation_index,
        alignment,
        escape_ass_text(text),
    )
    .map_err(|_error| return plan_error("could not generate ASS dialogue"))?;

    return Ok(());
}

fn presentation_has_footer(plan: &PresentationPlan) -> bool {
    return plan.layout.caption == CaptionLayout::Footer
        && plan.clips.iter().any(|clip| return clip.caption.is_some());
}

fn presentation_canvas_width(plan: &PresentationPlan) -> u16 {
    if plan.output_width > 0 {
        return plan.output_width;
    }

    if plan.change_visualization.as_ref().is_some_and(|change| {
        return change.mode == ChangeMode::SideDiff;
    }) {
        return plan.source_width.saturating_mul(2);
    }
    return plan.source_width;
}

fn annotation_position(position: &AnnotationPosition, width: i64, height: i64) -> (u8, i64, i64) {
    let x = i64::from(position.x);
    let y = i64::from(position.y);

    return match position.anchor {
        AnnotationAnchor::TopLeft => (7, x, y),
        AnnotationAnchor::TopRight => (9, width + x, y),
        AnnotationAnchor::BottomLeft => (1, x, height + y),
        AnnotationAnchor::BottomRight => (3, width + x, height + y),
        AnnotationAnchor::Center => (5, width / 2 + x, height / 2 + y),
    };
}

fn ass_color(rgb: &str, opacity: u8) -> Result<String> {
    let value = u32::from_str_radix(rgb.trim_start_matches('#'), 16)
        .map_err(|_error| return plan_error("annotation color is invalid"))?;
    let red = value >> 16;
    let green = value >> 8 & 0xff;
    let blue = value & 0xff;

    return Ok(format!(
        "&H{:02X}{blue:02X}{green:02X}{red:02X}",
        255 - opacity
    ));
}

fn ass_time(frame: u64, fps: u16, end: bool) -> String {
    let numerator = frame.saturating_mul(100);
    let centiseconds =
        if end { numerator.div_ceil(u64::from(fps)) } else { numerator / u64::from(fps) };

    return format!(
        "{}:{:02}:{:02}.{:02}",
        centiseconds / 360_000,
        centiseconds / 6_000 % 60,
        centiseconds / 100 % 60,
        centiseconds % 100,
    );
}

fn escape_ass_text(text: &str) -> String {
    return text
        .replace('\\', "\\\\")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace("\r\n", "\\N")
        .replace(['\r', '\n'], "\\N");
}

fn expand_annotation_template(
    plan: &PresentationPlan,
    output_frame: u64,
    template: &str,
) -> Result<String> {
    let clip = plan
        .clips
        .iter()
        .find(|clip| {
            return clip.output_start_frame <= output_frame && output_frame < clip.output_end_frame;
        })
        .ok_or_else(|| return plan_error("annotation frame has no source mapping"))?;

    let (source_frame, _) = mapped_source_frame(clip, output_frame);
    let marker = plan
        .markers
        .iter()
        .filter(|marker| return marker.nominal_frame == source_frame)
        .map(|marker| return marker.label.as_str())
        .collect::<Vec<_>>()
        .join(",");

    let source_millis = source_frame.saturating_mul(1000) / u64::from(plan.fps);
    let source_time = format!("{}.{:03}s", source_millis / 1000, source_millis % 1000);
    let output_millis = output_frame.saturating_mul(1000) / u64::from(plan.fps);
    let timecode = format!(
        "{:02}:{:02}:{:02}.{:03}",
        output_millis / 3_600_000,
        output_millis / 60_000 % 60,
        output_millis / 1000 % 60,
        output_millis % 1000,
    );

    return Ok(template
        .replace("{output_frame}", &output_frame.to_string())
        .replace("{source_frame}", &source_frame.to_string())
        .replace("{source_time}", &source_time)
        .replace("{marker}", &marker)
        .replace("{timecode}", &timecode));
}

pub(super) fn command_version(command: &str) -> Result<String> {
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

pub(super) fn probe_delivery_video(staging: &Path) -> Result<serde_json::Value> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,pix_fmt,width,height,r_frame_rate,nb_frames,color_range,color_space,color_transfer,color_primaries:format=format_name,duration",
            "-of",
            "json",
            "presentation.mp4",
        ])
        .current_dir(staging)
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffprobe"),
                source,
            };
        })?;

    if !output.status.success() {
        write_secure(&staging.join("ffprobe.log"), &output.stderr)?;

        return Err(plan_error("FFprobe rejected the presentation output"));
    }

    let value = serde_json::from_slice(&output.stdout).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("output-probe.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return Ok(value);
}

pub(super) fn write_source_frame_hashes(source_video: &Path, staging: &Path) -> Result<()> {
    let hashes_path = staging.join("source-frame-hashes.framemd5");
    let log_path = staging.join("source-frame-hashes.log");
    let hashes = secure_file(&hashes_path)?;
    let log = secure_file(&log_path)?;
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(source_video)
        .args(["-map", "0:v:0", "-f", "framemd5", "-"])
        .stdout(Stdio::from(hashes))
        .stderr(Stdio::from(log))
        .status()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;

    if !status.success() {
        return Err(plan_error(format!(
            "FFmpeg source-frame hashing failed with {status}"
        )));
    }

    let metadata = fs_err::metadata(&hashes_path).map_err(|source| {
        return Error::Filesystem {
            path: hashes_path,
            source,
        };
    })?;

    if metadata.len() == 0 {
        return Err(plan_error("FFmpeg source-frame hash output was empty"));
    }
    return Ok(());
}

/// Run a file-backed command under process-group ownership; no unbounded pipe buffering.
pub(super) fn run_logged_command(
    program: &str,
    arguments: &[String],
    directory: &Path,
    stem: &str,
    cancelled: Option<&AtomicBool>,
    timeout: Duration,
) -> Result<()> {
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    use termtrace_process::SupervisedChild;
    let stdout = secure_file(&directory.join(format!("{stem}.stdout")))?;
    let stderr = secure_file(&directory.join(format!("{stem}.log")))?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child = SupervisedChild::spawn(&mut command).map_err(|source| {
        return Error::Filesystem {
            path: program.into(),
            source,
        };
    })?;

    let deadline = Instant::now() + timeout;
    let result = loop {
        if cancelled.is_some_and(|flag| return flag.load(Ordering::Acquire)) {
            break Err(plan_error(format!(
                "{program} {stem} cancelled; no output publication"
            )));
        }

        if Instant::now() >= deadline {
            break Err(plan_error(format!("{program} {stem} timed out")));
        }

        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => {
                break Err(plan_error(format!(
                    "{program} {stem} failed with {status}; see {stem}.log"
                )))
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(source) => {
                break Err(Error::Filesystem {
                    path: program.into(),
                    source,
                })
            }
        }
    };
    // Terminate any descendants even when the leader exited successfully.
    child.terminate(Duration::from_millis(250));

    return result;
}

/// Probe an explicit file and actually decode all frames. GIF delay clamping is disabled.
pub(super) fn probe_video_file(
    directory: &Path,
    filename: &str,
    gif: bool,
) -> Result<serde_json::Value> {
    let mut args: Vec<String> = ["-v", "error"].map(str::to_owned).to_vec();

    if gif {
        args.extend(["-min_delay", "0"].map(str::to_owned));
    }

    args.extend(["-i", filename, "-select_streams", "v:0", "-count_frames", "-show_entries",
        "stream=codec_name,pix_fmt,width,height,r_frame_rate,avg_frame_rate,time_base,nb_read_frames,color_range,color_space,color_transfer,color_primaries:format=format_name,duration", "-of", "json"].map(str::to_owned));

    let stem = format!("{filename}-probe");
    run_logged_command(
        "ffprobe",
        &args,
        directory,
        &stem,
        None,
        Duration::from_secs(3600),
    )?;

    let path = directory.join(format!("{stem}.stdout"));
    let bytes = fs_err::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;

    return serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path,
            json_path: "$".into(),
            source,
        };
    });
}
