use serde::Deserialize;

use super::super::wire::{WireField, deserialize_field};
use super::super::{Listener, Pool, PoolHealthMonitor, PoolMember, PoolMemberDetail};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, ShapeField};

macro_rules! guard_model {
    ($name:ident { $($field:ident : $type:ty => $wire:literal),* $(,)? }) => {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        pub(super) struct $name {
            $(#[serde(rename = $wire, deserialize_with = "deserialize_field")]
            pub(super) $field: $type,)*
        }

        impl WireField for $name {
            const SHAPE: Shape<'static> = Shape::Object(&[
                $(ShapeField { name: $wire, shape: <$type as WireField>::SHAPE },)*
            ]);

            fn redact(&mut self, evidence: &Evidence) -> bool {
                $(self.$field.redact(evidence) &&)* true
            }
        }
    };
}

guard_model!(Parent {
    id: String => "id", package: String => "package", status: String => "status", kind: String => "type",
});
guard_model!(Region {
    id: String => "id", name: String => "name", status: Option<String> => "status", endpoint: String => "vserverEndpoint",
});
guard_model!(Package {
    id: String => "id", enabled: Option<bool> => "enabled", links: Vec<PackageLink> => "vlbPackages",
});
guard_model!(PackageLink {
    region: String => "region", regional_package: String => "vlb_package_id", glb_package: PackageId => "glb_package_id",
});
guard_model!(Server {
    data: ServerDetail => "data",
});
guard_model!(ServerDetail {
    id: String => "uuid", status: String => "status", interfaces: Vec<Interface> => "internalInterfaces",
});
guard_model!(Interface {
    server: String => "serverUuid", vpc: String => "networkUuid", subnet: String => "subnetUuid", address: String => "fixedIp",
});
guard_model!(Vpc {
    id: String => "id", status: String => "status", cidr: String => "cidr",
});
guard_model!(Subnet {
    id: String => "uuid", vpc: String => "networkUuid", status: String => "status", cidr: String => "cidr",
});
guard_model!(PoolWire {
    id: String => "id", name: String => "name", glb: String => "globalLoadBalancerId", algorithm: String => "algorithm", protocol: String => "protocol", status: String => "status", health: Option<Health> => "health",
});
guard_model!(Health {
    id: String => "id", pool: String => "globalPoolId", glb: String => "globalLoadBalancerId", protocol: String => "protocol", timeout: i64 => "timeout", interval: i64 => "intervalTime", healthy_threshold: i64 => "healthyThreshold", unhealthy_threshold: i64 => "unhealthyThreshold", status: String => "status",
});
guard_model!(Group {
    id: String => "id", name: String => "name", region: String => "region", pool: String => "globalPoolId", glb: String => "globalLoadBalancerId", traffic_dial: i64 => "trafficDial", vpc: String => "vpcId", kind: String => "type", status: String => "status", members: Vec<Member> => "members",
});
guard_model!(Member {
    id: String => "id", name: String => "name", group: String => "globalPoolMemberId", glb: String => "globalLoadBalancerId", subnet: String => "subnetId", address: String => "address", weight: i64 => "weight", port: i64 => "port", monitor_port: i64 => "monitorPort", backup_role: Option<bool> => "backupRole", status: String => "status",
});
guard_model!(ListenerWire {
    id: String => "id", name: String => "name", protocol: String => "protocol", port: i64 => "port", glb: String => "globalLoadBalancerId", pool: String => "globalPoolId", timeout_client: i64 => "timeoutClient", timeout_connection: i64 => "timeoutConnection", timeout_member: i64 => "timeoutMember", allowed_cidrs: String => "allowedCidrs", status: String => "status",
});
guard_model!(Acceptance {
    id: String => "id", glb: String => "globalLoadBalancerId", pool: String => "globalPoolId", groups: Vec<AcceptanceGroup> => "globalPoolMembers",
});
guard_model!(AcceptanceGroup {
    id: String => "id", members: Vec<MemberId> => "members",
});
guard_model!(MemberId { id: String => "id" });

impl From<PoolWire> for Pool {
    fn from(row: PoolWire) -> Self {
        Self {
            id: row.id,
            name: row.name,
            global_load_balancer_id: row.glb,
            algorithm: row.algorithm,
            protocol: row.protocol,
            status: row.status,
            health: row.health.map(Into::into),
            ..Self::default()
        }
    }
}

impl From<Health> for PoolHealthMonitor {
    fn from(row: Health) -> Self {
        Self {
            id: row.id,
            global_pool_id: row.pool,
            global_load_balancer_id: row.glb,
            protocol: row.protocol,
            timeout: row.timeout,
            interval_time: row.interval,
            healthy_threshold: row.healthy_threshold,
            unhealthy_threshold: row.unhealthy_threshold,
            status: row.status,
            ..Self::default()
        }
    }
}

impl From<Group> for PoolMember {
    fn from(row: Group) -> Self {
        Self {
            id: row.id,
            name: row.name,
            region: row.region,
            global_pool_id: row.pool,
            global_load_balancer_id: row.glb,
            traffic_dial: row.traffic_dial,
            vpc_id: row.vpc,
            r#type: row.kind,
            status: row.status,
            members: row.members.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }
}

impl From<Member> for PoolMemberDetail {
    fn from(row: Member) -> Self {
        Self {
            id: row.id,
            name: row.name,
            global_pool_member_id: row.group,
            global_load_balancer_id: row.glb,
            subnet_id: row.subnet,
            address: row.address,
            weight: row.weight,
            port: row.port,
            monitor_port: row.monitor_port,
            backup_role: row.backup_role.unwrap_or_default(),
            status: row.status,
            ..Self::default()
        }
    }
}

impl From<ListenerWire> for Listener {
    fn from(row: ListenerWire) -> Self {
        Self {
            id: row.id,
            name: row.name,
            protocol: row.protocol,
            port: row.port,
            global_load_balancer_id: row.glb,
            global_pool_id: row.pool,
            timeout_client: row.timeout_client,
            timeout_connection: row.timeout_connection,
            timeout_member: row.timeout_member,
            allowed_cidrs: row.allowed_cidrs,
            status: row.status,
            ..Self::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(transparent)]
pub(super) struct PackageId(Option<Box<serde_json::value::RawValue>>);

impl PackageId {
    pub(super) fn matches(&self, id: &str) -> bool {
        self.0.as_ref().is_none_or(|raw| {
            serde_json::from_str::<String>(raw.get()).is_ok_and(|value| value == id)
        })
    }
}

impl WireField for PackageId {
    fn read<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Box::<serde_json::value::RawValue>::deserialize(de).map(|raw| Self(Some(raw)))
    }

    fn redact(&mut self, _: &Evidence) -> bool {
        true
    }
}
