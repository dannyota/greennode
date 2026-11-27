use std::borrow::Cow;

use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::Error;
use crate::monitor::QuotaTimestamp;
use crate::monitor::wire::{check_path_id, requires_failure};
use crate::wire::product::evidence::Evidence;

use super::super::lifecycle::unexpected;
use super::super::state::{Attempt, Goal, Snapshot, field, invalid, validate_cap};
use super::{Action, changed, quota_invalid};

const MAX_DAYS: i64 = i64::MAX / 86_400_000_000_000;

#[derive(Clone, Copy)]
pub(super) enum Intent<'a> {
    Create {
        class: &'a str,
        package: &'a str,
        quantity: i64,
    },
    Renew {
        end: &'a str,
    },
    Recover {
        id: &'a str,
    },
    Ensure {
        id: &'a str,
    },
}
pub(super) struct Spec<'a> {
    pub(super) op: &'static str,
    pub(super) intent: Intent<'a>,
    pub(super) cap: f64,
    pub(super) days: i64,
    pub(super) no_wait: bool,
}
impl Spec<'_> {
    pub(super) fn validate(&self) -> Result<Option<OffsetDateTime>, Error> {
        let end = match self.intent {
            Intent::Create {
                class,
                package,
                quantity,
            } => {
                if class.is_empty() {
                    return Err(requires_failure::<()>(self.op, "ClassID").into_parts().0);
                }
                if package.is_empty() {
                    return Err(requires_failure::<()>(self.op, "PackageID").into_parts().0);
                }
                if quantity == 0 {
                    return Err(requires_failure::<()>(self.op, "Quantity").into_parts().0);
                }
                check_path_id::<()>(self.op, "ClassID", class)
                    .map_err(|failure| failure.into_parts().0)?;
                if quantity < 0 {
                    return Err(invalid(self.op, "Quantity must be positive"));
                }
                None
            }
            Intent::Renew { end } => Some(
                parse_expiry(end)
                    .ok_or_else(|| invalid(self.op, "ExpectedEndTime must be RFC 3339"))?,
            ),
            Intent::Recover { id } => {
                if id.is_empty() {
                    return Err(invalid(self.op, "ExpectedQuotaID is required"));
                }
                check_path_id::<()>(self.op, "ExpectedQuotaID", id)
                    .map_err(|failure| failure.into_parts().0)?;
                None
            }
            Intent::Ensure { id } => {
                if !id.is_empty() {
                    check_path_id::<()>(self.op, "ExpectedQuotaID", id)
                        .map_err(|failure| failure.into_parts().0)?;
                }
                None
            }
        };
        if !(0..=MAX_DAYS).contains(&self.days) {
            return Err(invalid(self.op, "MaxDaysLeft is out of range"));
        }
        validate_cap(self.op, self.cap)?;
        Ok(end)
    }
    pub(super) fn action(&self, before: &Snapshot, now: OffsetDateTime) -> Result<Action, Error> {
        match self.intent {
            Intent::Renew { .. } => Ok(Action::Renew),
            Intent::Recover { .. } => Ok(Action::Recover),
            Intent::Ensure { id } => {
                if !id.is_empty() && id != before.id() {
                    return Err(changed(self.op));
                }
                match before.status() {
                    "ACTIVE" if before.end().is_some_and(|end| end > now) => Ok(Action::Renew),
                    "IN-TRASH" if before.end().is_some_and(|end| end < now) => Ok(Action::Recover),
                    "ACTIVE" | "IN-TRASH" | "CREATING" | "ERROR" => {
                        Err(invalid(self.op, "quota is ineligible"))
                    }
                    _ => Err(unexpected(self.op)),
                }
            }
            Intent::Create { .. } => Err(invalid(self.op, "quota action is unsupported")),
        }
    }
    pub(super) fn eligible(
        &self,
        before: &Snapshot,
        expected_end: Option<OffsetDateTime>,
        action: Action,
        now: OffsetDateTime,
    ) -> Result<(), Error> {
        let mismatch = match self.intent {
            Intent::Renew { .. } => before.end() != expected_end,
            Intent::Recover { id } => before.id() != id,
            Intent::Ensure { id } => !id.is_empty() && before.id() != id,
            Intent::Create { .. } => false,
        };
        if mismatch {
            return Err(changed(self.op));
        }
        let status = match action {
            Action::Renew => "ACTIVE",
            Action::Recover => "IN-TRASH",
            Action::None => "",
        };
        if before.status() != status {
            return match before.status() {
                "ACTIVE" | "IN-TRASH" | "CREATING" | "ERROR" => {
                    Err(invalid(self.op, "quota is ineligible"))
                }
                _ => Err(unexpected(self.op)),
            };
        }
        let in_window = before.end().is_some_and(|end| {
            end != QuotaTimestamp::default().0
                && if action == Action::Recover {
                    end < now
                } else {
                    end > now
                }
        });
        if !in_window
            || before.id().is_empty()
            || before.package().is_empty()
            || before
                .start()
                .is_none_or(|time| time == QuotaTimestamp::default().0)
            || field(before.quota.poc) != Some(false)
            || field(before.quota.auto_renew) != Some(false)
        {
            return Err(invalid(self.op, "quota is ineligible"));
        }
        if before
            .details
            .as_ref()
            .is_none_or(|details| details.price.is_none())
        {
            return Err(quota_invalid(self.op, &Evidence::new()));
        }
        Ok(())
    }
    pub(super) fn guard_mismatch(&self, goal: &Goal, expected_end: Option<OffsetDateTime>) -> bool {
        match (&self.intent, goal) {
            (Intent::Renew { .. }, Goal::SyntheticChange { before, .. }) => {
                before.end() != expected_end
            }
            (Intent::Renew { .. }, _) => true,
            (Intent::Recover { id }, _) => goal.id() != *id,
            (Intent::Ensure { id }, _) => !id.is_empty() && goal.id() != *id,
            (Intent::Create { .. }, _) => false,
        }
    }
    pub(super) fn matches(&self, attempt: &Attempt, expected_end: Option<OffsetDateTime>) -> bool {
        if self.op != attempt.op || self.guard_mismatch(&attempt.goal, expected_end) {
            return false;
        }
        match (&self.intent, &attempt.goal) {
            (
                Intent::Create {
                    class,
                    package,
                    quantity,
                },
                Goal::SyntheticCreate {
                    class_id,
                    package: id,
                    quantity: q,
                    ..
                },
            ) => *class == class_id && *package == id && quantity == q,
            (Intent::Create { .. }, _) => false,
            (_, Goal::SyntheticChange { .. }) => true,
            _ => false,
        }
    }
}

fn parse_expiry(text: &str) -> Option<OffsetDateTime> {
    let day_and_hour = text.as_bytes().get(10..13)?;
    if day_and_hour[0] != b'T' || text.ends_with('z') {
        return None;
    }
    let adjusted = if day_and_hour[2] == b':' {
        Cow::Owned(format!("{}0{}", &text[..11], &text[11..]))
    } else {
        Cow::Borrowed(text)
    };
    let adjusted = adjusted.replace(',', ".");
    if adjusted.as_bytes().get(17) == Some(&b'6') {
        return None;
    }
    let value = OffsetDateTime::parse(&adjusted, &Rfc3339)
        .ok()
        .or_else(|| {
            let (head, zone) = adjusted.split_at_checked(adjusted.len().checked_sub(6)?)?;
            let sign = match zone.as_bytes() {
                [b'+', _, _, b':', _, _] => 1,
                [b'-', _, _, b':', _, _] => -1,
                _ => return None,
            };
            if !zone.as_bytes()[1..3]
                .iter()
                .chain(&zone.as_bytes()[4..6])
                .all(u8::is_ascii_digit)
            {
                return None;
            }
            let hour: i64 = zone.get(1..3)?.parse().ok()?;
            let minute: i64 = zone.get(4..6)?.parse().ok()?;
            if !(0..=24).contains(&hour) || !(0..=60).contains(&minute) {
                return None;
            }
            OffsetDateTime::parse(&format!("{head}Z"), &Rfc3339)
                .ok()?
                .checked_sub(Duration::minutes(sign * (60 * hour + minute)))
        })?;
    (value != QuotaTimestamp::default().0).then_some(value)
}
