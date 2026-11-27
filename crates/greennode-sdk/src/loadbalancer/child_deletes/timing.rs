use super::*;

pub(super) fn check_ready_time(
    parent: &RequestContext,
    wait: &RequestContext,
    clock: &dyn crate::Clock,
    child: &Child<'_>,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    if let Some(end) = parent.check(clock) {
        return Err(Box::new(context_error(end)));
    }
    if wait.check(clock).is_some() {
        let lb = safe_text(child.load_balancer, evidence);
        let id = safe_text(child.id, evidence);
        let message = format!(
            "loadbalancer: resource busy: {}: load balancer {lb} or {} {id} is not ready within 10m0s; nothing sent",
            child.operation, child.kind
        );
        return Err(Box::new(
            local(ErrorClass::Api, &message, evidence).with_outer_detail(WriteError::ResourceBusy),
        ));
    }
    Ok(())
}

pub(super) fn check_confirm_time(
    parent: &RequestContext,
    wait: &RequestContext,
    clock: &dyn crate::Clock,
    child: &Child<'_>,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    let Some(end) = wait.check(clock) else {
        return Ok(());
    };
    let reason = match end {
        ContextEnd::Cancelled => "confirmation cancelled by caller",
        ContextEnd::DeadlineExceeded if parent.deadline() == wait.deadline() => {
            "confirmation stopped at the caller's deadline"
        }
        ContextEnd::DeadlineExceeded => "did not settle within 10m0s",
    };
    let id = safe_text(child.id, evidence);
    let message = format!(
        "vngcloud: write outcome not confirmed: {}: {} {id} {reason}; delete was sent and a rerun is safe",
        child.operation, child.kind
    );
    let error = local(ErrorClass::NotSettled, &message, evidence);
    let error = if let Some(end) = parent.check(clock) {
        error.with_cause(context_error(end))
    } else {
        error
    };
    Err(Box::new(error))
}
