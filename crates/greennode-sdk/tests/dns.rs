use greennode_sdk::dns::{HostedZone, Record};

#[test]
fn baseline_dns_fixtures_decode_with_go_output_names() {
    let zone: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/dns/get_hosted_zone.json")).unwrap();
    let zone: HostedZone = serde_json::from_value(zone["data"].clone()).unwrap();
    let output = serde_json::to_value(zone).unwrap();
    assert_eq!(output["hostedZoneId"], "zone-1");

    let record: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/dns/get_record.json")).unwrap();
    let record: Record = serde_json::from_value(record["data"].clone()).unwrap();
    let output = serde_json::to_value(record).unwrap();
    assert_eq!(output["recordId"], "record-1");
}
