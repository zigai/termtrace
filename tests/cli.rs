//! End-to-end tests for the public command-line interface.

#[path = "cli/support.rs"]
mod support;

#[path = "cli/adoption_surfaces.rs"]
mod adoption_surfaces;
#[path = "cli/artifacts_and_bundles.rs"]
mod artifacts_and_bundles;
#[path = "cli/command_surface.rs"]
mod command_surface;
#[path = "cli/drivers_and_clients.rs"]
mod drivers_and_clients;
#[path = "cli/integrity_and_receipts.rs"]
mod integrity_and_receipts;
#[path = "cli/launch_validation_and_stdio.rs"]
mod launch_validation_and_stdio;
#[path = "cli/live_session_control.rs"]
mod live_session_control;
#[path = "cli/output_events_and_postprocessing.rs"]
mod output_events_and_postprocessing;
#[path = "cli/recording_basics.rs"]
mod recording_basics;
#[path = "cli/replay_index_and_playback.rs"]
mod replay_index_and_playback;
#[path = "cli/scenario_and_reviewer_workflows.rs"]
mod scenario_and_reviewer_workflows;
#[path = "cli/semantic_input.rs"]
mod semantic_input;
#[path = "cli/session_registry_and_lifecycle.rs"]
mod session_registry_and_lifecycle;
#[path = "cli/show_workflows.rs"]
mod show_workflows;
#[path = "cli/timeline_analysis.rs"]
mod timeline_analysis;
#[path = "cli/tmux_recording.rs"]
mod tmux_recording;
