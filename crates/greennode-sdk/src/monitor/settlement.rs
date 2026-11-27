use super::mutation::Mutation;
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Clock, Error, ErrorClass, PagedList, RequestContext};
use std::{collections::HashSet, time::Duration};

pub(super) struct Wait<'a> {
    context: RequestContext,
    clock: &'a dyn Clock,
    interval: Duration,
    first: bool,
}
impl<'a> Wait<'a> {
    pub(super) fn new(
        parent: &RequestContext,
        clock: &'a dyn Clock,
        interval: Duration,
        bound: Duration,
    ) -> Self {
        Self {
            context: parent.child_with_deadline(clock.monotonic_now() + bound),
            clock,
            interval,
            first: true,
        }
    }
    pub(super) async fn next(&mut self) -> Result<&RequestContext, Error> {
        self.check()?;
        if self.first {
            self.first = false;
        } else {
            self.context
                .child_with_deadline(self.clock.monotonic_now() + self.interval)
                .wait(self.clock)
                .await;
        }
        self.check()?;
        Ok(&self.context)
    }
    pub(super) fn check(&self) -> Result<(), Error> {
        self.context
            .check(self.clock)
            .map_or(Ok(()), |end| Err(end.into()))
    }
}

pub(super) const NAME_PAGE_CAP: i64 = 1000;
pub(super) struct Names<T> {
    first: i64,
    meta: Option<(i64, i64, i64)>,
    seen: HashSet<String>,
    matched: Option<T>,
}
impl<T> Names<T> {
    pub(super) fn new(first: i64) -> Self {
        Self {
            first,
            meta: None,
            seen: HashSet::new(),
            matched: None,
        }
    }
    pub(super) fn page(
        &mut self,
        page: i64,
        body: PagedList<T>,
        target: &str,
        identity: impl Fn(&T) -> (&str, &str),
        op: &Mutation,
        evidence: &Evidence,
    ) -> Result<bool, Error> {
        let invalid = || name_failure(op, evidence);
        let meta = (body.total_item, body.total_page, body.page_size);
        let count = i64::try_from(body.items.len()).map_err(|_| invalid())?;
        if body.page != page
            || body.page_size <= 0
            || body.total_item < 0
            || !(0..=NAME_PAGE_CAP).contains(&body.total_page)
            || count > body.page_size
            || self.meta.is_some_and(|actual| actual != meta)
        {
            return Err(invalid());
        }
        self.meta = Some(meta);
        for item in body.items {
            let (id, name) = identity(&item);
            if !PathIds::WithUnderscore.allows(id)
                || name.is_empty()
                || evidence.id_reflects(id)
                || evidence.text_reflects(name)
                || !self.seen.insert(id.into())
            {
                return Err(invalid());
            }
            if name == target && self.matched.replace(item).is_some() {
                return Err(invalid());
            }
        }
        if body.total_item == 0 {
            return if self.seen.is_empty() && body.total_page <= 1 {
                Ok(true)
            } else {
                Err(invalid())
            };
        }
        let seen = i64::try_from(self.seen.len()).map_err(|_| invalid())?;
        if body.total_page == 0 || count == 0 || seen > body.total_item {
            return Err(invalid());
        }
        if page - self.first + 1 == body.total_page {
            if seen != body.total_item || self.seen.iter().any(|id| evidence.id_reflects(id)) {
                return Err(invalid());
            }
            return Ok(true);
        }
        Ok(false)
    }
    pub(super) fn result(self) -> Option<T> {
        self.matched
    }
}

pub(super) fn name_failure(mutation: &Mutation, evidence: &Evidence) -> Error {
    mutation.error(
        Some(200),
        "InvalidResponse",
        "name search was incomplete or ambiguous",
        evidence,
    )
}
pub(super) fn uncertain(mutation: &Mutation, label: &str, cause: Error) -> Error {
    if cause.has_class(ErrorClass::Failed) {
        return cause;
    }
    Error::new(ErrorClass::NotSettled)
        .with_outer_detail(format!(
            "{}: {label}; do not send the same write again",
            mutation.id
        ))
        .with_cause(cause)
}
