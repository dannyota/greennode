use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn regional(
    resource: &str,
    action: &str,
    info: Option<&BTreeMap<String, Value>>,
) -> Value {
    let mut body = json!({"resourceType": resource, "action": action});
    if let Some(info) = info.filter(|info| !info.is_empty()) {
        body["resourceInfo"] = json!(info);
    }
    body
}

pub(crate) struct Server<'a> {
    pub(crate) zone: &'a str,
    pub(crate) flavor: &'a str,
    pub(crate) image: &'a str,
    pub(crate) root_size: i64,
    pub(crate) root_type: &'a str,
    pub(crate) data_size: i64,
    pub(crate) data_type: &'a str,
    pub(crate) encrypted: bool,
}

pub(crate) fn server(input: Server<'_>) -> Value {
    let mut info = json!({
        "zoneId": input.zone, "flavorId": input.flavor, "imageId": input.image,
        "rootDiskSize": input.root_size, "rootDiskTypeId": input.root_type,
        "encryptionVolume": input.encrypted, "period": 1, "isPoc": false,
    });
    if input.data_size != 0 {
        info["dataDiskSize"] = json!(input.data_size);
    }
    if !input.data_type.is_empty() {
        info["dataDiskTypeId"] = json!(input.data_type);
    }
    json!({"resourceType": "server", "action": "create", "resourceInfo": info})
}

pub(crate) fn volume(zone: &str, disk: &str, size: i64, encryption: &str) -> Value {
    let mut info =
        json!({"zoneId": zone, "volumeTypeId": disk, "size": size, "period": 1, "isPoc": false});
    if !encryption.is_empty() {
        info["encryptionType"] = json!(encryption);
    }
    json!({"resourceType": "volume", "action": "create", "resourceInfo": info})
}

pub(crate) fn public_ip(kind: &str) -> Value {
    json!({"resourceType": "public-vip", "action": "create", "resourceInfo": {"type": kind, "isBuyMorePo": false, "isPoc": false}})
}

pub(crate) fn storage(quota: i64, purchase: i64, project: i64) -> Value {
    json!({"resourceType": "object_storage", "action": "create", "resourceInfo": {"quota": quota, "purchaseTypeId": purchase, "projectType": project}})
}
