use super::state::{Goal, Snapshot};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    None,
    Renew,
    Recover,
}

impl Action {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Renew => "renew",
            Self::Recover => "recover",
        }
    }
}

impl Goal {
    pub(super) fn synthetic_candidate(&self, fresh: &Snapshot) -> bool {
        if fresh.status() != "ACTIVE" {
            return false;
        }
        match self {
            Self::SyntheticCreate {
                package, sent_at, ..
            } => {
                fresh.package() == package
                    && fresh
                        .start()
                        .zip(fresh.end())
                        .is_some_and(|(start, end)| start >= sent_at.0 && end > start)
            }
            Self::SyntheticChange { before, .. } => {
                fresh.id() == before.id()
                    && fresh.package() == before.package()
                    && fresh
                        .end()
                        .zip(before.end())
                        .is_some_and(|(new, old)| new > old)
            }
            _ => false,
        }
    }

    pub(super) fn synthetic_terms_match(&self, fresh: &Snapshot) -> bool {
        let Some(detail) = &fresh.details else {
            return false;
        };
        if detail.price != Some(0.0) {
            return false;
        }
        match self {
            Self::SyntheticCreate {
                class_id,
                quantity,
                location,
                ..
            } => {
                detail.class_id == *class_id
                    && detail.api == *quantity
                    && detail.location == *location
            }
            Self::SyntheticChange { before, .. } => {
                before.details.as_ref().is_some_and(|previous| {
                    detail.class_id == previous.class_id
                        && detail.api == previous.api
                        && detail.location == previous.location
                })
            }
            _ => false,
        }
    }
}
