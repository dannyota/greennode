use super::{Error, Failure};

/// A reconciliation step for a write that may have left a resource to inspect.
#[derive(Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum RecoveryAction {
    /// Check for a volume with the requested name before placing another order.
    /// CreateVolume attaches this action to uncertain dispatch, unusable acknowledgements,
    /// and confirmation failures when the separate no-repeat action does not apply.
    CheckExistingVolume,
    /// Keep the acknowledged volume. CreateVolume attaches this action when its settlement bound expires.
    DoNotRepeatVolumeCreate,
}

crate::error::debug_record!(RecoveryAction; CheckExistingVolume, DoNotRepeatVolumeCreate);

impl RecoveryAction {
    fn messages(self) -> [Option<&'static str>; 2] {
        match self {
            Self::CheckExistingVolume => [
                Some("Check for an existing volume with the exact name before retrying."),
                None,
            ],
            Self::DoNotRepeatVolumeCreate => [
                Some("The volume exists and this create must not be repeated."),
                Some("Creation was accepted. Do not retry."),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RecoveryGuidance {
    action: RecoveryAction,
    messages: [Option<&'static str>; 2],
}

impl RecoveryGuidance {
    fn screen(&mut self, reflects: &impl Fn(&str) -> bool) {
        for message in &mut self.messages {
            *message = message.filter(|text| !reflects(text));
        }
    }
}

impl Error {
    /// Returns the reconciliation step even if its diagnostic text was withheld.
    pub fn recovery_action(&self) -> Option<RecoveryAction> {
        self.internal_detail::<RecoveryGuidance>()
            .map(|guidance| guidance.action)
    }

    /// Returns `None` when no recovery action is attached. `Some("")` means
    /// the action is available through [`Error::recovery_action`] and its fixed text was withheld.
    pub fn recovery_diagnostic(&self) -> Option<&'static str> {
        self.internal_detail::<RecoveryGuidance>().map(|guidance| {
            guidance
                .messages
                .into_iter()
                .flatten()
                .next()
                .unwrap_or_default()
        })
    }

    pub(crate) fn with_recovery_action(
        self,
        action: RecoveryAction,
        reflects: impl Fn(&str) -> bool,
    ) -> Self {
        let mut guidance = RecoveryGuidance {
            action,
            messages: action.messages(),
        };
        guidance.screen(&reflects);
        self.with_outer_detail(guidance)
    }

    pub(super) fn screen_recovery(&mut self, reflects: &impl Fn(&str) -> bool) {
        if let Some(guidance) = self
            .detail
            .as_mut()
            .and_then(|detail| detail.value_mut().downcast_mut::<RecoveryGuidance>())
        {
            guidance.screen(reflects);
        }
    }
}

impl<T> Failure<T> {
    /// Returns the reconciliation step attached to this failure's error.
    /// A write can have reached the service without carrying a recovery action.
    pub fn recovery_action(&self) -> Option<RecoveryAction> {
        self.error().recovery_action()
    }
}
