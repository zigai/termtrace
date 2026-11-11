//! Edit normalization, presentation videos, and frame mapping.

use crate::errors::to_py_err;
use crate::host::{from_python, Model, PathArg, TYPES_MODULE};
use pyo3::prelude::*;
use pyo3_stub_gen::derive::gen_stub_pyfunction;
use pyo3_stub_gen::{PyStubType, TypeInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use termtrace_sdk::artifacts::{
    output_frames_for_source_frame as output_frames, source_for_output_frame as output_source,
    ArtifactService, EditSource, NormalizedEdit, OutputFrameSource, PresentationPlan, VideoMode,
    VideoOptions, VideoOutcome,
};
use termtrace_sdk::system::SystemService;

/// A `termtrace.Edit` (anything with `to_source()`), an `EditSource`, or the path of a JSON
/// edit file.
pub struct EditArg(pub EditSource);

impl<'a, 'py> FromPyObject<'a, 'py> for EditArg {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        if let Ok(path) = obj.extract::<PathArg>() {
            return Ok(Self(EditSource::File { path: path.0 }));
        }

        if obj.hasattr("to_source")? {
            return from_python(&obj.call_method0("to_source")?).map(Self);
        }
        return from_python(&obj).map(Self);
    }
}

impl PyStubType for EditArg {
    fn type_output() -> TypeInfo {
        let mut info =
            TypeInfo::unqualified("editing.Edit | types.EditSourceInput | str | os.PathLike[str]");
        info.import.insert("os".into());
        info.import.insert("termtrace.editing".into());
        info.import.insert("termtrace.types".into());
        return info;
    }
}

/// A `PresentationPlan` as returned by `video_plan`.
pub struct PlanArg(pub PresentationPlan);

impl<'a, 'py> FromPyObject<'a, 'py> for PlanArg {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        return from_python(&obj).map(Self);
    }
}

impl PyStubType for PlanArg {
    fn type_output() -> TypeInfo {
        return TypeInfo::locally_defined("PresentationPlan", TYPES_MODULE.into());
    }
}

pub(crate) fn video(
    binary: &Path,
    session: PathBuf,
    edit: EditSource,
    mode: VideoMode,
    options: &VideoOptions,
) -> PyResult<VideoOutcome> {
    let request = options.request(session, edit, mode).map_err(to_py_err)?;
    let cancelled = AtomicBool::new(false);

    return ArtifactService::video(
        &request,
        binary,
        &mut |id| return SystemService::require(id, &cancelled),
        &mut |_| {},
    )
    .map_err(to_py_err);
}

/// Resolve an edit's subtitle paths, validate it, and return it with its `edit_plan_hash`.
///
/// # Errors
///
/// Raises `TermtraceError` whose `code` names the edit failure (for example `invalid_speed` or
/// `relative_resource_without_base`) and whose `details` hold `field`, `clip_index`, and `marker`.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn normalize_edit(py: Python<'_>, edit: EditArg) -> PyResult<Model<NormalizedEdit>> {
    return py.detach(move || {
        return ArtifactService::normalize_edit(&edit.0)
            .map(Model)
            .map_err(to_py_err);
    });
}

/// Clip, source frame, and operation (`source` or `hold`) shown on `output_frame`, or `None`
/// past the end.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn source_for_output_frame(
    plan: PlanArg,
    output_frame: u64,
) -> Option<Model<OutputFrameSource>> {
    return output_source(&plan.0, output_frame).map(Model);
}

/// Half-open `(start, end)` output-frame spans that show `source_frame`, in output order.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn output_frames_for_source_frame(plan: PlanArg, source_frame: u64) -> Vec<(u64, u64)> {
    return output_frames(&plan.0, source_frame)
        .into_iter()
        .map(|span| return (span.start, span.end))
        .collect();
}

/// Register the editing functions.
///
/// # Errors
///
/// Fails when a function cannot be added to the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(normalize_edit, m)?)?;
    m.add_function(wrap_pyfunction!(source_for_output_frame, m)?)?;
    m.add_function(wrap_pyfunction!(output_frames_for_source_frame, m)?)?;

    return Ok(());
}
