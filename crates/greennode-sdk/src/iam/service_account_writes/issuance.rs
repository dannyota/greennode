use super::*;

#[derive(Default)]
pub(super) struct IssuedAccount {
    pub(super) acknowledged: Option<crate::wire::product::guard::AcknowledgedId>,
    pub(super) secret: Option<Secret>,
}

impl Wire for IssuedAccount {
    fn issued_only(secret: Secret) -> Option<Self> {
        Some(Self {
            acknowledged: None,
            secret: Some(secret),
        })
    }

    const SHAPE: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "id",
            shape: Shape::Any,
        },
        ShapeField {
            name: "clientSecret",
            shape: Shape::Any,
        },
    ]);

    fn decode(_: &RawValue) -> Result<Self, serde_json::Error> {
        Ok(Self::default())
    }

    fn decode_acknowledgement(
        raw: &RawValue,
        receipt: crate::wire::product::guard::CleanReceipt<'_>,
    ) -> Result<Self, serde_json::Error> {
        let fields: BTreeMap<String, &RawValue> =
            serde_json::from_str::<Option<_>>(raw.get())?.unwrap_or_default();
        let id = optional_field(&fields, "id")?.unwrap_or_default();
        Ok(Self {
            acknowledged: Some(receipt.bind_id(id)),
            secret: read_issued_secret(&fields)?,
        })
    }
}

#[derive(Default)]
pub(super) struct IssuedSecret {
    pub(super) secret: Option<Secret>,
    pub(super) field_only: bool,
}

impl Wire for IssuedSecret {
    fn issued_only(secret: Secret) -> Option<Self> {
        Some(Self {
            secret: Some(secret),
            field_only: true,
        })
    }

    const SHAPE: Shape<'static> = Shape::Object(&[ShapeField {
        name: "clientSecret",
        shape: Shape::Any,
    }]);

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields = serde_json::from_str::<Option<_>>(raw.get())?.unwrap_or_default();
        Ok(Self {
            secret: read_issued_secret(&fields)?,
            field_only: false,
        })
    }
}

/// Borrows a screened credential from an unconfirmed reset for private delivery.
pub fn issued_reset_service_account_secret(
    failure: &Failure<ResetServiceAccountSecretOutput>,
) -> Option<&Secret> {
    failure.private_delivery()
}

fn read_issued_secret(
    fields: &BTreeMap<String, &RawValue>,
) -> Result<Option<Secret>, serde_json::Error> {
    Ok(optional_field::<String>(fields, "clientSecret")?
        .filter(|value| !value.is_empty())
        .map(Secret::new))
}
