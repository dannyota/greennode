use super::Response;
use crate::wire::product::{absence, guard, operation, redirect, response};
use bytes::Bytes;

type Classifier = fn(
    &'static str,
    http::StatusCode,
    Bytes,
    &[http::StatusCode],
    (bool, bool, redirect::Retry),
    &super::SentCredentials,
) -> Result<Bytes, Box<crate::Error>>;

impl Response {
    pub(super) fn from_exchange(
        reply: crate::wire::send::Reply,
        reflection: response::Screen,
        credentials: super::SentCredentials,
        op: &super::Operation<'_>,
        requested: &url::Url,
        answered: &url::Url,
        inputs: super::RouteInputs<'_>,
    ) -> Self {
        use guard::ReceiptPolicy;
        let (receipt_policy, receipt_screen) = if op.guard.is_some() {
            (ReceiptPolicy::Guard, reflection)
        } else if matches!(inputs.receipt_policy, ReceiptPolicy::Transport) {
            let evidence = crate::wire::product::evidence::Evidence::from_credentials(
                &credentials.transport_only(),
            );
            (
                ReceiptPolicy::Transport,
                evidence.go_peer_body_screen(&reply.body),
            )
        } else {
            (inputs.receipt_policy, reflection)
        };
        Self {
            status: reply.status,
            headers: reply.headers,
            body: zeroize::Zeroizing::new(reply.body),
            body_cap: op.cap,
            credentials,
            reflection,
            receipt_screen,
            receipt_policy,
            project_output: inputs.project_output
                && (inputs.redact_output || reflection.is_clean()),
            write_response: matches!(op.class, redirect::Classification::Write),
            transport_screen: std::sync::OnceLock::new(),
            request: inputs.descriptor.map(|descriptor| {
                absence::RequestIdentity::new(descriptor, &op.method, requested, answered)
            }),
        }
    }

    pub(crate) fn clean_receipt(&self) -> Option<guard::CleanReceipt<'_>> {
        let request = self.request.as_ref()?.direct()?;
        Some(guard::CleanReceipt::new(
            &self.body,
            self.receipt_screen.clean()?,
            request,
            self.receipt_policy,
        ))
    }

    pub(crate) fn decode_projection<T>(
        &self,
        decode: impl FnOnce(&[u8]) -> Result<T, Box<crate::Error>>,
    ) -> Result<T, Box<crate::Error>> {
        if self.project_output {
            return decode(&self.body);
        }
        let receipt = self.clean_receipt().ok_or_else(|| {
            let id = self
                .request
                .as_ref()
                .map_or("", absence::RequestIdentity::operation);
            let refusal = operation::unsafe_guard_response(id);
            Box::new(if self.write_response && self.status.is_success() {
                crate::Error::new(crate::ErrorClass::NotSettled).with_cause(refusal)
            } else {
                refusal
            })
        })?;
        decode(receipt.body())
    }

    pub(crate) fn private_issuance(&self) -> Option<crate::Secret> {
        use crate::wire::strict_json::{self, Shape, ShapeField};
        let guard::ReceiptPolicy::PrivateIssuance(field) = self.receipt_policy else {
            return None;
        };
        if !self.status.is_success() || self.body.len() > self.body_cap {
            return None;
        }
        self.request.as_ref()?.direct()?;
        let text = crate::model::wire::go_json(&self.body);
        let fields = [ShapeField {
            name: field,
            shape: Shape::Any,
        }];
        strict_json::validate_exact_shape(text.as_bytes(), &Shape::Object(&fields), text.len())
            .ok()?;
        let values: std::collections::BTreeMap<String, &serde_json::value::RawValue> =
            serde_json::from_str(&text).ok()?;
        let secret =
            crate::Secret::new(serde_json::from_str::<String>(values.get(field)?.get()).ok()?);
        if secret.expose_secret().is_empty()
            || self
                .credentials
                .wire_credentials()
                .text_reflects(secret.expose_secret())
        {
            return None;
        }
        Some(secret)
    }

    pub(crate) fn malformed_json(&self) -> bool {
        serde_json::from_str::<&serde_json::value::RawValue>(&crate::model::wire::go_json(
            &self.body,
        ))
        .is_err()
    }

    pub(crate) fn empty_body(&self) -> bool {
        self.body.is_empty()
    }

    pub(crate) fn classify(
        &self,
        operation: &'static str,
        success: &[http::StatusCode],
        rules: (bool, bool, redirect::Retry),
        full: bool,
    ) -> Result<(), Box<crate::Error>> {
        let classify = if full {
            response::classify_full
        } else {
            response::classify
        };
        self.classify_body(operation, success, rules, &self.body, classify)
    }

    pub(crate) fn classify_normalized_json(
        &self,
        operation: &'static str,
        success: &[http::StatusCode],
        rules: (bool, bool, redirect::Retry),
    ) -> Result<(), Box<crate::Error>> {
        let body = crate::model::wire::go_json(&self.body);
        self.classify_body(
            operation,
            success,
            rules,
            body.as_bytes(),
            response::classify,
        )
    }

    pub(crate) fn classify_billing_transport(
        &self,
        operation: &'static str,
        rules: (bool, bool, redirect::Retry),
    ) -> Result<(), Box<crate::Error>> {
        let body = if self.body.trim_ascii().starts_with(b"[")
            && !response::has_transport_error_entries(&self.body)
        {
            &[]
        } else {
            self.body.as_slice()
        };
        self.classify_body(operation, &[], rules, body, response::classify)
    }

    pub(crate) fn billing_envelope_refusal(
        &self,
        operation: &'static str,
    ) -> Option<Box<crate::Error>> {
        use crate::wire::strict_json::{Shape, ShapeField, validate_folded_canonical_shape};
        const FIELDS: Shape<'static> = Shape::Object(&[
            ShapeField {
                name: "code",
                shape: Shape::Any,
            },
            ShapeField {
                name: "message",
                shape: Shape::Any,
            },
            ShapeField {
                name: "data",
                shape: Shape::Any,
            },
        ]);
        if self.body.len() > self.body_cap {
            return None;
        }
        let text = crate::model::wire::go_json(&self.body);
        validate_folded_canonical_shape(text.as_bytes(), &FIELDS, text.len()).ok()?;
        let (code, message) = response::billing_envelope_fields(&text)?;
        let code = response::sanitize_error_text(&code, &self.credentials);
        if code.as_str().is_empty() {
            return None;
        }
        let message = response::sanitize_billing_refusal_message(message, &self.credentials);
        Some(Box::new(crate::Error::from_api(
            crate::ApiError::without_fallback(
                operation,
                Some(self.status.as_u16()),
                code,
                message,
                false,
            ),
        )))
    }

    fn classify_body(
        &self,
        operation: &'static str,
        success: &[http::StatusCode],
        rules: (bool, bool, redirect::Retry),
        body: &[u8],
        classify: Classifier,
    ) -> Result<(), Box<crate::Error>> {
        if success.contains(&self.status) {
            return Ok(());
        }
        classify(
            operation,
            self.status,
            Bytes::copy_from_slice(body),
            success,
            rules,
            &self.credentials,
        )
        .map(drop)
        .map_err(|error| Box::new(error.with_clean_absence(self.clean_absence())))
    }

    fn transport_screen(&self) -> response::Screen {
        *self.transport_screen.get_or_init(|| {
            let credentials = self.credentials.wire_credentials();
            crate::wire::product::evidence::Evidence::from_credentials(&credentials)
                .go_peer_body_screen(&self.body)
        })
    }

    pub(in crate::wire::product) fn check_write_acknowledgement(
        &self,
        operation: &operation::Operation,
    ) -> Result<(), Box<crate::Error>> {
        if self.write_response && operation.no_output && operation.success.contains(&self.status) {
            self.check_discarded_acknowledgement(operation.id)?;
        }
        Ok(())
    }

    pub(crate) fn check_discarded_acknowledgement(
        &self,
        operation: &'static str,
    ) -> Result<(), Box<crate::Error>> {
        if self.transport_screen().is_clean() {
            Ok(())
        } else {
            Err(self.unconfirmed_acknowledgement(operation))
        }
    }

    fn unconfirmed_acknowledgement(&self, operation: &'static str) -> Box<crate::Error> {
        Box::new(
            crate::Error::new(crate::ErrorClass::NotSettled)
                .with_cause(operation::unsafe_guard_response(operation)),
        )
    }

    pub(crate) fn database_rejected_fields(&self) -> Vec<&'static str> {
        use serde::Deserialize;
        #[derive(Deserialize)]
        struct Entry {
            field: String,
        }
        #[derive(Deserialize)]
        struct Fields {
            #[serde(rename = "fieldError")]
            entries: Vec<Entry>,
        }
        let Ok(fields) = crate::wire::product::lossy::from_slice::<Fields>(&self.body) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for entry in fields.entries.iter().take(20) {
            let field = FIELD_INDEX
                .as_ref()
                .map_or(std::borrow::Cow::Borrowed(entry.field.as_str()), |index| {
                    index.replace_all(&entry.field, "")
                })
                .to_ascii_lowercase();
            let name = match field.as_str() {
                "name" => "Name",
                "password" | "user.password" => "Password",
                "user.name" | "username" => "Username",
                "databases.name" | "user.databases.name" | "databasename" => "DatabaseName",
                "volumetype" => "StorageTypeID",
                "volumesize" => "StorageSize",
                "packageid" => "FlavorID",
                "locatezoneid" => "ZoneID",
                "netids" => "SubnetID",
                _ => continue,
            };
            if !names.contains(&name) {
                names.push(name);
            }
            if names.len() == 5 {
                break;
            }
        }
        names
    }
}

static FIELD_INDEX: std::sync::LazyLock<Option<regex_lite::Regex>> =
    std::sync::LazyLock::new(|| regex_lite::Regex::new(r"\[[0-9]*\]").ok());
