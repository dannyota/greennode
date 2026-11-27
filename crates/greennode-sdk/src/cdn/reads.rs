use crate::wire::product::evidence::Evidence;
use bytes::Bytes;
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::models::{flex_id, nullable};
use super::wire::{Call, convert, list, read_failure};
use super::{ApiKey, Certificate, Client, WebAccelerator, WebAcceleratorSummary, invalid};
use crate::wire::{
    product::options::PathIds,
    strict_json::{Shape, ShapeField, validate_shape},
};
use crate::{Error, ErrorClass, Failure, List, RequestContext, Secret};

const LIST_WEB: Call = Call::get("cdn.ListWebAccelerators", "v1/cdn/list");
const GET_WEB: Call = Call {
    not_found: Some("no such CDN"),
    ..Call::get("cdn.GetWebAccelerator", "v1/cdn/detail/{}")
};
const LIST_CERTIFICATES: Call = Call {
    sensitive: true,
    ..Call::get("cdn.ListCertificates", "v1/certificate/list")
};
const GET_CERTIFICATE: Call = Call {
    sensitive: true,
    not_found: Some("no such certificate"),
    ..Call::get("cdn.GetCertificate", "v1/certificate/detail/{}")
};
const LIST_KEYS: Call = Call {
    sensitive: true,
    ..Call::get("cdn.ListAPIKeys", "v1/apikey/list")
};

/// Input for listing Web Accelerators.
#[derive(Clone, Copy, Default)]
pub struct ListWebAcceleratorsInput;

crate::error::debug_record!(ListWebAcceleratorsInput unit);

/// Web Accelerators in the account.
pub type ListWebAcceleratorsOutput = List<WebAcceleratorSummary>;

/// Input for a Web Accelerator detail read.
#[derive(Clone)]
pub struct GetWebAcceleratorInput {
    /// The Web Accelerator identifier.
    pub cdn_id: String,
}

crate::error::debug_record!(GetWebAcceleratorInput { cdn_id });

impl GetWebAcceleratorInput {
    /// Selects the Web Accelerator to read.
    pub fn new(cdn_id: impl Into<String>) -> Self {
        Self {
            cdn_id: cdn_id.into(),
        }
    }
}

/// The selected Web Accelerator.
#[derive(Clone, serde::Serialize)]
pub struct GetWebAcceleratorOutput {
    /// The configuration and status of the selected CDN.
    #[serde(rename = "WebAccelerator")]
    pub web_accelerator: WebAccelerator,
}

crate::wire::product::output::public_fields!(GetWebAcceleratorOutput { web_accelerator });

/// Input for listing certificates.
#[derive(Clone, Copy, Default)]
pub struct ListCertificatesInput;

crate::error::debug_record!(ListCertificatesInput unit);

/// Certificate metadata without PEM text.
pub type ListCertificatesOutput = List<Certificate>;

/// Input for a certificate detail read.
#[derive(Clone)]
pub struct GetCertificateInput {
    /// The certificate identifier.
    pub certificate_id: String,
}

crate::error::debug_record!(GetCertificateInput { certificate_id });

impl GetCertificateInput {
    /// Selects the certificate to read.
    pub fn new(certificate_id: impl Into<String>) -> Self {
        Self {
            certificate_id: certificate_id.into(),
        }
    }
}

/// The selected certificate and public PEM chain.
#[derive(Clone, serde::Serialize)]
pub struct GetCertificateOutput {
    /// The certificate detail.
    #[serde(rename = "Certificate")]
    pub certificate: Certificate,
}

crate::wire::product::output::public_fields!(GetCertificateOutput { certificate });

/// Input for listing API key metadata.
#[derive(Clone, Copy, Default)]
pub struct ListApiKeysInput;

crate::error::debug_record!(ListApiKeysInput unit);

/// API key metadata without key values.
pub type ListApiKeysOutput = List<ApiKey>;

#[derive(Deserialize, Default)]
#[serde(default)]
struct CertificateWire {
    #[serde(flatten)]
    certificate: Certificate,
    #[serde(rename = "privateKey", deserialize_with = "nullable")]
    private_key: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiKeyWire {
    #[serde(flatten)]
    metadata: ApiKey,
    #[serde(deserialize_with = "nullable")]
    token: String,
}

#[derive(Deserialize)]
struct State {
    #[serde(rename = "cdnId", deserialize_with = "flex_id")]
    id: String,
    status: i64,
}

const STATE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "cdnId",
        shape: Shape::Any,
    },
    ShapeField {
        name: "status",
        shape: Shape::Any,
    },
]);

pub(super) struct WebDetail {
    pub(super) raw: Box<RawValue>,
    pub(super) status: i64,
    pub(super) model: WebAccelerator,
}

fn screened<T: serde::Serialize>(
    output: T,
    call: &Call,
    evidence: &Evidence,
) -> Result<T, Failure<T>> {
    if evidence.serialized_output_reflects(&output) {
        return Err(read_failure(evidence.screen_error(call.unexpected(200))));
    }
    Ok(output)
}

impl Client {
    /// Lists Web Accelerators in the account.
    pub async fn list_web_accelerators(
        &self,
        ctx: &RequestContext,
        _: ListWebAcceleratorsInput,
    ) -> Result<ListWebAcceleratorsOutput, Failure<ListWebAcceleratorsOutput>> {
        crate::wire::product::exit::finish!(async {
            let answer = self
                .call(ctx, &LIST_WEB, &[], Bytes::new())
                .await
                .map_err(convert)?;
            let mut items = list::<WebAcceleratorSummary>(&LIST_WEB, answer.data.as_deref(), true)
                .map_err(convert)?;
            for item in &mut items {
                item.redact(&answer.evidence);
            }
            screened(List { items }, &LIST_WEB, &answer.evidence)
        })
    }

    /// Reads the Web Accelerator with the requested identifier.
    pub async fn get_web_accelerator(
        &self,
        ctx: &RequestContext,
        input: GetWebAcceleratorInput,
    ) -> Result<GetWebAcceleratorOutput, Failure<GetWebAcceleratorOutput>> {
        crate::wire::product::exit::finish!(async {
            if !PathIds::Strict.allows(&input.cdn_id) {
                return Err(invalid());
            }
            let mut evidence = Evidence::new();
            let mut detail = self
                .read_web_detail(ctx, GET_WEB.id, &input.cdn_id, &mut evidence)
                .await
                .map_err(convert)?;
            detail.model.redact(&evidence);
            screened(
                GetWebAcceleratorOutput {
                    web_accelerator: detail.model,
                },
                &GET_WEB,
                &evidence,
            )
        })
    }

    async fn read_web_detail(
        &self,
        ctx: &RequestContext,
        id: &'static str,
        cdn_id: &str,
        evidence: &mut Evidence,
    ) -> Result<WebDetail, Failure<()>> {
        let call = Call { id, ..GET_WEB };
        let data = self
            .call_with_evidence(ctx, &call, &[cdn_id], Bytes::new(), evidence)
            .await?;
        decode_web_detail(&call, cdn_id, data)
    }

    pub(super) async fn read_guard_detail(
        &self,
        ctx: &RequestContext,
        id: &'static str,
        cdn_id: &str,
        evidence: &mut Evidence,
    ) -> Result<crate::wire::product::guard::Guarded<WebDetail>, Failure<()>> {
        let guard = super::wire::GuardCall(Call { id, ..GET_WEB });
        let data = self.call_guard(ctx, &guard, &[cdn_id], evidence).await?;
        let detail = decode_web_detail(&guard.0, cdn_id, data)?;
        crate::wire::product::guard::Guarded::latest(detail, evidence)
            .map_err(|error| read_failure(*error))
    }

    /// Lists certificate metadata without PEM text.
    pub async fn list_certificates(
        &self,
        ctx: &RequestContext,
        _: ListCertificatesInput,
    ) -> Result<ListCertificatesOutput, Failure<ListCertificatesOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut answer = self
                .call(ctx, &LIST_CERTIFICATES, &[], Bytes::new())
                .await
                .map_err(convert)?;
            let rows = list::<CertificateWire>(&LIST_CERTIFICATES, answer.data.as_deref(), true)
                .map_err(convert)?;
            let mut items = Vec::with_capacity(rows.len());
            for row in rows {
                answer.evidence.add_secret(&Secret::new(row.private_key));
                let mut certificate = row.certificate;
                certificate.certificate.clear();
                certificate.ca_root.clear();
                items.push(certificate);
            }
            for certificate in &mut items {
                certificate.redact(&answer.evidence);
            }
            screened(List { items }, &LIST_CERTIFICATES, &answer.evidence)
        })
    }

    /// Reads certificate metadata and its public PEM chain.
    pub async fn get_certificate(
        &self,
        ctx: &RequestContext,
        input: GetCertificateInput,
    ) -> Result<GetCertificateOutput, Failure<GetCertificateOutput>> {
        crate::wire::product::exit::finish!(async {
            if !PathIds::Strict.allows(&input.certificate_id) {
                return Err(invalid());
            }
            let mut answer = self
                .call(
                    ctx,
                    &GET_CERTIFICATE,
                    &[&input.certificate_id],
                    Bytes::new(),
                )
                .await
                .map_err(convert)?;
            let row: CertificateWire = answer
                .data
                .as_deref()
                .and_then(|data| serde_json::from_str(data.get()).ok())
                .ok_or_else(|| read_failure(GET_CERTIFICATE.unexpected(200)))?;
            answer.evidence.add_secret(&Secret::new(row.private_key));
            let mut certificate = row.certificate;
            certificate.redact(&answer.evidence);
            screened(
                GetCertificateOutput { certificate },
                &GET_CERTIFICATE,
                &answer.evidence,
            )
        })
    }

    /// Lists key metadata and marks the key used for this request.
    pub async fn list_api_keys(
        &self,
        ctx: &RequestContext,
        _: ListApiKeysInput,
    ) -> Result<ListApiKeysOutput, Failure<ListApiKeysOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut answer = self
                .call(ctx, &LIST_KEYS, &[], Bytes::new())
                .await
                .map_err(convert)?;
            let rows =
                list::<ApiKeyWire>(&LIST_KEYS, answer.data.as_deref(), false).map_err(convert)?;
            let key = self
                .session
                .cdn_key()
                .ok_or_else(|| read_failure(Error::new(ErrorClass::NoCredentials)))?;
            let comparator = {
                let mut comparison_key = Zeroizing::new([0; 32]);
                getrandom::fill(comparison_key.as_mut())
                    .map_err(|_| read_failure(Error::new(ErrorClass::Api)))?;
                Hmac::<Sha256>::new_from_slice(comparison_key.as_ref())
                    .map_err(|_| read_failure(Error::new(ErrorClass::Api)))?
            };
            let mut expected = comparator.clone();
            expected.update(key.expose_secret().as_bytes());
            let expected = expected.finalize().into_bytes();
            let mut items = Vec::with_capacity(rows.len());
            for row in rows {
                let mut metadata = row.metadata;
                let token = Secret::new(row.token);
                let mut tag = comparator.clone();
                tag.update(token.expose_secret().as_bytes());
                metadata.current =
                    !token.expose_secret().is_empty() && tag.verify_slice(&expected).is_ok();
                answer.evidence.add_secret(&token);
                items.push(metadata);
            }
            for metadata in &mut items {
                metadata.redact(&answer.evidence);
            }
            screened(List { items }, &LIST_KEYS, &answer.evidence)
        })
    }
}

fn decode_web_detail(
    call: &Call,
    cdn_id: &str,
    data: Option<Box<serde_json::value::RawValue>>,
) -> Result<WebDetail, Failure<()>> {
    let malformed = || read_failure(call.unexpected(200));
    let raw = data.ok_or_else(malformed)?;
    let text = raw.get();
    validate_shape(text.as_bytes(), &STATE, text.len()).map_err(|_| malformed())?;
    let state: State = serde_json::from_str(text).map_err(|_| malformed())?;
    if state.id != cdn_id {
        return Err(malformed());
    }
    let mut model: WebAccelerator = serde_json::from_str(text).map_err(|_| malformed())?;
    model.status = super::Field::Value(state.status);
    Ok(WebDetail {
        raw,
        status: state.status,
        model,
    })
}
