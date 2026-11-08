//! Main Termtrace client and facade for Python.

use crate::control;
use crate::errors::to_py_err;
use crate::host::{Arg, Model, PathArg};
use crate::recording::PyRecording;
use crate::scenario::scenario_err;
use crate::session::PySession;
use crate::video::{video, EditArg};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use termtrace_model::artifact::bundle::{BundleProfile, BundleVerificationPolicy};
use termtrace_model::execution::event_log::RecoveryReport;
use termtrace_model::execution::session::{VerificationLevel, VerificationReport};
use termtrace_model::protocol::capability::CapabilityReport;
use termtrace_model::registry::{GarbageCollectionReport, RegistryEntry, RegistryListEntry};
use termtrace_model::scenario::ScenarioSuiteReport;
use termtrace_sdk::artifacts::{
    ArtifactService, GifLoop, PresentationPlan, PreviewOptions, SemanticRasterProfile, VideoMode,
    VideoOptions, VideoOutcome, VideoPreview, VideoRender, VideoRenderer,
};
use termtrace_sdk::client::Terminal;
use termtrace_sdk::contracts::LaunchOptions;
use termtrace_sdk::defaults;
use termtrace_sdk::integrity::IntegrityService;
use termtrace_sdk::locator::WaitCancellation;
use termtrace_sdk::registry::RegistryService;
use termtrace_sdk::scenario::{run_suite, ScenarioSuiteOptions};
use termtrace_sdk::session::{LaunchSpec, SessionTarget, VisualProfile, VisualProfileReport};
use termtrace_sdk::system::SystemService;
use termtrace_sdk::tmux::TmuxService;
use termtrace_sdk::tmux::TmuxStreamingBundleManifest;
use termtrace_sdk::{ByteString, EnvironmentSpec};

fn default_evidence_root() -> PathBuf {
    if let Ok(root) = std::env::var("TERMTRACE_ROOT") {
        return PathBuf::from(root);
    }
    return std::env::temp_dir().join("termtrace");
}

/// Primary entry point for launching and managing Termtrace recordings in Python.
#[gen_stub_pyclass]
#[pyclass(name = "Termtrace", from_py_object)]
#[derive(Clone)]
pub struct PyTermtrace {
    pub(crate) terminal: Terminal,
}

impl PyTermtrace {
    fn start(&self, py: Python<'_>, spec: LaunchSpec) -> PyResult<PyRecording> {
        let terminal = self.terminal.clone();

        py.detach(move || {
            let session = terminal.start(spec).map_err(to_py_err)?;
            let session_id = session.session_id().to_string();
            let path = session.path().display().to_string();
            let lifecycle = Arc::new(session.lifecycle_observer());

            Ok(PyRecording {
                lifecycle,
                finalization: Arc::new(Mutex::new(None)),
                terminal,
                session: Arc::new(Mutex::new(Some(session))),
                session_id,
                path,
            })
        })
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyTermtrace {
    /// Create a new Termtrace client instance with optional binary and evidence root overrides.
    #[new]
    #[pyo3(signature = (binary = None, evidence_root = None))]
    pub fn new(binary: Option<String>, evidence_root: Option<String>) -> Self {
        let bin_path = binary.map_or_else(defaults::binary, PathBuf::from);
        let root_path = evidence_root.map_or_else(default_evidence_root, PathBuf::from);
        Self {
            terminal: Terminal::new(bin_path, root_path),
        }
    }

    /// Local typed system service, without starting any probe yet.
    pub fn system(&self) -> crate::diagnostics::PySystem {
        return crate::diagnostics::PySystem::new();
    }
    /// Instrumentation bound to one live session.
    pub fn instrumentation(&self, source: PathArg) -> crate::diagnostics::PyInstrumentation {
        return crate::diagnostics::PyInstrumentation::new(source);
    }

    /// Aggregate tmux recording with an explicit root, tmux executable, and socket.
    #[pyo3(signature = (root = None, tmux = None, socket = None, socket_name = false))]
    pub fn tmux(
        &self,
        root: Option<PathArg>,
        tmux: Option<PathArg>,
        socket: Option<PathArg>,
        socket_name: bool,
    ) -> crate::diagnostics::PyTmux {
        return crate::diagnostics::PyTmux::new(
            root,
            Some(PathArg(self.terminal.binary().to_path_buf())),
            tmux,
            socket,
            socket_name,
        );
    }

    /// Registry operations at an explicit or the default state root.
    #[pyo3(signature = (root = None))]
    pub fn registry(&self, root: Option<PathArg>) -> crate::diagnostics::PyRegistry {
        return crate::diagnostics::PyRegistry::new(
            root,
            Some(PathArg(self.terminal.binary().to_path_buf())),
        );
    }

    /// Replay analysis of one canonical session.
    pub fn analysis(&self, source: PathArg) -> crate::diagnostics::PyDiagnostics {
        return crate::diagnostics::PyDiagnostics::new(source);
    }

    /// Evidence artifacts and integrity operations for one canonical session.
    pub fn evidence(&self, source: PathArg) -> crate::diagnostics::PyEvidence {
        return crate::diagnostics::PyEvidence::new(source);
    }

    /// Native replay operations for one canonical session.
    pub fn native(&self, source: PathArg) -> crate::native::PyNative {
        return crate::native::PyNative {
            terminal: self.terminal.clone(),
            path: source.0,
        };
    }

    /// Run scenario documents as one suite with this client's binary. A document with a
    /// `matrix` expands to one labelled case per combination; with `shard_index` and
    /// `shard_count`, case `i` of the sorted expansion runs only when
    /// `i % shard_count == shard_index`. Up to `jobs` cases run at once, each in a fresh session
    /// under `evidence_root` (default: this client's evidence root). Failure artifacts go under
    /// `failure_root` and snapshot baselines in its sibling `snapshots` directory, replaced when
    /// `update_snapshots` is set. A failed case runs again up to `reruns` times; a later pass
    /// makes it `flaky`, which fails CI only with `fail_on_flaky`.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` when the options are invalid, a document cannot be read or
    /// expanded, or the shard is empty. Case failures stay in the report.
    #[pyo3(signature = (scenarios, *, failure_root, evidence_root = None, jobs = 1, shard_index = None, shard_count = None, update_snapshots = false, reruns = 0, fail_on_flaky = false))]
    #[expect(clippy::too_many_arguments, reason = "Python keyword arguments")]
    pub fn run_scenarios(
        &self,
        py: Python<'_>,
        scenarios: Vec<PathArg>,
        failure_root: PathArg,
        evidence_root: Option<PathArg>,
        jobs: usize,
        shard_index: Option<usize>,
        shard_count: Option<usize>,
        update_snapshots: bool,
        reruns: u8,
        fail_on_flaky: bool,
    ) -> PyResult<Model<ScenarioSuiteReport>> {
        let options = ScenarioSuiteOptions {
            binary: self.terminal.binary().to_path_buf(),
            evidence_root: evidence_root.map_or_else(
                || return self.terminal.evidence_root().to_path_buf(),
                |root| return root.0,
            ),
            failure_root: failure_root.0,
            jobs,
            shard_index,
            shard_count,
            update_snapshots,
            reruns,
            fail_on_flaky,
        };

        return py.detach(move || {
            return run_suite(scenarios.into_iter().map(|path| return path.0), &options)
                .map(Model)
                .map_err(scenario_err);
        });
    }

    /// Launch a new recorded command session. `visual_profile` is committed into the session;
    /// its native section, when present, is resolved before the command starts.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for a malformed profile, and `TermtraceError` or a subclass when the
    /// profile is invalid or does not resolve, or the command cannot be launched and recorded.
    #[pyo3(signature = (command, rows = None, cols = None, env = None, cwd = None, visual_profile = None))]
    pub fn record(
        &self,
        py: Python<'_>,
        command: Vec<String>,
        rows: Option<u16>,
        cols: Option<u16>,
        env: Option<HashMap<String, String>>,
        cwd: Option<String>,
        visual_profile: Option<Arg<VisualProfile>>,
    ) -> PyResult<PyRecording> {
        let environment = EnvironmentSpec {
            set: env
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| return (name, ByteString::new(value.into_bytes())))
                .collect(),
            ..EnvironmentSpec::default()
        };
        let options = LaunchOptions {
            rows,
            cols,
            cwd,
            environment: Some(environment),
            visual_profile: visual_profile.map(|profile| return profile.0),
            ..LaunchOptions::default()
        };

        return self.start(py, control::launch_spec_from(command, &options)?);
    }

    /// Validate a visual profile and report the identity of everything a launch with it would
    /// use. A semantic-only profile runs no native program; a native section is resolved by
    /// Ghostty and the `cols` by `rows` grid is solved at the profile's DPI. Advisory only.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for a malformed profile, and `TermtraceError` or a subclass with the
    /// visual-profile failure code when it is invalid or does not resolve.
    #[pyo3(signature = (profile, cols = None, rows = None))]
    pub fn preflight_visual_profile(
        &self,
        py: Python<'_>,
        profile: Arg<VisualProfile>,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> PyResult<Model<VisualProfileReport>> {
        let terminal = self.terminal.clone();

        py.detach(move || {
            return terminal
                .preflight_visual_profile(
                    &profile.0,
                    cols.unwrap_or(defaults::COLS),
                    rows.unwrap_or(defaults::ROWS),
                )
                .map(Model)
                .map_err(to_py_err);
        })
    }

    /// Resolve the presentation plan of `edit` over a completed `session` without rendering.
    /// `renderer` is `native` (verified Ghostty pixels, captured first when missing; never
    /// semantic) or `semantic`; `fps` defaults to 30.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` with an edit failure `code` and `details` for a rejected edit,
    /// and for blocked capabilities, invalid evidence, or capture failures.
    #[pyo3(signature = (session, edit, *, renderer, fps = None, semantic_raster_profile = None))]
    pub fn video_plan(
        &self,
        py: Python<'_>,
        session: PathArg,
        edit: EditArg,
        renderer: Arg<VideoRenderer>,
        fps: Option<u16>,
        semantic_raster_profile: Option<Arg<SemanticRasterProfile>>,
    ) -> PyResult<Model<PresentationPlan>> {
        let binary = self.terminal.binary().to_path_buf();
        let options = VideoOptions {
            renderer: renderer.0,
            fps,
            semantic_raster_profile: semantic_raster_profile.map(|profile| return profile.0),
            ..VideoOptions::default()
        };

        py.detach(move || {
            let outcome = video(&binary, session.0, edit.0, VideoMode::Plan, &options)?;
            let VideoOutcome::Planned(plan) = outcome else {
                return Err(PyRuntimeError::new_err("plan returned another outcome"));
            };

            return Ok(Model(*plan));
        })
    }

    /// Render a contact sheet and low-resolution review video of `edit` into the new
    /// `directory`. Options are those of `video_plan`, plus `preview` storyboard settings.
    ///
    /// # Errors
    ///
    /// Raises like `video_plan`, and when the directory exists or rendering fails.
    #[pyo3(signature = (session, edit, directory, *, renderer, fps = None, semantic_raster_profile = None, preview = None))]
    #[expect(clippy::too_many_arguments, reason = "Python keyword arguments")]
    pub fn video_preview(
        &self,
        py: Python<'_>,
        session: PathArg,
        edit: EditArg,
        directory: PathArg,
        renderer: Arg<VideoRenderer>,
        fps: Option<u16>,
        semantic_raster_profile: Option<Arg<SemanticRasterProfile>>,
        preview: Option<Arg<PreviewOptions>>,
    ) -> PyResult<Model<VideoPreview>> {
        let binary = self.terminal.binary().to_path_buf();
        let options = VideoOptions {
            renderer: renderer.0,
            fps,
            semantic_raster_profile: semantic_raster_profile.map(|profile| return profile.0),
            preview: preview.map(|preview| return preview.0),
            ..VideoOptions::default()
        };
        let mode = VideoMode::Preview {
            directory: directory.0,
            options: PreviewOptions::default(),
        };

        py.detach(move || {
            let outcome = video(&binary, session.0, edit.0, mode, &options)?;
            let VideoOutcome::Previewed(preview) = outcome else {
                return Err(PyRuntimeError::new_err("preview returned another outcome"));
            };

            return Ok(Model(*preview));
        })
    }

    /// Render `edit` to `out`, whose lowercase `.mp4`, `.gif`, or `.webm` extension selects
    /// the format, keeping a manifested `.termtrace` sidecar beside it. An existing output
    /// fails unless `overwrite` is set for an MP4; GIF and `WebM` are never replaced.
    ///
    /// # Errors
    ///
    /// Raises like `video_plan`, and when the output exists or rendering fails.
    #[pyo3(signature = (session, edit, out, *, renderer, fps = None, semantic_raster_profile = None, gif_loop = None, overwrite = false))]
    #[expect(clippy::too_many_arguments, reason = "Python keyword arguments")]
    pub fn video_render(
        &self,
        py: Python<'_>,
        session: PathArg,
        edit: EditArg,
        out: PathArg,
        renderer: Arg<VideoRenderer>,
        fps: Option<u16>,
        semantic_raster_profile: Option<Arg<SemanticRasterProfile>>,
        gif_loop: Option<Arg<GifLoop>>,
        overwrite: bool,
    ) -> PyResult<Model<VideoRender>> {
        let binary = self.terminal.binary().to_path_buf();
        let options = VideoOptions {
            renderer: renderer.0,
            fps,
            semantic_raster_profile: semantic_raster_profile.map(|profile| return profile.0),
            gif_loop: gif_loop.map(|policy| return policy.0),
            overwrite,
            ..VideoOptions::default()
        };

        py.detach(move || {
            let outcome = video(
                &binary,
                session.0,
                edit.0,
                VideoMode::Render { out: out.0 },
                &options,
            )?;
            let VideoOutcome::Rendered(render) = outcome else {
                return Err(PyRuntimeError::new_err("render returned another outcome"));
            };

            return Ok(Model(*render));
        })
    }

    /// Launch a recorded command with typed options: `rows`, `cols`, `cwd`, `environment`
    /// (`base`, `set` name to bytes, `copy`, `unset`, `color`), `startup_timeout_ms`,
    /// `output_byte_budget`, `max_duration_ms`, `idle_timeout_ms`, and `visual_profile`.
    ///
    /// Returns once the recorder answers or the session already completed. A cleared environment
    /// still receives the reserved terminal variables.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for unknown or malformed options, and `TermtraceError` or a subclass when
    /// the options are invalid or the command cannot be launched and recorded.
    #[pyo3(signature = (command, options = None))]
    pub fn launch(
        &self,
        py: Python<'_>,
        command: Vec<String>,
        options: Option<Arg<LaunchOptions>>,
    ) -> PyResult<PyRecording> {
        return self.start(py, control::launch_spec(command, options)?);
    }

    /// Launch `command` as an original recording: it runs in a private Ghostty display whose
    /// pixels are captured from before its first byte.
    ///
    /// `options`: `grid` (`cols`, `rows`) or `viewport` (`width`, `height`), `canvas`, `fps`
    /// (24, 30 or 60), `cwd`, `environment`, `ghostty_config` or `visual_profile`, `alias`, `state_root`,
    /// `startup_timeout_ms`, `output_byte_budget`, `max_duration_ms` and `idle_timeout_ms`.
    /// Cancelling `cancel` tears startup down within a bounded deadline.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for malformed options, `OriginalCaptureFailedError` when acquisition
    /// fails or is cancelled, and `TermtraceError` when native capture is unavailable or startup
    /// misses its deadline.
    #[pyo3(signature = (command, options = None, cancel = None))]
    pub fn record_original(
        &self,
        py: Python<'_>,
        command: Vec<String>,
        options: Option<Arg<termtrace_sdk::contracts::OriginalLaunchOptions>>,
        cancel: Option<crate::locator::PyCancelToken>,
    ) -> PyResult<crate::original::PyOriginalRecording> {
        let spec = crate::original::original_launch_spec(command, options)?;

        return crate::original::start_original(py, self.terminal.clone(), spec, cancel);
    }

    /// Attach to an original recording without owning its capture supervisor.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` when the session or its capture status is missing or invalid.
    pub fn attach_original(
        &self,
        py: Python<'_>,
        session_path: String,
    ) -> PyResult<crate::original::PyOriginalRecording> {
        let terminal = self.terminal.clone();

        py.detach(move || {
            let original = terminal.attach_original(&session_path).map_err(to_py_err)?;

            return Ok(crate::original::PyOriginalRecording::new(
                terminal, original,
            ));
        })
    }

    /// Attach to an original recording named by path or registry alias, without owning its
    /// capture supervisor. `str` and `bytes` follow the `path:`/`name:` rules; an
    /// `os.PathLike` is an exact path. Aliases resolve under `state_root`, or the default
    /// registry.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` when the target does not resolve, or the session or its capture
    /// status is missing, invalid, not owner-only, or published for another session.
    #[pyo3(signature = (target, state_root=None))]
    pub fn attach_original_target(
        &self,
        py: Python<'_>,
        target: &Bound<'_, PyAny>,
        state_root: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<crate::original::PyOriginalRecording> {
        let target = crate::original::session_target(target)?;
        let state_root = crate::original::state_root(state_root)?;
        let terminal = self.terminal.clone();

        py.detach(move || {
            let original = terminal
                .attach_original_target(&target, state_root.as_deref())
                .map_err(to_py_err)?;

            return Ok(crate::original::PyOriginalRecording::new(
                terminal, original,
            ));
        })
    }

    /// Attach to a session named by path or registry alias; `path:` and `name:` prefixes are
    /// explicit.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the target does not resolve or cannot be
    /// attached.
    pub fn attach_target(&self, py: Python<'_>, target: String) -> PyResult<PySession> {
        let terminal = self.terminal.clone();

        py.detach(move || {
            let target = SessionTarget::parse(OsStr::new(&target)).map_err(to_py_err)?;
            let session = terminal.attach_target(&target).map_err(to_py_err)?;

            Ok(PySession {
                session_id: session.session_id().to_string(),
                session_path: session.path().to_path_buf(),
                terminal,
            })
        })
    }

    /// Attach to an existing session directory.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when `session_path` is not an attachable session.
    pub fn attach(&self, py: Python<'_>, session_path: String) -> PyResult<PySession> {
        let terminal = self.terminal.clone();
        let path = PathBuf::from(session_path);

        py.detach(move || {
            let session = terminal.attach(&path).map_err(to_py_err)?;
            let session_id = session.session_id().to_string();
            let session_path = session.path().to_path_buf();
            Ok(PySession {
                terminal,
                session_path,
                session_id,
            })
        })
    }

    /// Verify an evidence bundle (.tar.zst or directory) and return whether it passed integrity verification.
    ///
    /// # Errors
    ///
    /// Never raises; failed verification returns `False`.
    #[pyo3(signature = (bundle_path, source_session = None))]
    pub fn verify_bundle(
        &self,
        py: Python<'_>,
        bundle_path: String,
        source_session: Option<String>,
    ) -> PyResult<bool> {
        let path = PathBuf::from(bundle_path);
        let source = source_session.map_or_else(
            || {
                return path.join("canonical");
            },
            PathBuf::from,
        );

        py.detach(move || {
            let policy = BundleVerificationPolicy::new(&source, BundleProfile::Forensic);
            let res = ArtifactService::verify_bundle(&path, policy);

            return Ok(res.is_ok());
        })
    }

    /// Verify a completed session's structure and evidence bindings offline.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the verification report cannot be serialized;
    /// verification findings are part of the returned report.
    #[pyo3(signature = (session_path, level = None))]
    pub fn verify_session(
        &self,
        py: Python<'_>,
        session_path: String,
        level: Option<Arg<VerificationLevel>>,
    ) -> PyResult<Model<VerificationReport>> {
        let path = PathBuf::from(session_path);
        let lvl = level.map_or(VerificationLevel::Content, |level| return level.0);
        py.detach(move || {
            let report = IntegrityService::verify(&path, lvl, None);

            return Ok(Model(report));
        })
    }

    /// Recover the longest integrity-checked event prefix from a damaged session.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when no integrity-checked prefix can be recovered and
    /// published to `destination`, or the report cannot be serialized.
    pub fn recover_session(
        &self,
        py: Python<'_>,
        session_path: String,
        destination: String,
    ) -> PyResult<Model<RecoveryReport>> {
        let src = PathBuf::from(session_path);
        let dest = PathBuf::from(destination);

        py.detach(move || {
            let report = IntegrityService::recover_session(&src, &dest).map_err(to_py_err)?;

            return Ok(Model(report));
        })
    }

    /// List registered sessions in the default user registry.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the default registry cannot be opened or read, or
    /// the entries cannot be serialized.
    #[pyo3(signature = (include_all = None))]
    pub fn registry_list(
        &self,
        py: Python<'_>,
        include_all: Option<bool>,
    ) -> PyResult<Vec<Model<RegistryListEntry>>> {
        let all = include_all.unwrap_or(false);

        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            let entries = reg.list(all).map_err(to_py_err)?;

            return Ok(entries.into_iter().map(Model).collect());
        })
    }

    /// Resolve a friendly session name to its registered session path.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the default registry cannot be opened; an
    /// unresolvable name returns `None`.
    pub fn registry_resolve(&self, py: Python<'_>, name: String) -> PyResult<Option<String>> {
        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            match reg.resolve_optional(&name).map_err(to_py_err)? {
                Some(entry) => {
                    return Ok(Some(entry.session_path().display().to_string()));
                }
                None => {
                    return Ok(None);
                }
            }
        })
    }

    /// Register a friendly name for an existing canonical session directory.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the default registry cannot be opened, `name` is
    /// invalid or already registered, `session_path` is not a registrable session, or the entry
    /// cannot be serialized.
    pub fn registry_register(
        &self,
        py: Python<'_>,
        name: String,
        session_path: String,
    ) -> PyResult<Model<RegistryEntry>> {
        let path = PathBuf::from(session_path);

        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            let entry = reg.register(&name, &path).map_err(to_py_err)?;

            return Ok(Model(entry));
        })
    }

    /// Remove a friendly name binding.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the default registry cannot be opened; a failed
    /// removal returns `False`.
    pub fn registry_remove(&self, py: Python<'_>, name: String) -> PyResult<bool> {
        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            let res = reg.remove(&name);

            return Ok(res.is_ok());
        })
    }

    /// Plan or execute garbage collection over completed sessions.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the default registry cannot be opened, garbage
    /// collection cannot be planned or applied, or the report cannot be serialized.
    #[pyo3(signature = (older_than_days = None, keep_latest = None, max_completed_bytes = None, apply = None))]
    pub fn registry_gc(
        &self,
        py: Python<'_>,
        older_than_days: Option<u64>,
        keep_latest: Option<u64>,
        max_completed_bytes: Option<u64>,
        apply: Option<bool>,
    ) -> PyResult<Model<GarbageCollectionReport>> {
        let days = older_than_days.unwrap_or(defaults::GC_OLDER_THAN_DAYS);
        let keep = keep_latest.unwrap_or(defaults::GC_KEEP_LATEST);
        let is_apply = apply.unwrap_or(false);

        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            let report = reg
                .gc(days, keep, max_completed_bytes, is_apply)
                .map_err(to_py_err)?;

            return Ok(Model(report));
        })
    }

    /// Bundle an aggregate tmux recording into an evidentiary streaming bundle.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the aggregate tmux recording cannot be bundled or
    /// published, or the report cannot be serialized.
    pub fn tmux_bundle(
        &self,
        py: Python<'_>,
        manifest_path: String,
        destination: String,
    ) -> PyResult<Model<TmuxStreamingBundleManifest>> {
        let manifest = PathBuf::from(manifest_path);
        let dest = PathBuf::from(destination);

        py.detach(move || {
            let report = TmuxService::bundle(&manifest, &dest).map_err(to_py_err)?;

            return Ok(Model(report));
        })
    }

    /// Verify an aggregate tmux streaming bundle against its manifest.
    ///
    /// # Errors
    ///
    /// Never raises; failed verification returns `False`.
    pub fn tmux_verify_bundle(
        &self,
        py: Python<'_>,
        bundle_directory: String,
        manifest_path: String,
    ) -> PyResult<bool> {
        let bundle_dir = PathBuf::from(bundle_directory);
        let manifest = PathBuf::from(manifest_path);

        py.detach(move || {
            let res = TmuxService::verify_bundle(&bundle_dir, &manifest);

            return Ok(res.is_ok());
        })
    }

    /// Probe runtime system capabilities across engine, media, and evidence signers.
    ///
    /// Cancelling `cancel` stops the native capture probe early; it then reports blocked.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass only when the capability report cannot be serialized.
    #[pyo3(signature = (cancel = None))]
    pub fn probe_capabilities(
        &self,
        py: Python<'_>,
        cancel: Option<crate::locator::PyCancelToken>,
    ) -> PyResult<Model<CapabilityReport>> {
        py.detach(move || {
            let token = cancel.map(|token| return token.inner);
            let report =
                WaitCancellation::forward(token.as_ref(), SystemService::probe_capabilities);

            return Ok(Model(report));
        })
    }
}
