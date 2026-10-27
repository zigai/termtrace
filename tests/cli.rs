//! End-to-end tests for the public command-line interface.

#[path = "cli/integration/adoption_surfaces.rs"]
mod adoption_surfaces;
#[path = "cli/integration/agent_workflows.rs"]
mod agent_workflows;
#[path = "cli/integration/artifacts_and_bundles.rs"]
mod artifacts_and_bundles;
#[path = "cli/contracts/atomic_action_conformance.rs"]
mod atomic_action_conformance;
#[path = "cli/integration/command_surface.rs"]
mod command_surface;
#[path = "cli/integration/integrity_and_receipts.rs"]
mod integrity_and_receipts;
#[path = "cli/integration/launch_validation_and_stdio.rs"]
mod launch_validation_and_stdio;
#[path = "cli/integration/live_session_control/mod.rs"]
mod live_session_control;
#[path = "cli/integration/mcp.rs"]
mod mcp;
#[path = "cli/integration/original_native.rs"]
mod original_native;
#[path = "cli/integration/output_events_and_postprocessing.rs"]
mod output_events_and_postprocessing;
#[path = "cli/integration/recording_basics.rs"]
mod recording_basics;
#[path = "cli/integration/replay_index_and_playback.rs"]
mod replay_index_and_playback;
#[path = "cli/contracts/response_contract.rs"]
mod response_contract;
#[path = "cli/integration/scenario_and_reviewer_workflows.rs"]
mod scenario_and_reviewer_workflows;
#[path = "cli/contracts/sdk_contract.rs"]
mod sdk_contract;
#[path = "cli/integration/semantic_input.rs"]
mod semantic_input;
#[path = "cli/integration/session_registry_and_lifecycle/mod.rs"]
mod session_registry_and_lifecycle;
#[path = "cli/integration/show_workflows.rs"]
mod show_workflows;
#[path = "cli/support/mod.rs"]
mod support;
#[path = "cli/integration/task_oriented_workflows.rs"]
mod task_oriented_workflows;
#[path = "cli/contracts/testing_adapters.rs"]
mod testing_adapters;
#[path = "cli/integration/timeline_analysis.rs"]
mod timeline_analysis;
#[path = "cli/integration/tmux_recording/mod.rs"]
mod tmux_recording;
#[path = "cli/contracts/workflow_conformance.rs"]
mod workflow_conformance;
