use super::redirect::Retry;
use crate::Secret;
use time::OffsetDateTime;
use tokio::time::Instant;

pub(crate) enum ObtainedAt {
    InProcess(Instant),
    Disk(OffsetDateTime),
}

pub(crate) struct Now {
    pub(crate) monotonic: Instant,
    pub(crate) wall: OffsetDateTime,
}

pub(crate) enum Auth<'a> {
    Bearer {
        sent: &'a Secret,
        obtained_at: ObtainedAt,
        built_in: bool,
    },
    Key,
    Unauthenticated,
}
pub(crate) struct Decision<'a> {
    pub(crate) rejected: Option<&'a Secret>,
    pub(crate) invalidate: Option<&'a Secret>,
    pub(crate) resend: bool,
}
pub(crate) fn decide(
    auth: Auth<'_>,
    retry: Retry,
    allows_recovery: bool,
    body_error: bool,
    recovered: bool,
    now: Now,
) -> Decision<'_> {
    let (sent, obtained_at, built_in) = match auth {
        Auth::Bearer {
            sent,
            obtained_at,
            built_in,
        } => (sent, obtained_at, built_in),
        Auth::Key => {
            return Decision {
                rejected: None,
                invalidate: None,
                resend: false,
            };
        }
        Auth::Unauthenticated => {
            return Decision {
                rejected: None,
                invalidate: None,
                resend: false,
            };
        }
    };
    if sent.expose_secret().is_empty() {
        return Decision {
            rejected: None,
            invalidate: None,
            resend: false,
        };
    }
    let old_enough = match obtained_at {
        ObtainedAt::Disk(time) => now.wall < time || now.wall - time >= time::Duration::seconds(30),
        ObtainedAt::InProcess(time) => now
            .monotonic
            .checked_duration_since(time)
            .is_none_or(|age| age >= std::time::Duration::from_secs(30)),
    };
    let rejected = if recovered && !body_error {
        None
    } else {
        Some(sent)
    };
    let invalidate = if rejected.is_some() && (!built_in || old_enough) {
        Some(sent)
    } else {
        None
    };
    let resend = allows_recovery && !matches!(retry, Retry::Once) && !body_error && !recovered;
    Decision {
        rejected,
        invalidate,
        resend,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    fn bearer(secret: &Secret, age: Duration, built_in: bool) -> Auth<'_> {
        Auth::Bearer {
            sent: secret,
            obtained_at: ObtainedAt::Disk(OffsetDateTime::UNIX_EPOCH - age),
            built_in,
        }
    }
    fn check(
        auth: Auth<'_>,
        retry: Retry,
        allow: bool,
        body: bool,
        recovered: bool,
    ) -> Decision<'_> {
        decide(
            auth,
            retry,
            allow,
            body,
            recovered,
            Now {
                wall: OffsetDateTime::UNIX_EPOCH,
                monotonic: Instant::now(),
            },
        )
    }
    #[test]
    fn wall_jump_does_not_evict_in_process_token() {
        let token = Secret::new("toy");
        let issued = Instant::now();
        let decision = decide(
            Auth::Bearer {
                sent: &token,
                obtained_at: ObtainedAt::InProcess(issued),
                built_in: true,
            },
            Retry::Once,
            true,
            false,
            false,
            Now {
                monotonic: issued + std::time::Duration::from_secs(5),
                wall: OffsetDateTime::UNIX_EPOCH + Duration::seconds(35),
            },
        );
        assert!(decision.invalidate.is_none());
        assert!(
            decision
                .rejected
                .is_some_and(|sent| std::ptr::eq(sent, &token))
        );
    }
    #[test]
    fn in_process_age_uses_monotonic_boundaries() {
        let token = Secret::new("toy");
        let base = Instant::now();
        let auth = |issued| Auth::Bearer {
            sent: &token,
            obtained_at: ObtainedAt::InProcess(issued),
            built_in: true,
        };
        let now = |monotonic| Now {
            monotonic,
            wall: OffsetDateTime::UNIX_EPOCH - Duration::days(1),
        };
        let young = decide(
            auth(base),
            Retry::Once,
            true,
            false,
            false,
            now(base + std::time::Duration::from_millis(29_999)),
        );
        assert!(young.invalidate.is_none());
        let old = decide(
            auth(base),
            Retry::Once,
            true,
            false,
            false,
            now(base + std::time::Duration::from_secs(30)),
        );
        assert!(old.invalidate.is_some());
        let backwards = decide(
            auth(base + std::time::Duration::from_secs(1)),
            Retry::Once,
            true,
            false,
            false,
            now(base),
        );
        assert!(backwards.invalidate.is_some());
    }
    #[test]
    fn young_once_reports_rejection_without_eviction() {
        let token = Secret::new("toy");
        let decision = check(
            bearer(&token, Duration::seconds(5), true),
            Retry::Once,
            true,
            false,
            false,
        );
        assert!(decision.invalidate.is_none());
        assert!(
            decision
                .rejected
                .is_some_and(|sent| std::ptr::eq(sent, &token))
        );
        assert!(!decision.resend);
    }
    #[test]
    fn young_second_body_error_reports_rejection() {
        let token = Secret::new("toy");
        let normal = check(
            bearer(&token, Duration::seconds(5), true),
            Retry::Repeatable,
            true,
            false,
            true,
        );
        assert!(normal.rejected.is_none());
        let body = check(
            bearer(&token, Duration::seconds(5), true),
            Retry::Repeatable,
            true,
            true,
            true,
        );
        assert!(body.invalidate.is_none());
        assert!(body.rejected.is_some_and(|sent| std::ptr::eq(sent, &token)));
        assert!(!body.resend);
    }
    #[test]
    fn nonidempotent_recovery() {
        let token = Secret::new("toy");
        let allowed = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::NonIdempotent,
            true,
            false,
            false,
        );
        assert!(allowed.resend);
        let disabled = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::NonIdempotent,
            false,
            false,
            false,
        );
        assert!(!disabled.resend);
    }
    #[test]
    fn zero_age_keeps_builtin_token() {
        let token = Secret::new("toy");
        let fresh = check(
            bearer(&token, Duration::ZERO, true),
            Retry::Repeatable,
            true,
            false,
            false,
        );
        assert!(fresh.invalidate.is_none());
        assert!(fresh.resend);
        let expired = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::Repeatable,
            true,
            false,
            false,
        );
        assert!(expired.invalidate.is_some());
    }
    #[test]
    fn bearer_exact() {
        let token = Secret::new("canary-one");
        let other = Secret::new("canary-two");
        let out = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::Repeatable,
            true,
            false,
            false,
        );
        assert!(out.invalidate.is_some_and(|v| std::ptr::eq(v, &token)));
        assert!(!out.invalidate.is_some_and(|v| std::ptr::eq(v, &other)));
        assert!(out.resend);
    }
    #[test]
    fn key_no_credentials() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(30), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .resend
        );
        let out = check(Auth::Key, Retry::Repeatable, true, false, false);
        assert!(out.rejected.is_none());
        assert!(out.invalidate.is_none());
        assert!(!out.resend);
    }
    #[test]
    fn unauthenticated_no_credentials() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(30), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .resend
        );
        let out = check(Auth::Unauthenticated, Retry::Repeatable, true, false, false);
        assert!(out.rejected.is_none());
        assert!(out.invalidate.is_none());
        assert!(!out.resend);
    }
    #[test]
    fn once_never_resends() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(30), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .resend
        );
        let out = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::Once,
            true,
            false,
            false,
        );
        assert!(out.invalidate.is_some());
        assert!(!out.resend);
    }
    #[test]
    fn disallowed_recovery() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(30), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .resend
        );
        let out = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::Repeatable,
            false,
            false,
            false,
        );
        assert!(out.invalidate.is_some());
        assert!(!out.resend);
    }
    #[test]
    fn body_never_resends() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(30), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .resend
        );
        let out = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::Repeatable,
            true,
            true,
            false,
        );
        assert!(out.invalidate.is_some());
        assert!(!out.resend);
    }
    #[test]
    fn only_one_recovery() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(30), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .resend
        );
        let out = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::Repeatable,
            true,
            false,
            true,
        );
        assert!(out.rejected.is_none());
        assert!(out.invalidate.is_none());
        assert!(!out.resend);
    }
    #[test]
    fn second_body_invalidates() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(30), true),
                Retry::Repeatable,
                true,
                false,
                true
            )
            .invalidate
            .is_none()
        );
        let out = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::Repeatable,
            true,
            true,
            true,
        );
        assert!(out.invalidate.is_some());
        assert!(!out.resend);
    }
    #[test]
    fn age_boundary() {
        let token = Secret::new("token");
        let young = check(
            bearer(&token, Duration::milliseconds(29999), true),
            Retry::Repeatable,
            true,
            false,
            false,
        );
        assert!(young.invalidate.is_none());
        assert!(young.resend);
        let old = check(
            bearer(&token, Duration::seconds(30), true),
            Retry::Repeatable,
            true,
            false,
            false,
        );
        assert!(old.invalidate.is_some());
        assert!(old.resend);
    }
    #[test]
    fn negative_age() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(1), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .invalidate
            .is_none()
        );
        assert!(
            check(
                bearer(&token, Duration::seconds(-1), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .invalidate
            .is_some()
        );
    }
    #[test]
    fn external_provider_owns_age_policy() {
        let token = Secret::new("token");
        assert!(
            check(
                bearer(&token, Duration::seconds(1), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .invalidate
            .is_none()
        );
        assert!(
            check(
                bearer(&token, Duration::seconds(1), false),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .invalidate
            .is_some()
        );
    }
    #[test]
    fn empty_bearer() {
        let good = Secret::new("token");
        let empty = Secret::new("");
        assert!(
            check(
                bearer(&good, Duration::seconds(30), true),
                Retry::Repeatable,
                true,
                false,
                false
            )
            .resend
        );
        let out = check(
            bearer(&empty, Duration::seconds(30), true),
            Retry::Repeatable,
            true,
            false,
            false,
        );
        assert!(out.rejected.is_none());
        assert!(out.invalidate.is_none());
        assert!(!out.resend);
    }
}
