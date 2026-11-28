use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use sha2::{Digest, Sha256};

use crate::network::{exposure_invalid, exposure_reflects, read_security_group_exposure};
use crate::wire::product::{
    evidence::{Evidence, SentCredentials},
    options::{AttemptCheck, PathIds},
};
use crate::wire::strict_json::{self, field_matches};
use crate::{Error, RequestContext, pricing::GetQuoteOutput};

use super::quotes::path_id;
use super::wire::{api, invalid_input};
use super::{Client, Server};

const MAX_GROUPS: usize = 64;
const MAX_RULES: usize = 4096;
const CREDENTIAL_LIMIT: usize = 256;
const LOGIN_PORTS: [u16; 4] = [22, 234, 3389, 3490];
const RULE_FIELDS: [&str; 7] = [
    "direction",
    "etherType",
    "protocol",
    "portRangeMin",
    "portRangeMax",
    "remoteIpPrefix",
    "remoteGroupId",
];

/// The requested floating IP could expose guest login, or the rules could not establish restricted login access.
#[derive(Clone, Copy)]
pub struct OpenLogin;

crate::error::debug_record!(OpenLogin unit);

pub(super) fn groups<'a>(operation: &str, ids: &'a [String]) -> Result<Vec<&'a str>, Box<Error>> {
    let mut seen = HashSet::new();
    let mut groups = Vec::new();
    for id in ids {
        path_id(operation, "SecurityGroupIDs", id)?;
        if groups.len() <= MAX_GROUPS && seen.insert(id.as_str()) {
            groups.push(id.as_str());
        }
    }
    if !(1..=MAX_GROUPS).contains(&groups.len()) {
        return Err(Box::new(invalid_input(&format!(
            "{operation} requires 1 to 64 security groups"
        ))));
    }
    Ok(groups)
}

impl Client {
    pub(super) async fn exposure_snapshot(
        &self,
        ctx: &RequestContext,
        groups: &[&str],
        allow_open_login: bool,
        evidence: &mut Evidence,
        attempt_check: Option<AttemptCheck<'_>>,
    ) -> Result<[u8; 32], Box<Error>> {
        let ctx = self.server_deadline(ctx);
        let mut digest = Sha256::new();
        let mut rules = Vec::new();
        for id in groups {
            let documents = read_security_group_exposure(
                &self.session,
                &ctx,
                id,
                evidence,
                attempt_check,
                MAX_RULES - rules.len(),
            )
            .await?;
            if documents.rules.len() > MAX_RULES - rules.len() {
                return Err(exposure_invalid(evidence));
            }
            hash_raw(&documents.group, &mut digest).map_err(|_| exposure_invalid(evidence))?;
            digest.update(documents.rules.len().to_be_bytes());
            for rule in documents.rules {
                if let Some(end) = ctx.check(self.session.clock()) {
                    return Err(Box::new(end.into()));
                }
                check_rule_keys(&rule).map_err(|()| exposure_invalid(evidence))?;
                hash_raw(&rule, &mut digest).map_err(|_| exposure_invalid(evidence))?;
                rules.push(rule);
            }
        }
        if let Some(end) = ctx.check(self.session.clock()) {
            return Err(Box::new(end.into()));
        }
        crate::wire::product::guard::decide(evidence, |_| ())?;
        if !allow_open_login {
            check_exposure(
                rules.iter().map(Box::as_ref),
                evidence,
                &ctx,
                self.session.clock(),
            )?;
        }
        let digest = digest.finalize().into();
        if let Some(end) = ctx.check(self.session.clock()) {
            return Err(Box::new(end.into()));
        }
        Ok(digest)
    }
}

fn check_rule_keys(raw: &RawValue) -> Result<(), ()> {
    let fields = RULE_FIELDS.map(|name| strict_json::Field {
        name,
        children: &[],
    });
    strict_json::validate(raw.get().as_bytes(), &fields, 8 << 20).map_err(|_| ())?;
    let object: BTreeMap<String, &RawValue> = serde_json::from_str(raw.get()).map_err(|_| ())?;
    if object.keys().any(|key| {
        RULE_FIELDS
            .iter()
            .any(|field| key != field && field_matches(key, field))
    }) {
        return Err(());
    }
    Ok(())
}

fn hash_raw(raw: &RawValue, digest: &mut Sha256) -> Result<(), serde_json::Error> {
    let text = raw.get();
    match text.as_bytes().first() {
        Some(b'{') => {
            let fields: BTreeMap<String, &RawValue> = serde_json::from_str(text)?;
            digest.update(b"{");
            digest.update(fields.len().to_be_bytes());
            for (key, value) in fields {
                digest.update(key.len().to_be_bytes());
                digest.update(key.as_bytes());
                hash_raw(value, digest)?;
            }
        }
        Some(b'[') => {
            let fields: Vec<&RawValue> = serde_json::from_str(text)?;
            digest.update(b"[");
            digest.update(fields.len().to_be_bytes());
            for value in fields {
                hash_raw(value, digest)?;
            }
        }
        Some(b'"') => {
            let value = zeroize::Zeroizing::new(serde_json::from_str::<String>(text)?);
            digest.update(b"\"");
            digest.update(value.len().to_be_bytes());
            digest.update(value.as_bytes());
        }
        _ => {
            digest.update(b"#");
            digest.update(text.len().to_be_bytes());
            digest.update(text.as_bytes());
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rule {
    direction: String,
    ether_type: String,
    protocol: String,
    port_range_min: Option<crate::model::wire::Field<i64>>,
    port_range_max: Option<crate::model::wire::Field<i64>>,
    remote_ip_prefix: String,
    remote_group_id: String,
}

fn check_exposure<'a>(
    rules: impl Iterator<Item = &'a RawValue>,
    evidence: &Evidence,
    ctx: &RequestContext,
    clock: &dyn crate::Clock,
) -> Result<(), Box<Error>> {
    let unsupported = || open_login("unsupported rule encoding", evidence);
    let mut spans: BTreeMap<(bool, u16), Vec<(u128, u128)>> = BTreeMap::new();
    for raw in rules {
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        let rule: Rule = serde_json::from_str(raw.get()).map_err(|_| unsupported())?;
        if !matches!(rule.direction.as_str(), "ingress" | "egress")
            || !matches!(rule.ether_type.as_str(), "IPv4" | "IPv6")
            || rule.remote_group_id.is_empty()
        {
            return Err(unsupported());
        }
        let any = rule.protocol.eq_ignore_ascii_case("any");
        if !any && !rule.protocol.eq_ignore_ascii_case("tcp") {
            let mut bytes = rule.protocol.bytes();
            if rule.protocol.len() <= 16
                && bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
                && bytes.all(|byte| byte.is_ascii_alphanumeric())
            {
                return Err(open_login(
                    &format!("unsupported rule encoding (protocol {:?})", rule.protocol),
                    evidence,
                ));
            }
            return Err(unsupported());
        }
        let bounds = (
            u16::try_from(rule.port_range_min.ok_or_else(unsupported)?.0)
                .map_err(|_| unsupported())?,
            u16::try_from(rule.port_range_max.ok_or_else(unsupported)?.0)
                .map_err(|_| unsupported())?,
        );
        if bounds.0 > bounds.1 || ((any || bounds.0 == 0) && bounds != (0, u16::MAX)) {
            return Err(unsupported());
        }
        let ipv4 = rule.ether_type == "IPv4";
        let interval = prefix_interval(&rule.remote_ip_prefix, ipv4).ok_or_else(unsupported)?;
        if rule.direction == "ingress" {
            for port in LOGIN_PORTS {
                if (bounds.0..=bounds.1).contains(&port) {
                    spans.entry((ipv4, port)).or_default().push(interval);
                }
            }
        }
    }
    for ((ipv4, _), mut intervals) in spans {
        let last = if ipv4 {
            u128::from(u32::MAX)
        } else {
            u128::MAX
        };
        if covers_all(&mut intervals, last) {
            return Err(open_login("port open to the internet", evidence));
        }
    }
    Ok(())
}

fn prefix_interval(prefix: &str, ipv4: bool) -> Option<(u128, u128)> {
    let (address, bits) = prefix.split_once('/')?;
    if bits.is_empty()
        || (bits.len() > 1 && bits.starts_with('0'))
        || !bits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let bits = bits.parse::<u32>().ok()?;
    let (number, width, full) = match address.parse::<IpAddr>().ok()? {
        IpAddr::V4(address) if ipv4 => {
            (u128::from(u32::from(address)), 32_u32, u128::from(u32::MAX))
        }
        IpAddr::V6(address) if !ipv4 && address.to_ipv4_mapped().is_none() => {
            (u128::from(address), 128_u32, u128::MAX)
        }
        _ => return None,
    };
    let host = width.checked_sub(bits)?;
    let mask = u128::MAX.checked_shl(host).unwrap_or_default() & full;
    Some((number & mask, number | (full & !mask)))
}

fn covers_all(intervals: &mut [(u128, u128)], last: u128) -> bool {
    intervals.sort_unstable();
    let mut next = 0_u128;
    for &(first, end) in intervals.iter() {
        if first > next {
            return false;
        }
        if end == last {
            return true;
        }
        next = next.max(end.saturating_add(1));
    }
    false
}

fn open_login(reason: &str, evidence: &Evidence) -> Box<Error> {
    Box::new(
        api(
            "",
            None,
            "OpenLogin",
            &format!("floating IP: login exposure refused: {reason}"),
            evidence,
        )
        .with_detail(OpenLogin),
    )
}

pub(super) fn address_settled(server: &Server, evidence: &Evidence) -> bool {
    if !server.status.eq_ignore_ascii_case("ACTIVE")
        || server.public_ip.is_empty()
        || evidence.text_reflects(&server.public_ip)
    {
        return false;
    }
    let mut id = None;
    for nic in server.internal_interfaces.as_deref().unwrap_or_default() {
        if nic.floating_ip.is_empty() && nic.floating_ipid.is_empty() {
            continue;
        }
        if !PathIds::Strict.allows(&nic.floating_ipid)
            || evidence.id_reflects(&nic.floating_ipid)
            || evidence.text_reflects(&nic.floating_ip)
            || !nic
                .floating_ip
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.to_canonical().to_string() == server.public_ip)
            || id.is_some_and(|previous| previous != nic.floating_ipid)
        {
            return false;
        }
        id = Some(nic.floating_ipid.as_str());
    }
    id.is_some()
}

pub(super) const ADDRESS_WAIT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(super) struct FloatingWatch {
    quote: Mutex<Option<WatchedQuote>>,
}

struct WatchedQuote {
    quote: GetQuoteOutput,
    value: Value,
    screened: HashSet<[u8; 32]>,
}

impl FloatingWatch {
    pub(super) fn check(&self, credentials: &SentCredentials) -> Option<Box<Error>> {
        if credentials.bearers().len() > CREDENTIAL_LIMIT {
            let mut evidence = Evidence::new();
            evidence.absorb(credentials);
            return Some(Box::new(api(
                "",
                None,
                "InvalidResponse",
                "credential observation failed; response withheld",
                &evidence,
            )));
        }
        let Ok(mut quote) = self.quote.lock() else {
            return Some(quote_invalid(&Evidence::new()));
        };
        let watched = quote.as_mut()?;
        let mut newly_screened = Vec::new();
        let evidence = Evidence::from_credentials(credentials);
        for credential in credentials.values() {
            let id = fingerprint(credential);
            if !watched.screened.contains(&id) {
                newly_screened.push(id);
            }
        }
        if !newly_screened.is_empty()
            && (evidence.serialized_output_reflects(&watched.quote)
                || quote_reflects(&watched.value, &evidence))
        {
            return Some(quote_invalid(&evidence));
        }
        watched.screened.extend(newly_screened);
        None
    }

    pub(super) fn retain_quote(
        &self,
        quote: &GetQuoteOutput,
        evidence: &Evidence,
    ) -> Result<(), Box<Error>> {
        let value = serde_json::to_value(quote).map_err(|_| quote_invalid(evidence))?;
        if evidence.serialized_output_reflects(quote) || quote_reflects(&value, evidence) {
            return Err(quote_invalid(evidence));
        }
        let screened = evidence
            .credentials()
            .values()
            .iter()
            .map(fingerprint)
            .collect();
        *self.quote.lock().map_err(|_| quote_invalid(evidence))? = Some(WatchedQuote {
            quote: quote.clone(),
            value,
            screened,
        });
        Ok(())
    }
}

fn fingerprint(credential: &crate::Secret) -> [u8; 32] {
    Sha256::digest(credential.expose_secret().as_bytes()).into()
}

pub(super) fn exhausted(evidence: &Evidence) -> bool {
    evidence.credentials().bearers().len() > CREDENTIAL_LIMIT
}

pub(super) fn can_return_server(server: &Server, evidence: &Evidence) -> bool {
    !exhausted(evidence)
        && serde_json::to_string(server).is_ok_and(|text| !exposure_reflects(&text, evidence))
}

fn quote_invalid(evidence: &Evidence) -> Box<Error> {
    Box::new(api(
        "compute.CreateServer",
        None,
        "InvalidResponse",
        "invalid server response; body withheld",
        evidence,
    ))
}

fn quote_reflects(value: &Value, evidence: &Evidence) -> bool {
    if evidence.serialized_output_reflects(value) {
        return true;
    }
    match value {
        Value::Number(amount) => {
            evidence.identifier_output_reflects(value)
                || evidence.output_reflects(value)
                || amount.as_f64().is_some_and(|amount| {
                    evidence.text_reflects(&crate::output_number::fixed_two(amount))
                        || [
                            crate::output_number::Style::Json,
                            crate::output_number::Style::Fixed,
                        ]
                        .into_iter()
                        .any(|style| {
                            crate::output_number::f64(amount, style)
                                .is_none_or(|text| evidence.text_reflects(&text))
                        })
                })
        }
        Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| evidence.text_reflects(key) || quote_reflects(value, evidence)),
        Value::Array(items) => items.iter().any(|value| quote_reflects(value, evidence)),
        _ => evidence.output_reflects(value),
    }
}

#[cfg(test)]
mod quote_tests {
    use super::*;

    #[test]
    fn retained_quote_checks_struct_field_order() {
        let watch = FloatingWatch::default();
        let quote = GetQuoteOutput {
            optimum_price: 20.0,
            original_price: 25.0,
            discount_price: 5.0,
            discount_percent: 20.0,
            properties: vec![],
        };
        watch.retain_quote(&quote, &Evidence::new()).unwrap();
        let credentials =
            SentCredentials::for_test(vec![crate::Secret::new("25.0,\"DiscountPrice\"")]);
        assert!(watch.check(&credentials).is_some());
    }

    #[test]
    fn quote_screens_cross_field_boundary_and_null() {
        let mut evidence = Evidence::new();
        evidence.add_bearer(&crate::Secret::new("Price\":20"));
        assert!(quote_reflects(
            &serde_json::json!({"OptimumPrice":20}),
            &evidence
        ));
        let mut evidence = Evidence::new();
        evidence.add_bearer(&crate::Secret::new("nUlL"));
        assert!(quote_reflects(&serde_json::json!(null), &evidence));
    }

    #[test]
    fn later_credential_matching_sdk_scientific_rendering_withholds_quote() {
        let mut evidence = Evidence::new();
        evidence.add_bearer(&crate::Secret::new(" 1.23456789e+20 "));
        assert!(quote_reflects(&serde_json::json!(1.23456789e20), &evidence));
    }

    #[test]
    fn later_credential_matching_fixed_numeric_rendering_withholds_quote() {
        let mut evidence = Evidence::new();
        evidence.add_bearer(&crate::Secret::new(" 0.0000001 "));
        assert!(quote_reflects(&serde_json::json!(1e-7), &evidence));
    }
}
