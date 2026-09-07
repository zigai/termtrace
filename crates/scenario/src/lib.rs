//! Scenario document validation and execution.

#![deny(unsafe_code)]

mod artifact;
mod document;
mod error;
mod execution;
mod suite;

pub use error::{Error, Result};
pub use execution::run;
pub use suite::{ScenarioSuiteOptions, run_suite, scenario_junit, write_scenario_junit};
