use super::EstimateInput;
use crate::quote::catalog::storage_region_name;
use crate::wire::product::options::PathIds;
use std::collections::{HashMap, HashSet};

pub(super) fn setup(in_: &EstimateInput, region: &str) -> Result<(), &'static str> {
    if !(0..=2000).contains(&in_.max_requests) {
        return Err("MaxRequests");
    }
    let rows = [
        in_.servers.len(),
        in_.volumes.len(),
        in_.public_ips.len(),
        in_.storage.len(),
    ]
    .into_iter()
    .fold(0usize, usize::saturating_add);
    if !(1..=1000).contains(&rows) {
        return Err(if rows == 0 {
            "setup"
        } else {
            "component count"
        });
    }
    let mut keys = HashSet::new();
    let mut quantity = 0;
    let mut check = |key: &str, count: i64| {
        if key.len() > 64 || !PathIds::WithUnderscore.allows(key) || !keys.insert(key.to_owned()) {
            return Err("Key");
        }
        if !(1..=1000).contains(&count) || count > 10_000 - quantity {
            return Err("Quantity");
        }
        quantity += count;
        Ok(())
    };
    let mut servers = HashMap::new();
    for row in &in_.servers {
        check(&row.key, row.quantity)?;
        for (name, id) in [
            ("ZoneID", &row.zone_id),
            ("FlavorID", &row.flavor_id),
            ("ImageID", &row.image_id),
            ("RootDiskTypeID", &row.root_disk_type_id),
        ] {
            if !PathIds::Strict.allows(id) {
                return Err(name);
            }
        }
        if row.root_disk_size <= 0
            || row.data_disk_size < 0
            || ((row.data_disk_size > 0) == row.data_disk_type_id.is_empty())
        {
            return Err("disk size/type pair");
        }
        if !row.data_disk_type_id.is_empty() && !PathIds::Strict.allows(&row.data_disk_type_id) {
            return Err("DataDiskTypeID");
        }
        if !row.data_disk_encryption_type_id.is_empty() && row.data_disk_size == 0 {
            return Err("DataDiskEncryptionTypeID");
        }
        if [
            &row.root_disk_encryption_type_id,
            &row.data_disk_encryption_type_id,
        ]
        .into_iter()
        .any(|id| !id.is_empty() && !PathIds::WithUnderscore.allows(id))
        {
            return Err("encryption");
        }
        servers.insert(row.key.as_str(), row);
    }
    for row in &in_.volumes {
        check(&row.key, row.quantity)?;
        if row.size <= 0 {
            return Err("Size");
        }
        if !PathIds::Strict.allows(&row.zone_id) || !PathIds::Strict.allows(&row.volume_type_id) {
            return Err("volume selectors");
        }
        if !row.encryption_type_id.is_empty()
            && !PathIds::WithUnderscore.allows(&row.encryption_type_id)
        {
            return Err("EncryptionTypeID");
        }
        if !row.server_key.is_empty()
            && !servers.get(row.server_key.as_str()).is_some_and(|server| {
                server.zone_id == row.zone_id
                    && (row.encryption_type_id.is_empty()
                        || !server.root_disk_encryption_type_id.is_empty())
            })
        {
            return Err("ServerKey association");
        }
    }
    for row in &in_.public_ips {
        check(&row.key, row.quantity)?;
        match row.kind.as_str() {
            "public-vip"
                if matches!(row.r#type.as_str(), "public" | "public-vm" | "public-mkp") => {}
            "floating-ip" | "external-interface" | "reservation" if row.r#type.is_empty() => {}
            "public-vip" | "floating-ip" | "external-interface" | "reservation" => {
                return Err("IP Type");
            }
            _ => return Err("IP Kind"),
        }
    }
    for row in &in_.storage {
        check(&row.key, row.quantity)?;
        if row.r#type.is_empty() || row.quota_gb <= 0 {
            return Err("storage type or quota");
        }
        if storage_region_name(region, &row.region).is_none() {
            return Err("storage region");
        }
    }
    Ok(())
}
