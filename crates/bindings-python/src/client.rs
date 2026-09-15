//! Main Termtrace client and facade for Python.

use crate::errors::to_py_err;
use crate::recording::PyRecording;
use crate::session::PySession;
use pyo3::prelude::*;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use termtrace_model::artifact::bundle::{BundleProfile, BundleVerificationPolicy};
use termtrace_model::execution::session::VerificationLevel;
use termtrace_sdk::artifacts::ArtifactService;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::integrity::IntegrityService;
use termtrace_sdk::registry::RegistryService;
use termtrace_sdk::system::SystemService;
use termtrace_sdk::tmux::TmuxService;

fn default_termtrace_bin() -> PathBuf {
    if let Ok(bin) = std::env::var("TERMTRACE_BIN") {
        return PathBuf::from(bin);
    }

    // Check if termtrace is next to current executable or in PATH
    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe.with_file_name("termtrace");
        if candidate.is_file() {
            return candidate;
        }
    }

    return PathBuf::from("termtrace");
}

fn default_evidence_root() -> PathBuf {
    if let Ok(root) = std::env::var("TERMTRACE_ROOT") {
        return PathBuf::from(root);
    }
    return std::env::temp_dir().join("termtrace");
}

/// Primary entry point for launching and managing Termtrace recordings in Python.
#[pyclass(name = "Termtrace", from_py_object)]
#[derive(Clone)]
pub struct PyTermtrace {
    pub(crate) terminal: Terminal,
}

#[pymethods]
impl PyTermtrace {
    /// Create a new Termtrace client instance with optional binary and evidence root overrides.
    #[new]
    #[pyo3(signature = (binary = None, evidence_root = None))]
    pub fn new(binary: Option<String>, evidence_root: Option<String>) -> Self {
        let bin_path = binary.map_or_else(default_termtrace_bin, PathBuf::from);
        let root_path = evidence_root.map_or_else(default_evidence_root, PathBuf::from);
        Self {
            terminal: Terminal::new(bin_path, root_path),
        }
    }

    /// Launch a new recorded command session.
    #[pyo3(signature = (command, rows = None, cols = None, env = None, cwd = None))]
    pub fn record(
        &self,
        py: Python<'_>,
        command: Vec<String>,
        rows: Option<u16>,
        cols: Option<u16>,
        env: Option<HashMap<String, String>>,
        cwd: Option<String>,
    ) -> PyResult<PyRecording> {
        let terminal = self.terminal.clone();
        let cmd_os: Vec<OsString> = command.into_iter().map(OsString::from).collect();
        let rows_val = rows.unwrap_or(24);
        let cols_val = cols.unwrap_or(80);

        py.detach(move || {
            let mut builder = terminal.launch(cmd_os).geometry(cols_val, rows_val);
            if let Some(dir) = cwd {
                builder = builder.working_directory(dir);
            }
            if let Some(env_map) = env {
                for (k, v) in env_map {
                    builder = builder.set_environment(k, v.into_bytes());
                }
            }

            let session = builder.start().map_err(to_py_err)?;
            let session_id = session.session_id().to_string();
            let path = session.path().display().to_string();

            Ok(PyRecording {
                terminal: terminal.clone(),
                session: Arc::new(Mutex::new(Some(session))),
                session_id,
                path,
            })
        })
    }

    /// Attach to an existing session directory.
    pub fn attach(&self, py: Python<'_>, session_path: String) -> PyResult<PySession> {
        let terminal = self.terminal.clone();
        let path = PathBuf::from(session_path);

        py.detach(move || {
            let session = terminal.attach(&path).map_err(to_py_err)?;
            let session_id = session.session_id().to_string();
            let session_path = session.path().display().to_string();
            Ok(PySession {
                terminal,
                session_path,
                session_id,
            })
        })
    }

    /// Verify an evidence bundle (.tar.zst or directory) and return whether it passed integrity verification.
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
    #[pyo3(signature = (session_path, level = None))]
    pub fn verify_session(
        &self,
        py: Python<'_>,
        session_path: String,
        level: Option<String>,
    ) -> PyResult<String> {
        let path = PathBuf::from(session_path);
        let lvl = match level
            .as_deref()
            .unwrap_or("content")
            .to_ascii_lowercase()
            .as_str()
        {
            "structure" => VerificationLevel::Structure,
            "durability" | "durability_record" | "durability-record" => {
                VerificationLevel::DurabilityRecord
            }
            "signature" => VerificationLevel::Signature,
            _ => VerificationLevel::Content,
        };
        py.detach(move || {
            let report = IntegrityService::verify(&path, lvl, None);

            return serde_json::to_string_pretty(&report).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: path.clone(),
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Recover the longest integrity-checked event prefix from a damaged session.
    pub fn recover_session(
        &self,
        py: Python<'_>,
        session_path: String,
        destination: String,
    ) -> PyResult<String> {
        let src = PathBuf::from(session_path);
        let dest = PathBuf::from(destination);

        py.detach(move || {
            let report = IntegrityService::recover_session(&src, &dest).map_err(to_py_err)?;

            return serde_json::to_string_pretty(&report).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: dest,
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// List registered sessions in the default user registry.
    #[pyo3(signature = (include_all = None))]
    pub fn registry_list(&self, py: Python<'_>, include_all: Option<bool>) -> PyResult<String> {
        let all = include_all.unwrap_or(false);

        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            let entries = reg.list(all).map_err(to_py_err)?;

            return serde_json::to_string_pretty(&entries).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: PathBuf::from("registry"),
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Resolve a friendly session name to its registered session path.
    pub fn registry_resolve(&self, py: Python<'_>, name: String) -> PyResult<Option<String>> {
        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            match reg.resolve(&name) {
                Ok(entry) => {
                    return Ok(Some(entry.session_path().display().to_string()));
                }
                Err(_) => {
                    return Ok(None);
                }
            }
        })
    }

    /// Register a friendly name for an existing canonical session directory.
    pub fn registry_register(
        &self,
        py: Python<'_>,
        name: String,
        session_path: String,
    ) -> PyResult<String> {
        let path = PathBuf::from(session_path);

        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            let entry = reg.register(&name, &path).map_err(to_py_err)?;

            return serde_json::to_string_pretty(&entry).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: path.clone(),
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Remove a friendly name binding.
    pub fn registry_remove(&self, py: Python<'_>, name: String) -> PyResult<bool> {
        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            let res = reg.remove(&name);

            return Ok(res.is_ok());
        })
    }

    /// Plan or execute garbage collection over completed sessions.
    #[pyo3(signature = (older_than_days = None, keep_latest = None, max_completed_bytes = None, apply = None))]
    pub fn registry_gc(
        &self,
        py: Python<'_>,
        older_than_days: Option<u64>,
        keep_latest: Option<u64>,
        max_completed_bytes: Option<u64>,
        apply: Option<bool>,
    ) -> PyResult<String> {
        let days = older_than_days.unwrap_or(30);
        let keep = keep_latest.unwrap_or(10);
        let is_apply = apply.unwrap_or(false);

        py.detach(move || {
            let reg = RegistryService::open_default().map_err(to_py_err)?;
            let report = reg
                .gc(days, keep, max_completed_bytes, is_apply)
                .map_err(to_py_err)?;

            return serde_json::to_string_pretty(&report).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: PathBuf::from("registry"),
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Bundle an aggregate tmux recording into an evidentiary streaming bundle.
    pub fn tmux_bundle(
        &self,
        py: Python<'_>,
        manifest_path: String,
        destination: String,
    ) -> PyResult<String> {
        let manifest = PathBuf::from(manifest_path);
        let dest = PathBuf::from(destination);

        py.detach(move || {
            let report = TmuxService::bundle(&manifest, &dest).map_err(to_py_err)?;

            return serde_json::to_string_pretty(&report).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: dest,
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Verify an aggregate tmux streaming bundle against its manifest.
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
    pub fn probe_capabilities(&self, py: Python<'_>) -> PyResult<String> {
        py.detach(move || {
            let report = SystemService::probe_capabilities();

            return serde_json::to_string_pretty(&report).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: PathBuf::from("capabilities"),
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }
}
