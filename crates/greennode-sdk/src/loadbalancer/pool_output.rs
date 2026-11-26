use serde::{
    Serialize, Serializer,
    ser::{SerializeSeq, SerializeStruct},
};
use serde_json::Value;

use crate::model::wire::WireField;
use crate::wire::product::evidence::Evidence;

use super::Pool;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PoolProjection(Value);

impl PoolProjection {
    pub(super) fn retain(pool: &mut Pool, evidence: &Evidence) -> Result<Option<Self>, ()> {
        if evidence.text_reflects("Pool") {
            return Err(());
        }
        let mut value = serde_json::to_value(&*pool).map_err(|_| ())?;
        if !evidence.reflected_in(&value) {
            return Ok(None);
        }
        evidence.redact(&mut value);
        if evidence.reflected_in(&value) {
            return Err(());
        }
        scrub_pool(pool, evidence);
        let typed = serde_json::to_value(&*pool).map_err(|_| ())?;
        if evidence.reflected_in(&typed) {
            return Err(());
        }
        Ok(Some(Self(value)))
    }
}

impl Serialize for PoolProjection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Row(&self.0, POOL).serialize(serializer)
    }
}

const POOL: &[&str] = &[
    "UUID",
    "Name",
    "Protocol",
    "Description",
    "LoadBalanceMethod",
    "DisplayStatus",
    "ProgressStatus",
    "Stickiness",
    "TLSEncryption",
    "Members",
    "HealthMonitor",
];
const MEMBER: &[&str] = &[
    "UUID",
    "Address",
    "ProtocolPort",
    "Weight",
    "MonitorPort",
    "SubnetID",
    "Name",
    "PoolID",
    "TypeCreate",
    "Backup",
    "DisplayStatus",
    "CreatedAt",
    "UpdatedAt",
    "CreatedBy",
    "ProgressStatus",
];
const MONITOR: &[&str] = &[
    "Timeout",
    "CreatedAt",
    "UpdatedAt",
    "DomainName",
    "HTTPVersion",
    "HealthCheckProtocol",
    "Interval",
    "HealthyThreshold",
    "UnhealthyThreshold",
    "HealthCheckMethod",
    "HealthCheckPath",
    "SuccessCode",
    "ProgressStatus",
    "DisplayStatus",
];

struct Row<'a>(&'a Value, &'static [&'static str]);

impl Serialize for Row<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(fields) = self.0.as_object() else {
            return self.0.serialize(serializer);
        };
        let mut row = serializer.serialize_struct("PoolFields", fields.len())?;
        for &key in self.1 {
            if let Some(value) = fields.get(key) {
                match key {
                    "HealthMonitor" => row.serialize_field(key, &Row(value, MONITOR))?,
                    "Members" => row.serialize_field(key, &Members(value))?,
                    _ => row.serialize_field(key, value)?,
                }
            }
        }
        row.end()
    }
}

struct Members<'a>(&'a Value);

impl Serialize for Members<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(items) = self.0.as_array() else {
            return self.0.serialize(serializer);
        };
        let mut sequence = serializer.serialize_seq(Some(items.len()))?;
        for item in items {
            sequence.serialize_element(&Row(item, MEMBER))?;
        }
        sequence.end()
    }
}

fn scrub_pool(pool: &mut Pool, evidence: &Evidence) {
    for text in [
        &mut pool.uuid,
        &mut pool.name,
        &mut pool.protocol,
        &mut pool.description,
        &mut pool.load_balance_method,
        &mut pool.display_status,
        &mut pool.progress_status,
    ] {
        text.redact(evidence);
    }
    for flag in [&mut pool.stickiness, &mut pool.tls_encryption] {
        scrub_scalar(flag, evidence);
    }
    for member in &mut pool.members {
        for text in [
            &mut member.uuid,
            &mut member.address,
            &mut member.subnet_id,
            &mut member.name,
            &mut member.pool_id,
            &mut member.type_create,
            &mut member.display_status,
            &mut member.created_at,
            &mut member.updated_at,
            &mut member.created_by,
            &mut member.progress_status,
        ] {
            text.redact(evidence);
        }
        for number in [
            &mut member.protocol_port,
            &mut member.weight,
            &mut member.monitor_port,
        ] {
            scrub_scalar(number, evidence);
        }
        scrub_scalar(&mut member.backup, evidence);
    }
    if let Some(monitor) = &mut pool.health_monitor {
        for text in [
            &mut monitor.created_at,
            &mut monitor.updated_at,
            &mut monitor.health_check_protocol,
            &mut monitor.progress_status,
            &mut monitor.display_status,
        ] {
            text.redact(evidence);
        }
        for text in [
            &mut monitor.domain_name,
            &mut monitor.http_version,
            &mut monitor.health_check_method,
            &mut monitor.health_check_path,
            &mut monitor.success_code,
        ] {
            text.redact(evidence);
        }
        for number in [
            &mut monitor.timeout,
            &mut monitor.interval,
            &mut monitor.healthy_threshold,
            &mut monitor.unhealthy_threshold,
        ] {
            scrub_scalar(number, evidence);
        }
    }
}

fn scrub_scalar<T: Copy + Default + Into<Value>>(value: &mut T, evidence: &Evidence) {
    if evidence.reflected_in(&(*value).into()) {
        *value = T::default();
    }
}
