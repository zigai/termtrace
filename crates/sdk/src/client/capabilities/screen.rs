use super::super::control::{
    client_error, exact_duration_ns, public_control_error, validate_stable_options,
};
use super::super::{
    CaptureQuality, ControlCommand, ControlData, ControlRequest, ControlResult, Duration, Error,
    ExchangeTiming, FrameDetail, Observation, Result, ScreenFrame, ScreenPredicate,
    ScreenProjectionRequest, ScreenRegion, ScreenWaitOutcome, Session, StableCaptureOptions,
    StableCaptureQuality, StatePoint, Uuid, exchange, observe_session_with_options,
    project_screen_frame, resolved_screen_state_for_session, send_request_response,
    wait_for_screen,
};

/// Presented-screen operations.
pub struct Screen<'session> {
    pub(in crate::client) session: &'session Session,
}

/// Atomic stable-screen outcome retaining the exact actor frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StableScreenCapture {
    /// Typed reason the actor returned.
    pub quality: StableCaptureQuality,
    /// Exact frame projected before waiter ordering was released.
    pub frame: ScreenFrame,
    /// First actor timestamp at which this exact state was observed.
    pub first_observed_monotonic_ns: u64,
    /// Actor timestamp of the returned snapshot.
    pub observed_monotonic_ns: u64,
    /// Requested unchanged interval.
    pub stable_for_ns: u64,
}

impl Screen<'_> {
    pub(crate) fn session(&self) -> &Session {
        return self.session;
    }

    /// Return one bounded rich latest-presented frame from a live or completed session.
    pub fn frame(&self, projection: ScreenProjectionRequest) -> Result<ScreenFrame> {
        if self.session.path.join("completion.json").is_file() {
            let (state, source, presented) = resolved_screen_state_for_session(
                &self.session.path,
                StatePoint::LatestPresented,
                false,
            )?;
            return project_screen_frame(
                &state,
                &projection,
                source,
                presented,
                CaptureQuality::Completed,
            )
            .map_err(Error::ScreenFrame);
        }
        let request = ControlRequest::new(ControlCommand::ScreenFrame { projection });
        let response = send_request_response(&self.session.path, &request)?;
        let data = match response.result {
            ControlResult::Ok { data } => data,
            ControlResult::Error {
                code,
                message,
                details,
            } => return Err(public_control_error(code, message, details, None, None)),
        };
        let ControlData::ScreenFrame { frame } = data else {
            return Err(client_error("actor returned the wrong rich-screen payload"));
        };
        return Ok(frame);
    }

    /// Return exact cells for one explicit absolute region.
    pub fn cells(&self, region: ScreenRegion) -> Result<ScreenFrame> {
        return self.frame(ScreenProjectionRequest {
            detail: FrameDetail::Cells,
            region: Some(region),
        });
    }

    /// Return a truth-labeled latest presented compact observation.
    pub fn observation(&self) -> Result<Observation> {
        return observe_session_with_options(
            &self.session.path,
            StatePoint::LatestPresented,
            false,
        );
    }

    /// Wait inside the actor for a stable presented predicate.
    pub fn wait(
        &self,
        predicate: ScreenPredicate,
        stable_for: Duration,
        timeout: Duration,
    ) -> Result<ScreenWaitOutcome> {
        return wait_for_screen(&self.session.path, predicate, stable_for, timeout);
    }

    /// Atomically capture one exact rich frame under actor stability ordering.
    pub fn capture_stable(&self, options: StableCaptureOptions) -> Result<StableScreenCapture> {
        validate_stable_options(options)?;
        let stable_for_ns = exact_duration_ns("stable_for", options.stable_for)?;
        if self.session.path.join("completion.json").is_file() {
            return Ok(StableScreenCapture {
                quality: StableCaptureQuality::Stable,
                frame: self.frame(options.projection)?,
                first_observed_monotonic_ns: 0,
                observed_monotonic_ns: 0,
                stable_for_ns,
            });
        }
        let mut request = ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns,
            timeout_ns: exact_duration_ns("timeout", options.timeout)?,
            projection: options.projection,
        });
        request.request_id = Uuid::now_v7();
        let response = exchange(
            &self.session.path,
            &request,
            ExchangeTiming::deadline(options.timeout)?,
        )?;
        let data = match response.result {
            ControlResult::Ok { data } => data,
            ControlResult::Error {
                code,
                message,
                details,
            } => return Err(public_control_error(code, message, details, None, None)),
        };
        let ControlData::StableSnapshot {
            frame,
            first_observed_monotonic_ns,
            observed_monotonic_ns,
            stable_for_ns,
            quality,
            semantic_state_zstd: _,
        } = data
        else {
            return Err(client_error(
                "actor returned the wrong stable-capture payload",
            ));
        };
        let capture = StableScreenCapture {
            quality,
            frame,
            first_observed_monotonic_ns,
            observed_monotonic_ns,
            stable_for_ns,
        };
        if quality != StableCaptureQuality::Stable && !options.allow_incomplete {
            return Err(Error::StableCaptureIncomplete {
                result: Box::new(capture),
            });
        }
        return Ok(capture);
    }
}
