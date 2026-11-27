use http::{Method, StatusCode};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::model::wire::go_json;
use crate::wire::product::options::PathIds;
use crate::wire::strict_json;
use crate::{Failure, RequestContext, Secret, WriteDispatch};

use super::wire::{DecodeError, Envelope, Read, invalid_input, screen_input};
use super::write::{Write, advise, api, reject};
use super::{Certificate, Client};

const OP: &str = "loadbalancer.ImportCertificate";
const REQUEST: Write = Write {
    id: OP,
    route: "v2/{}/cas",
    method: Method::POST,
    success: &[StatusCode::CREATED],
    sensitive: true,
    report_redirect: false,
};

/// Certificate type for TLS imports with a private key.
pub const CERTIFICATE_TYPE_TLS: &str = "TLS/SSL";
/// Certificate type for CA imports.
pub const CERTIFICATE_TYPE_CA: &str = "CA";
/// Fixed message used for provider errors during certificate import.
pub const IMPORT_CERTIFICATE_WITHHELD_MESSAGE: &str =
    "server message withheld: it may quote the private key";

/// Certificate material to import. Debug output withholds every input field.
#[derive(Clone)]
pub struct ImportCertificateInput {
    /// Certificate name.
    pub name: String,
    /// Requested certificate type, such as `TLS/SSL` or `CA`.
    pub r#type: String,
    /// One or more PEM certificate blocks.
    pub certificate: String,
    /// An optional PEM certificate chain for TLS imports.
    pub certificate_chain: String,
    /// One PEM private-key block, required for TLS imports.
    pub private_key: Secret,
    /// An optional private-key passphrase for TLS imports.
    pub passphrase: Secret,
}

impl ImportCertificateInput {
    /// Creates an input with the required public certificate fields.
    pub fn new(
        name: impl Into<String>,
        certificate_type: impl Into<String>,
        certificate: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            r#type: certificate_type.into(),
            certificate: certificate.into(),
            certificate_chain: String::new(),
            private_key: Secret::new(""),
            passphrase: Secret::new(""),
        }
    }
}

crate::error::debug_record!(ImportCertificateInput => |self, f| {
    f.debug_struct("ImportCertificateInput")
        .finish_non_exhaustive()
});

/// Metadata returned by certificate import.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImportCertificateOutput {
    /// Imported certificate metadata.
    pub certificate: Certificate,
}

crate::wire::product::output::public_fields!(ImportCertificateOutput { certificate });

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Body<'a> {
    name: &'a str,
    r#type: &'a str,
    certificate: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    certificate_chain: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    private_key: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    passphrase: &'a str,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Imports a certificate once and returns only certificate metadata.
    pub async fn import_certificate(
        &self,
        ctx: &RequestContext,
        input: ImportCertificateInput,
    ) -> Result<ImportCertificateOutput, Failure<ImportCertificateOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self
                .quote_evidence(ctx)
                .map_err(|error| super::write::not_sent(*error))?;
            let invalid =
                |field: &str| super::write::not_sent(*invalid_input(&format!("{OP}: {field}")));
            let body = Body {
                name: &input.name,
                r#type: &input.r#type,
                certificate: &input.certificate,
                certificate_chain: &input.certificate_chain,
                private_key: input.private_key.expose_secret(),
                passphrase: input.passphrase.expose_secret(),
            };
            let public = [
                ("Name", body.name),
                ("Type", body.r#type),
                ("Certificate", body.certificate),
                ("CertificateChain", body.certificate_chain),
            ];
            screen_input(
                &evidence,
                public
                    .iter()
                    .map(|(_, value)| *value)
                    .chain([body.private_key, body.passphrase]),
            )
            .map_err(|error| super::write::not_sent(*error))?;
            for (field, value) in &public[..3] {
                if value.trim().is_empty() {
                    return Err(super::write::not_sent(*invalid_input(&format!(
                        "{OP} requires {field}"
                    ))));
                }
            }
            evidence.add_redaction(&input.passphrase);
            for (field, value) in public {
                if value.contains("PRIVATE KEY")
                    || (!value.is_empty() && evidence.text_reflects(value))
                {
                    return Err(invalid(&format!(
                        "{field} looks like a private key, not a certificate"
                    )));
                }
            }
            check_certificate("Certificate", body.certificate).map_err(invalid)?;
            if !body.certificate_chain.is_empty() {
                check_certificate("CertificateChain", body.certificate_chain).map_err(invalid)?;
            }
            if input.r#type == CERTIFICATE_TYPE_TLS && body.private_key.is_empty() {
                return Err(super::write::not_sent(*invalid_input(&format!(
                    "{OP} requires PrivateKey when Type is {CERTIFICATE_TYPE_TLS}"
                ))));
            }
            if !body.private_key.is_empty() {
                let (kind, rest) = pem_block(body.private_key)
                    .ok_or_else(|| invalid("PrivateKey must hold a PEM block"))?;
                if !rest.trim().is_empty() {
                    return Err(invalid("PrivateKey must hold exactly one PEM block"));
                }
                if !kind.ends_with("PRIVATE KEY") {
                    return Err(invalid(
                        "PrivateKey must be a PEM block whose type ends in PRIVATE KEY",
                    ));
                }
            }
            if !body.passphrase.is_empty() && body.private_key.is_empty() {
                return Err(invalid(
                    "Passphrase must be empty unless PrivateKey is given",
                ));
            }
            if body.r#type != CERTIFICATE_TYPE_TLS
                && [body.private_key, body.passphrase, body.certificate_chain]
                    .iter()
                    .any(|value| !value.is_empty())
            {
                return Err(invalid(
                    "PrivateKey, Passphrase, and CertificateChain must be empty unless Type is TLS/SSL",
                ));
            }
            evidence.add_redaction(&input.private_key);
            for line in body.private_key.lines() {
                let line = line.trim();
                if line.len() >= 8 && !line.starts_with("-----") && !line.contains(':') {
                    evidence.add_redaction(&Secret::new(line));
                }
            }
            screen_input(&evidence, [body.name, body.r#type])
                .map_err(|error| super::write::not_sent(*error))?;
            let result = self
                .send_once(ctx, &REQUEST, &[], body, &mut evidence)
                .await;
            let advice = format!(
                "{OP}: import may have already reached the server; check with list-certificates --name {:?}, then match exactly",
                input.name
            );
            let advised = |failure| advise(failure, &advice, &evidence);
            let (status, raw) = result.map_err(advised)?;
            let raw = reject(
                &REQUEST,
                status,
                raw,
                &evidence,
                Some(IMPORT_CERTIFICATE_WITHHELD_MESSAGE),
            )
            .map_err(|failure| {
                if status.is_client_error() {
                    failure
                } else {
                    advised(failure)
                }
            })?;
            let read = Read {
                id: OP,
                route: REQUEST.route,
            };
            let failed = |detail| {
                advised(Failure::new(
                    *read.decode_error(&evidence, detail),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ))
            };
            let text = go_json(&raw);
            serde_json::from_str::<&serde_json::value::RawValue>(&text)
                .map_err(|_| failed(DecodeError::MalformedJson))?;
            strict_json::validate(text.as_bytes(), &[], text.len())
                .map_err(|_| failed(DecodeError::InvalidShape))?;
            let decode_failed = |error: Box<crate::Error>| {
                advised(Failure::new(*error, None, WriteDispatch::MayHaveBeenSent))
            };
            let envelope = Envelope::parse(&text, &read, &evidence).map_err(decode_failed)?;
            let row = envelope
                .raw(&read, &evidence, "data")
                .map_err(decode_failed)?;
            let certificate: Certificate = read.parse(row, &evidence).map_err(decode_failed)?;
            if !PathIds::WithUnderscore.allows(&certificate.uuid)
                || evidence.text_reflects(&certificate.uuid)
            {
                let message = format!(
                    "import response had no id; a certificate may exist, check with list-certificates --name {:?} and match exactly",
                    input.name
                );
                return Err(Failure::new(
                    api(OP, Some(status), "", &message, &evidence),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ));
            }
            let certificate = read.retain(certificate, &evidence).map_err(decode_failed)?;
            Ok(ImportCertificateOutput { certificate })
        })
    }
}

fn check_certificate(field: &str, text: &str) -> Result<(), &'static str> {
    let mut block_found = false;
    let mut rest = text;
    while let Some((kind, remaining)) = pem_block(rest) {
        if kind != "CERTIFICATE" {
            return Err(if field == "Certificate" {
                "Certificate must hold only PEM CERTIFICATE blocks"
            } else {
                "CertificateChain must hold only PEM CERTIFICATE blocks"
            });
        }
        block_found = true;
        rest = remaining;
    }
    if block_found {
        Ok(())
    } else {
        Err(if field == "Certificate" {
            "Certificate must hold a PEM CERTIFICATE block"
        } else {
            "CertificateChain must hold a PEM CERTIFICATE block"
        })
    }
}

fn pem_block(mut text: &str) -> Option<(&str, &str)> {
    fn take_line(text: &str) -> (&str, &str) {
        let (line, rest) = text.split_once('\n').unwrap_or((text, ""));
        (line.trim_end_matches(['\r', ' ', '\t']), rest)
    }

    while !text.is_empty() {
        let (line, rest) = take_line(text);
        text = rest;
        let Some(kind) = line
            .strip_prefix("-----BEGIN ")
            .and_then(|line| line.strip_suffix("-----"))
        else {
            continue;
        };
        let mut data = Zeroizing::new(Vec::new());
        let mut headers = true;
        while !text.is_empty() {
            let (line, rest) = take_line(text);
            if line.starts_with("-----BEGIN ") {
                break;
            }
            text = rest;
            if let Some(end) = line
                .strip_prefix("-----END ")
                .and_then(|line| line.strip_suffix("-----"))
            {
                if end == kind && valid_base64(&data) {
                    return Some((kind, text));
                }
                break;
            }
            if headers && line.contains(':') {
                continue;
            }
            if !line.is_empty() {
                headers = false;
                data.extend(line.bytes().filter(|byte| !matches!(byte, b' ' | b'\t')));
            }
        }
    }
    None
}

fn valid_base64(data: &[u8]) -> bool {
    let encoding = data_encoding::BASE64;
    let Ok(size) = encoding.decode_len(data.len()) else {
        return false;
    };
    let mut decoded = Zeroizing::new(vec![0; size]);
    encoding.decode_mut(data, &mut decoded).is_ok()
}
