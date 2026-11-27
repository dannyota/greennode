use std::time::Duration;

use http::StatusCode;

use super::super::{
    status::{decode_quota, read_synthetic_detail},
    wire::{Object, Reader},
};
use super::state::{Attempt, Goal, Kind, Observation, POLL, Snapshot, field, unsettled};
use crate::model::wire::WireField;
use crate::monitor::settlement::Wait;
use crate::monitor::wire::{BodyReadFailure, Family};
use crate::monitor::{Client, Quota};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, RequestContext};

impl Client {
    pub(super) async fn reconcile_quota(
        &self,
        ctx: &RequestContext,
        attempt: &mut Attempt,
        op: &'static str,
        bound: Duration,
    ) -> Result<Option<Quota>, Error> {
        let result = if bound.is_zero() {
            self.observe_attempt(ctx, attempt).await
        } else {
            let mut wait = Wait::new(ctx, self.session.clock(), POLL, bound);
            loop {
                let turn = match wait.next().await {
                    Ok(turn) => turn,
                    Err(error) => break Err(error),
                };
                let outcome = self.observe_attempt(turn, attempt).await;
                if let Err(error) = wait.check() {
                    break Err(error);
                }
                match outcome {
                    Ok(None) => (),
                    other => break other,
                }
            }
        };
        match result {
            Ok(Some(quota)) => {
                self.quota_ledger.clear(attempt.kind)?;
                Ok((!attempt.credentials_exhausted).then_some(quota))
            }
            Ok(None) => {
                self.quota_ledger.put(attempt)?;
                Err(unsettled(op, None))
            }
            Err(error) => {
                self.quota_ledger.put(attempt)?;
                Err(unsettled(op, Some(error)))
            }
        }
    }

    async fn observe_attempt(
        &self,
        ctx: &RequestContext,
        attempt: &mut Attempt,
    ) -> Result<Option<Quota>, Error> {
        let kind = attempt.kind;
        let strict_absence = kind == Kind::Synthetic
            && matches!(
                attempt.goal,
                Goal::Delete { .. } | Goal::SyntheticCreate { .. }
            );
        let detail = |snapshot: &Snapshot| match &attempt.goal {
            Goal::RenewSetting { .. } => true,
            goal => goal.synthetic_candidate(snapshot),
        };
        let observe = |credentials: &crate::wire::product::evidence::SentCredentials| {
            self.quota_ledger.observe(kind, credentials)
        };
        let observed = self
            .observe_quota_matching(
                ctx,
                kind,
                attempt.op,
                &mut attempt.evidence,
                detail,
                (strict_absence, Some(&observe)),
            )
            .await;
        if let Some(retained) = self.quota_ledger.get(kind)? {
            attempt.merge_retained(&retained);
        }
        let observed = observed?;
        let Observation::Present(fresh) = observed else {
            if let Goal::Delete { before } = &attempt.goal {
                return Ok(Some(before.quota.clone()));
            }
            return Ok(None);
        };
        if !ready(&attempt.goal, &fresh) {
            return Ok(None);
        }
        if let Goal::RenewSetting { before, .. } = &attempt.goal {
            return Ok(Some(before.quota.clone()));
        }
        Ok(Some(fresh.quota))
    }

    pub(super) async fn observe_quota(
        &self,
        ctx: &RequestContext,
        kind: Kind,
        op: &'static str,
        evidence: &mut Evidence,
        detail: bool,
        strict_absence: bool,
    ) -> Result<Observation, Error> {
        self.observe_quota_matching(ctx, kind, op, evidence, |_| detail, (strict_absence, None))
            .await
    }
    async fn observe_quota_matching(
        &self,
        ctx: &RequestContext,
        kind: Kind,
        op: &'static str,
        evidence: &mut Evidence,
        detail: impl FnOnce(&Snapshot) -> bool,
        (strict_absence, observe): (
            bool,
            Option<crate::quote::request::monitor::CredentialObserver<'_>>,
        ),
    ) -> Result<Observation, Error> {
        let mut reader = Reader::new(self, ctx, op);
        reader.observe = observe;
        reader.read.family = Family::Quota {
            no_redirect: strict_absence && kind == Kind::Synthetic,
        };
        reader.evidence.absorb(evidence.credentials());
        let result = async {
            let response = reader.get_status(kind.current(), &[], strict_absence).await;
            let (status, raw) = match response {
                Ok(value) => value,
                Err(error)
                    if !strict_absence
                        && error.has_class(ErrorClass::NotFound)
                        && !error.is_body_too_large()
                        && error.detail::<BodyReadFailure>().is_none() =>
                {
                    return Ok(Observation::Absent(error));
                }
                Err(error) => return Err(error),
            };
            if status == StatusCode::NOT_FOUND {
                let absent = Object::of(&raw).is_some_and(|body| {
                    body.len() == 1
                        && body.text("message").is_some_and(|msg| {
                            msg == kind.absent() && !reader.evidence.text_reflects(&msg)
                        })
                });
                let error = reader.read.error(
                    Some(404),
                    "NotFound",
                    "quota response withheld",
                    false,
                    &reader.evidence,
                );
                return if absent {
                    Ok(Observation::Absent(error))
                } else {
                    Err(error)
                };
            }
            let quota = reader.decode(decode_quota(&raw))?;
            let mut snapshot = Snapshot {
                quota: quota.to_model(),
                details: None,
            };
            snapshot.screen_decisions(&reader)?;
            if kind == Kind::Synthetic && detail(&snapshot) {
                snapshot.details = Some(read_synthetic_detail(&quota, &mut reader).await?.into());
                snapshot.screen_decisions(&reader)?;
            }
            Ok(Observation::Present(snapshot))
        }
        .await;
        evidence.absorb(reader.evidence.credentials());
        result
    }
}

fn ready(goal: &Goal, fresh: &Snapshot) -> bool {
    match goal {
        Goal::Notification { package } => fresh.package() == package && fresh.status() == "ACTIVE",
        Goal::RenewSetting { before, enabled } => {
            fresh.id() == before.id()
                && fresh.package() == before.package()
                && field(fresh.quota.auto_renew) == Some(*enabled)
                && field(fresh.quota.auto_renew_period) == Some(i64::from(*enabled))
        }
        Goal::Delete { .. } => false,
        goal => goal.synthetic_candidate(fresh) && goal.synthetic_terms_match(fresh),
    }
}

impl Snapshot {
    fn screen_decisions(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let evidence = &reader.evidence;
        if evidence.id_reflects(self.id()) || evidence.id_reflects(self.package()) {
            return Err(reader.invalid());
        }
        let quota = &mut self.quota;
        for time in [
            &mut quota.start_time,
            &mut quota.end_time,
            &mut quota.created_at,
            &mut quota.updated_at,
        ] {
            if let crate::monitor::ResponseField::Value(time) = time
                && !time.redact(evidence)
            {
                return Err(reader.invalid());
            }
        }
        if !quota.status.redact(evidence)
            || !quota.auto_renew.redact(evidence)
            || !quota.poc.redact(evidence)
            || !quota.auto_renew_period.redact(evidence)
        {
            return Err(reader.invalid());
        }
        Ok(())
    }
}
