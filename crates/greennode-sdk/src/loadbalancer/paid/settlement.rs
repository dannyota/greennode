use super::*;

impl Client {
    pub(super) async fn settle(
        &self,
        ctx: &RequestContext,
        evidence: &mut Evidence,
        target: Target<'_>,
        id: &str,
        mut last: LoadBalancer,
        acknowledgement: Option<&crate::wire::product::guard::AcknowledgedId>,
    ) -> (LoadBalancer, Option<Error>) {
        let clock = self.session.clock();
        let (minutes, wanted, warning) = target.settlement();
        let wait =
            ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(minutes * 60));
        let op = target.operation();
        let package_requirement = match target {
            Target::Create => PackageRequirement::Optional,
            Target::Resize(_) => PackageRequirement::UnlessFailed,
        };
        let why = format!("vngcloud: write outcome not confirmed: {op}: load balancer {id}");
        let error = guarded_poll!(evidence, {
            if let Some(end) = wait.check(clock) {
                let msg = format!("{why} did not reach {wanted} within {minutes}m0s; {warning}");
                break outcome(ErrorClass::NotSettled, &msg, evidence).with_cause(Error::from(end));
            }
            let recheck = |evidence: &Evidence| {
                acknowledgement.map_or(Ok(()), |receipt| receipt.recheck(evidence).map(drop))
            };
            if let Err(error) = recheck(evidence) {
                break outcome(
                    ErrorClass::NotSettled,
                    &format!("{why}; {warning}"),
                    evidence,
                )
                .with_cause(*error);
            }
            let read = self
                .read_state(&wait, id, evidence, package_requirement)
                .await;
            match recheck(evidence)
                .and(read)
                .and_then(|row| row.into_checked(evidence))
            {
                Ok(row) => {
                    last = row;
                    if wait.check(clock).is_none() {
                        if last.progress_status == "ERROR" {
                            break outcome(
                                ErrorClass::Failed,
                                &format!(
                                    "vngcloud: write failed: {op}: load balancer {id} is ERROR"
                                ),
                                evidence,
                            );
                        }
                        if last.progress_status == "CREATED" && target.matches(&last) {
                            return (last, None);
                        }
                    }
                }
                Err(error)
                    if matches!(target, Target::Create)
                        && crate::confirms_absence(&error, GET.id, "GET", GET.route) => {}
                Err(error) => {
                    break outcome(
                        ErrorClass::NotSettled,
                        &format!("{why}; {warning}"),
                        evidence,
                    )
                    .with_cause(*error);
                }
            }
            wait.child_with_deadline(clock.monotonic_now() + POLL)
                .wait(clock)
                .await;
        });
        (last, Some(error))
    }
}
