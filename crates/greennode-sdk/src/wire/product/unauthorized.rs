use super::redirect::Retry;
use crate::Secret;
use time::OffsetDateTime;

pub(crate) enum Auth<'a> {
    Bearer {
        sent: &'a Secret,
        obtained_at: OffsetDateTime,
        built_in: bool,
    },
    Key,
    Unauthenticated,
}
pub(crate) struct Decision<'a> {
    pub(crate) invalidate: Option<&'a Secret>,
    pub(crate) resend: bool,
}
pub(crate) fn decide(
    auth: Auth<'_>,
    retry: Retry,
    allows_recovery: bool,
    body_error: bool,
    recovered: bool,
    now: OffsetDateTime,
) -> Decision<'_> {
    let (sent, obtained_at, built_in) = match auth {
        Auth::Bearer {
            sent,
            obtained_at,
            built_in,
        } => (sent, obtained_at, built_in),
        Auth::Key => {
            return Decision {
                invalidate: None,
                resend: false,
            };
        }
        Auth::Unauthenticated => {
            return Decision {
                invalidate: None,
                resend: false,
            };
        }
    };
    if sent.expose_secret().is_empty() {
        return Decision {
            invalidate: None,
            resend: false,
        };
    }
    let age = now - obtained_at;
    let invalidate = if recovered && !body_error {
        None
    } else if !built_in || age.is_negative() || age >= time::Duration::seconds(30) {
        Some(sent)
    } else {
        None
    };
    let resend = allows_recovery && !matches!(retry, Retry::Once) && !body_error && !recovered;
    Decision { invalidate, resend }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    fn bearer(secret: &Secret, age: Duration, built_in: bool) -> Auth<'_> {
        Auth::Bearer {
            sent: secret,
            obtained_at: OffsetDateTime::UNIX_EPOCH - age,
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
            OffsetDateTime::UNIX_EPOCH,
        )
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
        assert!(out.invalidate.is_none());
        assert!(!out.resend);
    }
}
