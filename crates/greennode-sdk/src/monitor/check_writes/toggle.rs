use super::{Check, Client, Mutation, Privacy};
use crate::monitor::inventory::get_input;
use crate::monitor::mutation::{WriteError, before_write, maybe_sent, not_sent};
use crate::monitor::wire::check_path_id;
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use http::{Method, StatusCode};
use serde::Serialize;
use std::time::Duration;

const STATUSES: &[StatusCode] = &[
    StatusCode::OK,
    StatusCode::CREATED,
    StatusCode::ACCEPTED,
    StatusCode::NON_AUTHORITATIVE_INFORMATION,
    StatusCode::NO_CONTENT,
    StatusCode::RESET_CONTENT,
    StatusCode::PARTIAL_CONTENT,
];
const PAUSE: Mutation = Mutation {
    success: STATUSES,
    ..Mutation::standard(
        "monitor.PauseCheck",
        "vmonitor-uptime-manager/v1/uptimes/status/{}",
        &Method::PUT,
    )
};
const RESUME: Mutation = Mutation {
    id: "monitor.ResumeCheck",
    ..PAUSE
};
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

get_input!(
    /// Selects a check to pause.
    PauseCheckInput,
    /// Check identifier.
    check_id
);
get_input!(
    /// Selects a check to resume.
    ResumeCheckInput,
    /// Check identifier.
    check_id
);

macro_rules! result_toggle {
    ($(#[$meta:meta])* $name:ident)=>{
        $(#[$meta])*
        #[derive(Clone,Debug,Serialize)]
        #[serde(rename_all="PascalCase")]
        pub struct $name {
            /// Check whose resulting status was confirmed.
            pub check:Check,
            /// Whether this call sent a toggle to reach the target status.
            pub changed:bool,
        }
    };
}
result_toggle!(
    /// Result of driving a check to the disabled state.
    PauseCheckOutput
);
result_toggle!(
    /// Result of driving a check to the enabled state.
    ResumeCheckOutput
);

impl Client {
    /// Pauses a check and confirms that its status is disabled.
    pub async fn pause_check(
        &self,
        ctx: &RequestContext,
        input: PauseCheckInput,
    ) -> Result<PauseCheckOutput, Failure<PauseCheckOutput>> {
        let (check, changed) = self
            .toggle_check(ctx, &PAUSE, &input.check_id, DISABLED)
            .await?;
        Ok(PauseCheckOutput { check, changed })
    }
    /// Resumes a check and confirms that its status is enabled.
    pub async fn resume_check(
        &self,
        ctx: &RequestContext,
        input: ResumeCheckInput,
    ) -> Result<ResumeCheckOutput, Failure<ResumeCheckOutput>> {
        let (check, changed) = self
            .toggle_check(ctx, &RESUME, &input.check_id, ENABLED)
            .await?;
        Ok(ResumeCheckOutput { check, changed })
    }
    async fn toggle_check<T>(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        id: &str,
        target: &str,
    ) -> Result<(Check, bool), Failure<T>> {
        check_path_id(mutation.id, "CheckID", id).map_err(before_write)?;
        let mut evidence = self.write_evidence(ctx, &[id]).map_err(not_sent)?;
        let _guard = self.check_lock(ctx, id).await.map_err(not_sent)?;
        let mut privacy = Privacy::default();
        let current = self
            .toggle_snapshot(ctx, mutation, id, &mut evidence, &mut privacy)
            .await
            .map_err(not_sent)?;
        if current.status == target {
            return Ok((current, false));
        }
        if ![ENABLED, DISABLED].contains(&current.status.as_str()) {
            let value = &current.status[..current.status.floor_char_boundary(64)];
            let message = format!(
                "monitor: unexpected check status: {id} is {}",
                crate::go_text::go_quote(value)
            );
            let message = if evidence.text_reflects(&message) || privacy.withheld(&message) {
                "monitor: unexpected check status".into()
            } else {
                message
            };
            let error = Error::new(ErrorClass::Api)
                .with_outer_detail(WriteError::UnexpectedStatus)
                .with_outer_detail(message);
            return Err(not_sent(error));
        }
        let result = self
            .mutate::<T>(ctx, mutation, &[id], None, &mut evidence)
            .await;
        let put_error = match result {
            Ok(_) => None,
            Err(failure) => {
                let refused = failure.write_dispatch() == WriteDispatch::NotSent
                    || failure.error().api_error().is_some_and(|api| {
                        api.status()
                            .is_some_and(|status| (400..500).contains(&status))
                    });
                if refused {
                    return Err(privacy.screen_failure(failure));
                }
                Some(failure.into_parts().0)
            }
        };
        let clock = self.session.clock();
        let mut last_status = String::new();
        let mut last_error = None;
        for seconds in [0, 1, 2, 4] {
            if seconds > 0 {
                let wait =
                    ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(seconds));
                wait.wait(clock).await;
            }
            if let Some(end) = ctx.check(clock) {
                last_error = Some(end.into());
                break;
            }
            let found = self
                .toggle_snapshot(ctx, mutation, id, &mut evidence, &mut privacy)
                .await;
            if let Some(end) = ctx.check(clock) {
                last_error = Some(end.into());
                break;
            }
            match found {
                Ok(found) if found.status == target => return Ok((found, true)),
                Ok(found) => {
                    last_status = found.status;
                    last_error = None;
                }
                Err(error) => last_error = Some(error),
            }
        }
        let status = if last_status.is_empty()
            || evidence.text_reflects(&last_status)
            || privacy.withheld(&last_status)
        {
            "unknown"
        } else {
            &last_status
        };
        let recovery = if target == DISABLED {
            "treat the pause as done and resume later"
        } else {
            "ask a person to check it"
        };
        let message = format!(
            "monitor: check status not confirmed: toggle sent or may have been sent, check still {status}; {recovery}"
        );
        let message = if evidence.text_reflects(&message) || privacy.withheld(&message) {
            "monitor: check status not confirmed".into()
        } else {
            message
        };
        let mut error = Error::new(ErrorClass::Api)
            .with_outer_detail(WriteError::StatusUnconfirmed)
            .with_outer_detail(message);
        for cause in [put_error, last_error].into_iter().flatten() {
            error = error.with_cause(cause);
        }
        Err(maybe_sent(privacy.screen_error(
            error.screen_api(&|text| evidence.text_reflects(text)),
        )))
    }
    async fn toggle_snapshot(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        id: &str,
        evidence: &mut Evidence,
        privacy: &mut Privacy,
    ) -> Result<Check, Error> {
        let check = self
            .read_check_guard(ctx, mutation.id, id, evidence)
            .await?;
        privacy.add_request(&check.config.request);
        privacy.validate(mutation, 200, &check, evidence)?;
        if check.id != id {
            return Err(mutation.error(
                Some(200),
                "InvalidResponse",
                "check confirmation did not match the requested identity",
                evidence,
            ));
        }
        Ok(check)
    }
}
