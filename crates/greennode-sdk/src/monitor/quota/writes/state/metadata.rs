use crate::model::wire::WireField;
use crate::wire::product::evidence::Evidence;

#[derive(Clone, Copy, Default)]
pub(in crate::monitor::quota::writes) struct RetainedRedaction {
    statuses: [u64; 16],
}

impl RetainedRedaction {
    pub(super) fn observe(&mut self, dropped: &Evidence) -> bool {
        for status in std::iter::once(0).chain(100..=999) {
            if !i64::from(status).redact(dropped) {
                self.statuses[status as usize / 64] |= 1 << (status % 64);
            }
        }
        ![
            "yes",
            "unknown",
            "unconfirmed",
            "confirmed",
            "sent",
            "settled",
            "true",
            "false",
            "none",
            "renew",
            "recover",
            "NotSettled",
            "<redacted>",
            "monitor.CreateSyntheticQuota",
            "monitor.RenewSyntheticQuota",
            "monitor.RecoverSyntheticQuota",
            "monitor.EnsureSyntheticQuota",
            "monitor.SetSyntheticQuotaAutoRenew",
            "monitor.DeleteSyntheticQuota",
        ]
        .into_iter()
        .any(|text| dropped.text_reflects(text))
    }

    pub(super) fn merge(&mut self, other: &Self) {
        for (current, previous) in self.statuses.iter_mut().zip(other.statuses) {
            *current |= previous;
        }
    }

    pub(in crate::monitor::quota::writes) fn hides_status(&self, status: i64) -> bool {
        let Ok(status) = usize::try_from(status) else {
            return true;
        };
        self.statuses
            .get(status / 64)
            .is_some_and(|bits| bits & (1 << (status % 64)) != 0)
    }
}
