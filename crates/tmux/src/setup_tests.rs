use super::*;
use std::ffi::OsStr;
use tempfile::TempDir;

fn request() -> SetupRequest {
    return SetupRequest {
        launch_hook: false,
        profile_mode: GhosttyProfileMode::Off,
        ghostty_config: None,
        coverage: TmuxRecordingCoverage::Warn,
        aggregate_manifest: None,
        explicit_only: false,
        targets: ScopeTargets::default(),
        cell_width: 12,
        cell_height: 24,
        term: None,
        retention: PaneRetention::default(),
    };
}

#[test]
fn explicit_executable_preserves_exact_path_quoting_and_setup_policies() {
    let root = TempDir::new().expect("private root");
    let binary = root
        .path()
        .join(OsStr::from_bytes(b"selected ' helper-\xff"));

    fs_err::write(&binary, b"private setup fixture, not executed").expect("helper file");
    let setup = PreparedTmuxSetup::prepare_with_executable(
        request(),
        PathBuf::from("/usr/bin/tmux"),
        root.path(),
        &binary,
    )
    .expect("explicit setup");
    assert_eq!(
        setup.termtrace,
        fs_err::canonicalize(&binary).expect("canonical selected helper")
    );
    assert_eq!(setup.profile_mode, GhosttyProfileMode::Off);
    assert_eq!(setup.coverage, TmuxRecordingCoverage::Warn);
    assert_eq!((setup.cell_width, setup.cell_height), (12, 24));
    assert!(!setup.retention.is_enabled());

    let fragment =
        String::from_utf8(setup.config_fragment().expect("config bytes")).expect("octal config");
    assert!(fragment.contains(&format!(
        "set-option -gq {} {}\n",
        options::TERMTRACE,
        config_quote(setup.termtrace.as_os_str().as_bytes())
    )));
    let command = shell_quote(setup.termtrace.as_os_str().as_bytes());
    assert!(fragment.contains(&config_quote(
        &[
            b"exec env TERMTRACE_TMUX_BINARY=".as_slice(),
            shell_quote(b"/usr/bin/tmux").as_slice(),
            b" ".as_slice(),
            command.as_slice(),
            b" tmux run -q".as_slice(),
        ]
        .concat()
    )));
}

#[test]
fn legacy_setup_and_explicit_current_binary_have_identical_configuration_bytes() {
    let root = TempDir::new().expect("private root");
    let tmux = PathBuf::from("/usr/bin/tmux");
    let current = env::current_exe().expect("legacy CLI executable");
    let legacy =
        PreparedTmuxSetup::prepare(request(), tmux.clone(), root.path()).expect("legacy API");
    let explicit =
        PreparedTmuxSetup::prepare_with_executable(request(), tmux, root.path(), &current)
            .expect("explicit API");

    assert_eq!(
        legacy.termtrace,
        fs_err::canonicalize(current).expect("canonical legacy executable")
    );
    assert_eq!(
        legacy.config_fragment().expect("legacy config"),
        explicit.config_fragment().expect("explicit config")
    );
}

#[test]
fn explicit_missing_binary_never_falls_back_to_the_host() {
    let root = TempDir::new().expect("private root");
    let missing = root.path().join("missing-selected-recorder");
    let error = PreparedTmuxSetup::prepare_with_executable(
        request(),
        PathBuf::from("/usr/bin/tmux"),
        root.path(),
        &missing,
    )
    .expect_err("must reject missing binary");
    assert!(matches!(error, Error::Filesystem { path, .. } if path == missing));
    assert!(!missing.exists());

    let mut invalid = request();
    invalid.cell_width = 0;

    assert!(PreparedTmuxSetup::prepare_with_executable(
        invalid,
        PathBuf::from("/usr/bin/tmux"),
        root.path(),
        &missing
    )
    .is_err());
}
