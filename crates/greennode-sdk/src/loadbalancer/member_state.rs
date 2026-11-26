use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use serde::Serialize;
use serde_json::value::RawValue;

use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, field_matches};
use crate::{Error, ErrorClass};

use super::child_writes::{ChildTarget, ChildWriter, canonical_field};
use super::deletes::{local, safe_text};
use super::pool_writes::MEMBER;
use super::wire::{DecodeError, Envelope, Read};
use super::{Pool, PoolMember, WriteError, guard};

pub(super) const MEMBERS: Read = Read {
    id: "loadbalancer.ListPoolMembers",
    route: "v2/{}/loadBalancers/{}/pools/{}/members",
};
const ROWS: Shape<'_> = Shape::Array(&MEMBER);

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Entry {
    pub(super) ip_address: String,
    pub(super) port: i64,
    pub(super) backup: bool,
    pub(super) weight: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(super) name: String,
    #[serde(skip_serializing_if = "zero")]
    pub(super) monitor_port: i64,
}

impl Entry {
    pub(super) fn key(&self) -> (String, i64) {
        key(&self.ip_address, self.port)
    }

    pub(super) fn same_settings(&self, other: &Self) -> bool {
        self.backup == other.backup
            && self.weight == other.weight
            && self.name == other.name
            && self.monitor_port == other.monitor_port
    }
}

impl From<&PoolMember> for Entry {
    fn from(member: &PoolMember) -> Self {
        Self {
            ip_address: member.address.clone(),
            port: member.protocol_port,
            backup: member.backup,
            weight: member.weight,
            name: member.name.clone(),
            monitor_port: default_port(member.protocol_port, member.monitor_port),
        }
    }
}

fn zero(value: &i64) -> bool {
    *value == 0
}

pub(super) fn default_port(port: i64, monitor: i64) -> i64 {
    if monitor == 0 { port } else { monitor }
}

pub(super) fn default_weight(weight: i64) -> i64 {
    if weight == 0 { 1 } else { weight }
}

fn canonical_ip(address: &str) -> String {
    address
        .parse::<IpAddr>()
        .map_or_else(|_| address.to_owned(), |ip| ip.to_string())
}

pub(super) fn key(address: &str, port: i64) -> (String, i64) {
    (canonical_ip(address), port)
}

pub(super) fn validate_pool_document(
    read: &Read,
    raw: &str,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    let fields = guard_object(read, raw, &["uuid", "progressStatus", "members"], evidence)?;
    if let Some(members) = fields.get("members").filter(|raw| raw.get() != "null") {
        validate_member_fields(read, members.get(), &["address", "protocolPort"], evidence)?;
    }
    Ok(())
}

pub(super) fn read_members(raw: &str, evidence: &Evidence) -> Result<Vec<PoolMember>, Box<Error>> {
    if raw == "null" {
        return Err(Box::new(MEMBERS.api(
            "",
            "list response had no data",
            evidence,
        )));
    }
    validate_member_fields(
        &MEMBERS,
        raw,
        &[
            "address",
            "protocolPort",
            "backup",
            "weight",
            "name",
            "monitorPort",
        ],
        evidence,
    )?;
    let members: Vec<PoolMember> = guard::parse(&MEMBERS, raw, &ROWS, evidence)?;
    validate_keys(&MEMBERS, &members, evidence)?;
    Ok(members)
}

fn validate_member_fields(
    read: &Read,
    raw: &str,
    fields: &[&str],
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    let rows: Vec<&RawValue> = serde_json::from_str(raw)
        .map_err(|_| read.decode_error(evidence, DecodeError::InvalidShape))?;
    for row in rows {
        guard_object(read, row.get(), fields, evidence)?;
    }
    Ok(())
}

fn guard_object<'a>(
    read: &Read,
    raw: &'a str,
    fields: &[&str],
    evidence: &Evidence,
) -> Result<BTreeMap<String, &'a RawValue>, Box<Error>> {
    let parsed: BTreeMap<String, &RawValue> = serde_json::from_str(raw)
        .map_err(|_| read.decode_error(evidence, DecodeError::InvalidShape))?;
    if parsed.keys().any(|name| {
        fields
            .iter()
            .any(|field| *field != name && field_matches(field, name))
    }) {
        return Err(read.decode_error(evidence, DecodeError::InvalidShape));
    }
    Ok(parsed)
}

fn validate_keys(
    read: &Read,
    members: &[PoolMember],
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    if members
        .iter()
        .any(|member| member.address.is_empty() || !(1..=65_535).contains(&member.protocol_port))
    {
        return Err(read.decode_error(evidence, DecodeError::InvalidShape));
    }
    Ok(())
}

pub(super) fn pool_and_embedded(
    row: &str,
    read: &Read,
    evidence: &Evidence,
) -> Result<(Pool, bool), Box<Error>> {
    let fields = Envelope::parse(row, read, evidence)?;
    let embedded: Option<Vec<PoolMember>> = canonical_field(&fields, read, evidence, "members")?;
    if let Some(members) = &embedded {
        validate_keys(read, members, evidence)?;
    }
    Ok((read.parse(row, evidence)?, embedded.is_some()))
}

pub(super) fn check_agree(
    write: &ChildWriter<'_>,
    target: &ChildTarget<'_>,
    pool: &Pool,
    embedded: bool,
    members: &[PoolMember],
) -> Result<(), Box<Error>> {
    let expected = key_set(write, target, "the pool read", &pool.members)?;
    let observed = key_set(write, target, "ListPoolMembers", members)?;
    if embedded && expected != observed {
        let reason = format!(
            "the pool read named {} member(s) and ListPoolMembers named {}; the two reads disagree, nothing sent, retry",
            expected.len(),
            observed.len()
        );
        return Err(busy(write, target, &reason));
    }
    Ok(())
}

fn key_set(
    write: &ChildWriter<'_>,
    target: &ChildTarget<'_>,
    source: &str,
    members: &[PoolMember],
) -> Result<BTreeSet<(String, i64)>, Box<Error>> {
    let mut keys = BTreeSet::new();
    for member in members {
        if !keys.insert(key(&member.address, member.protocol_port)) {
            let address = safe_text(&member.address, &write.evidence);
            let reason = format!(
                "{source} names more than one member at {address}:{}; nothing sent, retry",
                member.protocol_port
            );
            return Err(busy(write, target, &reason));
        }
    }
    Ok(keys)
}

fn busy(write: &ChildWriter<'_>, target: &ChildTarget<'_>, reason: &str) -> Box<Error> {
    let id = safe_text(target.id, &write.evidence);
    let message = format!(
        "loadbalancer: resource busy: {}: pool {id}: {reason}",
        write.operation
    );
    Box::new(
        local(ErrorClass::Api, &message, &write.evidence)
            .with_outer_detail(WriteError::ResourceBusy),
    )
}

pub(super) fn submitted(pool: &mut Pool, members: Vec<PoolMember>, entries: Vec<Entry>) {
    let mut existing: BTreeMap<_, _> = members
        .into_iter()
        .map(|member| (key(&member.address, member.protocol_port), member))
        .collect();
    pool.members = entries
        .into_iter()
        .map(|entry| {
            let current = existing.remove(&entry.key()).unwrap_or_default();
            PoolMember {
                address: entry.ip_address,
                protocol_port: entry.port,
                monitor_port: entry.monitor_port,
                weight: entry.weight,
                backup: entry.backup,
                name: entry.name,
                ..current
            }
        })
        .collect();
}

pub(super) fn matches_replacement(members: &[PoolMember], mut sent: Vec<Entry>) -> bool {
    let mut observed: Vec<_> = members.iter().map(Entry::from).collect();
    for entry in sent.iter_mut().chain(&mut observed) {
        entry.ip_address = canonical_ip(&entry.ip_address);
    }
    sent.sort_unstable();
    observed.sort_unstable();
    sent == observed
}
