use std::collections::BTreeMap;

use serde::{Serialize, Serializer, ser::SerializeStruct};
use serde_json::{Value, value::RawValue};

use super::wire::{self, REDACTED, Screen, ScreenId, Wire};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, ShapeField};

macro_rules! decode {
    ($raw:expr, $ty:ty) => {
        <$ty as Wire>::decode($raw)
    };
    ($raw:expr, $ty:ty, $decoder:path) => {
        $decoder($raw)
    };
}

macro_rules! projected {
    ($value:expr) => {
        $value
    };
    ($value:expr, $projection:path) => {
        &$projection($value)
    };
}

macro_rules! screen_field {
    ($value:expr, $evidence:expr) => {
        $value.screen($evidence)
    };
    ($value:expr, $evidence:expr, $method:ident) => {
        $value.$method($evidence)
    };
}

macro_rules! record {
    ($(#[$meta:meta])* $name:ident $(<$item:ident>)? {
        $($(#[$field_meta:meta])* $(@ $screen:ident)? $field:ident: $ty:ty => $wire:literal, $go:literal $(, $decoder:path)? $(=> $projection:path)?;) +
    }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct $name $(<$item>)? {
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )+
            masked: Vec<&'static str>,
        }

        impl $(<$item: Wire>)? Wire for $name $(<$item>)? {
            const SHAPE: Shape<'static> = Shape::Object(&[
                $(ShapeField { name: $wire, shape: <$ty as Wire>::SHAPE },)+
            ]);

            fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
                let fields: Option<BTreeMap<String, &RawValue>> = serde_json::from_str(raw.get())?;
                let Some(fields) = fields else { return Ok(Self::default()); };
                Ok(Self {
                    $($field: wire::field(&fields, $wire)
                        .map_or_else(|| Ok(<$ty>::default()), |raw| decode!(raw, $ty $(, $decoder)?))?,)+
                    masked: Vec::new(),
                })
            }
        }

        impl $(<$item: Screen>)? Screen for $name $(<$item>)? {
            fn screen(&mut self, evidence: &Evidence) -> bool {
                $(if screen_field!(self.$field, evidence $(, $screen)?) { self.masked.push($go); })+
                false
            }
        }

        impl $(<$item: crate::wire::product::output::PublicOutput>)? crate::wire::product::output::PublicOutput for $name $(<$item>)? {
            fn public_reflects(&self, scan: &crate::wire::product::output::FieldScan<'_>, identifier: bool) -> bool {
                $(scan.field(&self.$field, identifier, $wire, $go) ||)+ false
            }
        }

        impl $(<$item: Serialize>)? Serialize for $name $(<$item>)? {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut output = serializer.serialize_struct(stringify!($name), [$($go),+].len())?;
                $(
                    if self.masked.contains(&$go) {
                        if crate::wire::product::identifier::declared($wire, $go) {
                            crate::wire::product::identifier::check::<_, S::Error>(REDACTED)?;
                        }
                        output.serialize_field($go, REDACTED)?;
                    } else {
                        let value = projected!(&self.$field $(, $projection)?);
                        if crate::wire::product::identifier::declared($wire, $go) {
                            crate::wire::product::identifier::check::<_, S::Error>(value)?;
                        }
                        output.serialize_field($go, value)?;
                    }
                )+
                output.end()
            }
        }
    };
}

record! {
    /// One zero-based page from the IAM accounts or policies API.
    Page<T> {
        /// Entries in this page.
        items: Vec<T> => "data", "Items";
        /// Zero-based page number, when reported.
        page_number: Option<i64> => "pageNumber", "PageNumber";
        /// Requested page size, when reported.
        page_size: Option<i64> => "pageSize", "PageSize";
        /// Total entries, when reported.
        total_items: Option<i64> => "totalItems", "TotalItems";
        /// Total pages, when reported.
        total_pages: Option<i64> => "totalPages", "TotalPages";
    }
}

record! {
    /// Identity associated with the current sign-in.
    GetCallerIdentityOutput {
        /// Identifier of the caller.
        @ screen_id user_id: String => "userId", "UserID";
        /// IAM principal type.
        user_type: String => "userType", "UserType";
        /// Account name supplied by the IAM API.
        username: String => "username", "Username";
        /// Owning account number.
        @ screen_id account_id: i64 => "accountId", "AccountID";
    }
}

record! {
    /// IAM user summary from the accounts API.
    User {
        /// User identifier.
        @ screen_id id: String => "id", "ID";
        /// User name.
        username: String => "username", "Username";
        /// Creation time in the API's RFC 3339 string form.
        created_at: String => "createdAt", "CreatedAt";
    }
}

record! {
    /// Enrollment status for an MFA factor, with unknown states left absent.
    MfaFactor {
        /// Whether the factor was verified.
        verified: Option<bool> => "verified", "Verified";
        /// Whether the factor is enabled.
        enabled: Option<bool> => "enabled", "Enabled";
    }
}

record! {
    /// The factor states exposed by a user read.
    UserMfa {
        /// Email factor status, when reported.
        email: Option<MfaFactor> => "email", "Email";
        /// Authenticator factor status, when reported.
        google_authenticator: Option<MfaFactor> => "googleAuthenticator", "GoogleAuthenticator";
    }
}

record! {
    /// User details and the states of supported MFA factors.
    UserDetail {
        /// User identifier.
        @ screen_id id: String => "id", "ID";
        /// User name.
        username: String => "username", "Username";
        /// Creation time in epoch milliseconds.
        created_at: i64 => "createdAt", "CreatedAt", wire::epoch;
        /// Whether the user can sign in.
        enabled: bool => "enabled", "Enabled";
        /// Source of the user record.
        from: String => "from", "From";
        /// Most recent use in epoch milliseconds, or zero if unreported.
        last_use: i64 => "lastUse", "LastUse", wire::epoch;
        /// MFA state without contact or enrollment material.
        mfa: Option<UserMfa> => "twoFactorAuth", "MFA";
    }
}

record! {
    /// Account identifier and metadata from an IAM service-account read.
    ServiceAccount {
        /// Service-account resource identifier.
        @ screen_id id: String => "id", "ID";
        /// OAuth client identifier.
        @ screen_id client_id: String => "clientId", "ClientID";
        /// Display name.
        name: String => "name", "Name";
        /// Service-account description.
        description: String => "description", "Description";
        /// Access-token lifetime in seconds.
        access_token_life_span: i64 => "accessTokenLifeSpan", "AccessTokenLifeSpan";
        /// Creation time in epoch milliseconds.
        created_at: i64 => "createdAt", "CreatedAt", wire::epoch;
        /// Whether the account is enabled.
        enabled: bool => "enabled", "Enabled";
        /// Most recent use in epoch milliseconds, or zero if unreported.
        last_use: i64 => "lastUse", "LastUse", wire::epoch;
    }
}

record! {
    /// IAM action and the resource forms it accepts.
    Action {
        /// Action name.
        action: String => "action", "Action";
        /// Action category supplied by IAM.
        label: String => "label", "Label";
        /// Resource forms associated with the action.
        resources: Vec<String> => "resources", "Resources";
    }
}

record! {
    /// Entry from an IAM policy list.
    PolicySummary {
        /// Policy identifier.
        @ screen_id id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Creation time in epoch milliseconds.
        created_at: i64 => "createdAt", "CreatedAt", wire::epoch;
    }
}

record! {
    /// An allow or deny statement in an IAM policy.
    Statement {
        /// Effect supplied by the policy.
        effect: String => "effect", "Effect";
        /// Action patterns covered by the statement.
        actions: Vec<String> => "actions", "Actions";
        /// Resource patterns covered by the statement.
        resources: Vec<String> => "resources", "Resources";
        /// Conditions keyed by the API's operator names.
        condition: Option<BTreeMap<String, Value>> => "condition", "Condition", wire::condition => wire::condition_output;
    }
}

record! {
    /// Full IAM policy and its statements.
    Policy {
        /// Policy identifier.
        @ screen_id id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Policy description.
        description: String => "description", "Description";
        /// Manager of the policy.
        manager: String => "manager", "Manager";
        /// Scope of the policy.
        scope: String => "scope", "Scope";
        /// Owning account number as text, or empty when unreported.
        @ screen_id root: String => "root", "Root", wire::root;
        /// Rules in policy order.
        statements: Vec<Statement> => "statements", "Statements";
        /// Creation time in epoch milliseconds.
        created_at: i64 => "createdAt", "CreatedAt", wire::epoch;
    }
}

impl Policy {
    /// Returns whether the policy is managed by GreenNode.
    pub fn managed(&self) -> bool {
        self.manager != "user"
    }
}

record! {
    /// Entry from an IAM group list.
    GroupSummary {
        /// Group identifier.
        @ screen_id id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Creation time in epoch milliseconds.
        created_at: i64 => "createdAt", "CreatedAt", wire::epoch;
    }
}

record! {
    /// IAM group with its members and attached policies.
    Group {
        /// Group identifier.
        @ screen_id id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Group description.
        description: String => "description", "Description";
        /// Membership mode supplied by IAM.
        mode: String => "mode", "Mode";
        /// Owning account number as text, or empty when unreported.
        @ screen_id root: String => "root", "Root", wire::root;
        /// IDs of the group's users.
        @ screen_id user_ids: Vec<String> => "iamUsers", "UserIDs";
        /// IDs of the group's policies.
        @ screen_id policy_ids: Vec<String> => "policies", "PolicyIDs";
        /// Creation time in epoch milliseconds.
        created_at: i64 => "createdAt", "CreatedAt", wire::epoch;
    }
}
