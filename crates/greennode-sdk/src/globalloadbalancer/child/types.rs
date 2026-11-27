use super::super::{Listener, Pool, PoolMember};
use super::{Evidence, safe_id};
use crate::wire::product::identifier;
use std::fmt;

use serde::{Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;

/// Selects one TCP pool, one private member group, and one backend.
#[derive(Clone, Debug)]
pub struct CreatePoolInput {
    /// Parent global load balancer identifier.
    pub load_balancer_id: String,
    /// Name of the resource to create.
    pub name: String,
    /// Name of the member group and backend row.
    pub member_name: String,
    /// Backend project identifier, which must match the client's resolved project.
    pub backend_project_id: String,
    /// Backend vServer identifier.
    pub backend_server_id: String,
    /// Backend interface VPC identifier.
    pub backend_vpc_id: String,
    /// Backend interface subnet identifier.
    pub backend_subnet_id: String,
    /// TCP service and health check port.
    pub backend_port: i64,
}
impl CreatePoolInput {
    /// Builds the input with all required fields.
    #[expect(
        clippy::too_many_arguments,
        reason = "The operation requires all eight fields"
    )]
    pub fn new(
        load_balancer_id: impl Into<String>,
        name: impl Into<String>,
        member_name: impl Into<String>,
        backend_project_id: impl Into<String>,
        backend_server_id: impl Into<String>,
        backend_vpc_id: impl Into<String>,
        backend_subnet_id: impl Into<String>,
        backend_port: i64,
    ) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
            name: name.into(),
            member_name: member_name.into(),
            backend_project_id: backend_project_id.into(),
            backend_server_id: backend_server_id.into(),
            backend_vpc_id: backend_vpc_id.into(),
            backend_subnet_id: backend_subnet_id.into(),
            backend_port,
        }
    }
}

/// Selects one restricted TCP listener bound to the only pool.
#[derive(Clone, Debug)]
pub struct CreateListenerInput {
    /// Parent global load balancer identifier.
    pub load_balancer_id: String,
    /// Name of the resource to create.
    pub name: String,
    /// Pool identifier.
    pub pool_id: String,
    /// TCP listener port.
    pub port: i64,
    /// Canonical IPv4 source prefixes.
    pub allowed_cidrs: Vec<String>,
    /// Backend project identifier, which must match the client's resolved project.
    pub backend_project_id: String,
    /// Backend vServer identifier.
    pub backend_server_id: String,
}
impl CreateListenerInput {
    /// Builds the input with all required fields.
    pub fn new(
        load_balancer_id: impl Into<String>,
        name: impl Into<String>,
        pool_id: impl Into<String>,
        port: i64,
        allowed_cidrs: Vec<String>,
        backend_project_id: impl Into<String>,
        backend_server_id: impl Into<String>,
    ) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
            name: name.into(),
            pool_id: pool_id.into(),
            port,
            allowed_cidrs,
            backend_project_id: backend_project_id.into(),
            backend_server_id: backend_server_id.into(),
        }
    }
}

/// Selects the pool and its nested backend group for deletion.
#[derive(Clone, Debug)]
pub struct DeletePoolInput {
    /// Parent global load balancer identifier.
    pub load_balancer_id: String,
    /// Pool identifier.
    pub pool_id: String,
}
impl DeletePoolInput {
    /// Builds the input with all required fields.
    pub fn new(load_balancer_id: impl Into<String>, pool_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
            pool_id: pool_id.into(),
        }
    }
}

/// Selects the listener for deletion.
#[derive(Clone, Debug)]
pub struct DeleteListenerInput {
    /// Parent global load balancer identifier.
    pub load_balancer_id: String,
    /// Listener identifier.
    pub listener_id: String,
}
impl DeleteListenerInput {
    /// Builds the input with all required fields.
    pub fn new(load_balancer_id: impl Into<String>, listener_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
            listener_id: listener_id.into(),
        }
    }
}

/// Contains the confirmed pool and group or their safely retained identifiers.
#[derive(Clone, PartialEq)]
pub struct CreatePoolOutput {
    /// Confirmed pool or its identifier.
    pub pool: Pool,
    /// Confirmed member group or its identifier.
    pub pool_member: PoolMember,
    screen: Option<Value>,
}
impl CreatePoolOutput {
    pub(super) fn new(pool: Pool, pool_member: PoolMember, evidence: &Evidence) -> Self {
        let mut result = Self {
            pool,
            pool_member,
            screen: None,
        };
        result.screen = screen(&result, evidence);
        result
    }
}
impl Serialize for CreatePoolOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(value) = self.screen.as_ref() {
            return value.serialize(serializer);
        }
        let mut out = serializer.serialize_struct("CreatePoolOutput", 2)?;
        out.serialize_field("Pool", &self.pool)?;
        out.serialize_field("PoolMember", &self.pool_member)?;
        out.end()
    }
}

/// Contains the confirmed listener or its safely retained identifier.
#[derive(Clone, PartialEq)]
pub struct CreateListenerOutput {
    /// Confirmed listener or its identifier.
    pub listener: Listener,
    screen: Option<Value>,
}
impl CreateListenerOutput {
    pub(super) fn new(listener: Listener, evidence: &Evidence) -> Self {
        let mut result = Self {
            listener,
            screen: None,
        };
        result.screen = screen(&result, evidence);
        result
    }
}
impl Serialize for CreateListenerOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(value) = self.screen.as_ref() {
            return value.serialize(serializer);
        }
        let mut out = serializer.serialize_struct("CreateListenerOutput", 1)?;
        out.serialize_field("Listener", &self.listener)?;
        out.end()
    }
}

/// Identifies the pool and whether absence was confirmed.
#[derive(Clone, PartialEq)]
pub struct DeletePoolOutput {
    /// Parent global load balancer identifier.
    pub load_balancer_id: String,
    /// Target pool identifier.
    pub pool_id: String,
    /// Whether the operation confirmed absence.
    pub deleted: bool,
    screen: Option<Value>,
}
impl DeletePoolOutput {
    pub(super) fn new(glb: &str, id: &str, deleted: bool, evidence: &Evidence) -> Self {
        let mut result = Self {
            load_balancer_id: safe_id(glb, evidence),
            pool_id: safe_id(id, evidence),
            deleted,
            screen: None,
        };
        result.screen = screen(&result, evidence);
        result
    }
}
impl Serialize for DeletePoolOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(value) = self.screen.as_ref() {
            return value.serialize(serializer);
        }
        let mut out = serializer.serialize_struct("DeletePoolOutput", 3)?;
        out.serialize_field(
            "LoadBalancerID",
            &identifier::checked(&self.load_balancer_id),
        )?;
        out.serialize_field("PoolID", &identifier::checked(&self.pool_id))?;
        out.serialize_field("Deleted", &self.deleted)?;
        out.end()
    }
}

/// Identifies the listener and whether absence was confirmed.
#[derive(Clone, PartialEq)]
pub struct DeleteListenerOutput {
    /// Parent global load balancer identifier.
    pub load_balancer_id: String,
    /// Target listener identifier.
    pub listener_id: String,
    /// Whether the operation confirmed absence.
    pub deleted: bool,
    screen: Option<Value>,
}
impl DeleteListenerOutput {
    pub(super) fn new(glb: &str, id: &str, deleted: bool, evidence: &Evidence) -> Self {
        let mut result = Self {
            load_balancer_id: safe_id(glb, evidence),
            listener_id: safe_id(id, evidence),
            deleted,
            screen: None,
        };
        result.screen = screen(&result, evidence);
        result
    }
}
impl Serialize for DeleteListenerOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(value) = self.screen.as_ref() {
            return value.serialize(serializer);
        }
        let mut out = serializer.serialize_struct("DeleteListenerOutput", 3)?;
        out.serialize_field(
            "LoadBalancerID",
            &identifier::checked(&self.load_balancer_id),
        )?;
        out.serialize_field("ListenerID", &identifier::checked(&self.listener_id))?;
        out.serialize_field("Deleted", &self.deleted)?;
        out.end()
    }
}

macro_rules! debug_projection {
    ($($output:ident),+ $(,)?) => {$(
        impl fmt::Debug for $output {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match serde_json::to_value(self) {
                    Ok(value) => fmt::Debug::fmt(&value, f),
                    Err(_) => f.write_str("<redacted>"),
                }
            }
        }
    )+};
}

debug_projection!(
    CreatePoolOutput,
    CreateListenerOutput,
    DeletePoolOutput,
    DeleteListenerOutput
);

fn screen(output: &impl Serialize, evidence: &Evidence) -> Option<Value> {
    let mut value = serde_json::to_value(output).ok()?;
    if !evidence.output_reflects(&value) {
        return None;
    }
    evidence.redact(&mut value);
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Secret;

    #[test]
    fn debug_uses_each_child_outputs_screened_projection() {
        let mut evidence = Evidence::new();
        let numeric = "45673";
        evidence.add_secret(&Secret::new(numeric));
        evidence.add_secret(&Secret::new("true"));
        let pool = CreatePoolOutput::new(
            Pool::default(),
            PoolMember {
                traffic_dial: 45673,
                ..Default::default()
            },
            &evidence,
        );
        let listener = CreateListenerOutput::new(
            Listener {
                port: 45673,
                ..Default::default()
            },
            &evidence,
        );
        let delete_pool = DeletePoolOutput::new("glb-1", "pool-1", true, &evidence);
        let delete_listener = DeleteListenerOutput::new("glb-1", "listener-1", true, &evidence);
        let outputs = [
            (format!("{pool:?}"), numeric),
            (format!("{listener:?}"), numeric),
            (format!("{delete_pool:?}"), "true"),
            (format!("{delete_listener:?}"), "true"),
        ];
        for (debug, secret) in outputs {
            assert!(debug.contains("<redacted>"));
            assert!(!debug.contains(secret));
        }
    }
}
