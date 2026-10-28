mod support;

use support::{scratch, screen_text, shell, stop, terminal, wait_text, Launch};
use termtrace_sdk::{
    registry::RegistryService,
    session::{LaunchSpec, SessionTarget},
};

#[test]
fn attach_target_resolves_paths_and_registry_aliases() {
    let state = scratch("target-state").join("state");
    // SAFETY: this is the only test in this binary, so no other thread reads the environment.
    unsafe { std::env::set_var("TERMTRACE_STATE_DIR", &state) };
    let terminal = terminal("target-session");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(
            "printf TARGET-READY; IFS= read -r _",
        )))
        .expect("launch");
    wait_text(&session, "TARGET-READY");
    RegistryService::open(&state)
        .expect("registry")
        .register("sample-alias", session.path())
        .expect("register");

    let by_path = terminal
        .attach_target(&SessionTarget::Path(session.path().to_path_buf()))
        .expect("attach by path");
    let by_name = terminal
        .attach_target(&SessionTarget::Name("sample-alias".to_owned()))
        .expect("attach by alias");

    assert_eq!(by_path.path(), session.path());
    assert_eq!(by_name.path(), session.path());
    assert!(screen_text(&by_name).contains("TARGET-READY"));
    assert!(terminal
        .attach_target(&SessionTarget::Name("unknown-alias".to_owned()))
        .is_err());
    stop(&mut session);
}
