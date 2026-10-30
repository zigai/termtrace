#![no_main]

use libfuzzer_sys::fuzz_target;
use termtrace_model::terminal::effect::TerminalEffect;
use termtrace_terminal::{semantic_state_hash, Format, GhosttyVt};

// Ghostty snapshots do not carry an in-flight Kitty clipboard (OSC 5522) transaction.
fn comparable(effects: Vec<TerminalEffect>) -> Vec<TerminalEffect> {
    return effects
        .into_iter()
        .filter(|effect| return !matches!(effect, TerminalEffect::ClipboardWrite { .. }))
        .collect();
}

fuzz_target!(|bytes: &[u8]| {
    let mut vt = GhosttyVt::new(80, 24, 1000).expect("valid fixed terminal geometry");
    vt.capture_effects().expect("effect callbacks");
    let split = bytes.first().map_or(0, |&byte| usize::from(byte) % (bytes.len() + 1));
    let (prefix, suffix) = bytes.split_at(split);
    vt.write(prefix);
    drop(vt.take_effects());

    let checkpoint = vt
        .encode_checkpoint()
        .expect("checkpoint after arbitrary terminal output");
    let mut restored = checkpoint.map(|checkpoint| {
        GhosttyVt::restore_checkpoint(&checkpoint, 1000).expect("restore encoded checkpoint")
    });

    vt.write(suffix);
    let snapshot = vt
        .snapshot()
        .expect("snapshot after arbitrary terminal output");
    let _responses = vt.take_responses();
    let effects = comparable(vt.take_effects());

    if let Some(restored) = restored.as_mut() {
        restored.write(suffix);
        assert_eq!(comparable(restored.take_effects()), effects);
        let restored_snapshot = restored.snapshot().expect("snapshot after restore");

        assert_eq!(
            semantic_state_hash(&restored_snapshot).expect("restored hash"),
            semantic_state_hash(&snapshot).expect("original hash")
        );
        assert_eq!(
            restored.format(Format::Vt).expect("restored VT"),
            vt.format(Format::Vt).expect("original VT")
        );
    }

    let mut stepped = GhosttyVt::new(80, 24, 1000).expect("valid fixed terminal geometry");
    stepped
        .capture_render_holds()
        .expect("render hold callback");
    stepped.capture_effects().expect("effect callbacks");
    let mut rest = bytes;
    while !rest.is_empty() {
        let step = stepped.write_step(rest);
        assert!(step.consumed > 0 && step.plain <= step.consumed && step.consumed <= rest.len());
        let _holds = stepped
            .take_render_holds()
            .expect("frame at the start of a render hold");
        let _responses = stepped.take_responses();
        let _effects = stepped.take_effects();
        rest = &rest[step.consumed..];
    }
});
