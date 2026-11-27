use super::inventory::{get_input, id, list_input, output, plain};
use super::quota::integer;
use super::wire::{check_path_id, failure};
use super::{Client, ResponseField};
use crate::model::wire::{Field, WireField, model};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::Shape;
use crate::{Failure, List, RequestContext};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use std::{collections::BTreeMap, fmt};

const GET: super::wire::Read = plain("monitor.GetCheck", "vmonitor-uptime-manager/v1/uptimes/{}");
const LIST: super::wire::Read = plain("monitor.ListChecks", "vmonitor-uptime-manager/v1/uptimes");
const LOCATIONS: super::wire::Read = plain(
    "monitor.ListLocations",
    "vmonitor-uptime-manager/v1/locations",
);

get_input!(
    /// Selects one synthetic check.
    GetCheckInput,
    /// Check identifier.
    check_id
);
list_input!(
    /// Selects every check on the account.
    ListChecksInput
);
list_input!(
    /// Selects every available probe location.
    ListLocationsInput
);
/// Account checks in provider order.
pub type ListChecksOutput = List<Check>;
/// Available probe locations.
pub type ListLocationsOutput = List<Location>;
/// Selected synthetic check.
#[derive(Clone, Debug, Serialize)]
pub struct GetCheckOutput {
    /// The selected check.
    #[serde(rename = "Check")]
    pub check: Check,
}

model! { @ true;
    /// Configuration and state of a synthetic check.
    pub struct Check {
        /// Check identifier.
        id:String=>"id","ID";
        /// Check display name.
        name:String=>"name","Name";
        /// Provider check type.
        r#type:String=>"type","Type";
        /// Check subtype.
        subtype:String=>"subtype","Subtype";
        /// Whether the check is enabled.
        status:String=>"status","Status";
        /// Request and assertions used by the check.
        config:CheckConfig=>"config","Config";
        /// Probe scheduling and failure policy.
        options:CheckOptions=>"options","Options";
        /// Configured probe location identifiers.
        locations:Vec<String> =>"locations","Locations";
        /// Channels to notify on probe state transitions.
        notifications:CheckNotifications=>"notifications","Notifications";
        /// Provider creation timestamp text.
        created_at:String=>"created_at","CreatedAt";
        /// Provider update timestamp text.
        updated_at:String=>"updated_at","UpdatedAt";
        /// Current alarm state.
        monitor_status:String=>"monitor_status","MonitorStatus";
        /// Alarm definition identifier.
        history_alarm_id:String=>"history_alarm","HistoryAlarmID";
    }
}
model! { @ true;
    /// Request and assertions used by a synthetic check.
    pub struct CheckConfig {
        /// Probe request.
        request:CheckRequest=>"request","Request";
        /// Conditions evaluated against the response.
        assertions:Vec<Assertion> =>"assertions","Assertions";
    }
}
model! { @ true;
    /// One assertion against a probe response.
    pub struct Assertion {
        /// Assertion type.
        r#type:String=>"type","Type";
        /// Comparison operator.
        operator:String=>"operator","Operator";
        /// Comparison target.
        target:String=>"target","Target";
    }
}
model! { @ true;
    /// Channels notified on a check's state transitions.
    pub struct CheckNotifications {
        /// Identifiers for notifications on entering alarm.
        in_alarm:Vec<String> =>"In-alarm","InAlarm";
        /// Identifiers for notifications on recovering.
        up:Vec<String> =>"Up","Up";
        /// Identifiers for notifications on entering an unknown state.
        undetermined:Vec<String> =>"Undetermined","Undetermined";
    }
}
model! { @ true;
    /// An available probe location.
    pub struct Location {
        /// Location identifier.
        id:String=>"id","ID";
        /// Location name.
        name:String=>"name","Name";
        /// Provider location type.
        r#type:String=>"type","Type";
        /// Location description.
        description:String=>"description","Description";
        /// Current availability state.
        status:String=>"status","Status";
        /// Provider creation timestamp text.
        created_at:String=>"created_at","CreatedAt";
        /// Provider update timestamp text.
        updated_at:String=>"updated_at","UpdatedAt";
    }
}

/// HTTP request used by a check. The URL, maps and body can contain credentials.
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CheckRequest {
    /// Request URL.
    #[serde(rename = "URL")]
    pub url: String,
    /// HTTP method.
    pub method: String,
    /// Request headers.
    #[serde(serialize_with = "text_map")]
    pub headers: BTreeMap<String, String>,
    /// Query parameters.
    #[serde(serialize_with = "text_map")]
    pub query: BTreeMap<String, String>,
    /// Request body.
    pub body: String,
    /// Request timeout in seconds.
    pub timeout: ResponseField<i64>,
    /// Whether TLS certificate verification is enabled.
    #[serde(rename = "VerifiedSSL")]
    pub verified_ssl: ResponseField<bool>,
}
impl fmt::Debug for CheckRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckRequest").finish_non_exhaustive()
    }
}

/// Scheduling and failure policy for a synthetic check.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CheckOptions {
    /// Minutes between tests.
    pub test_frequency: ResponseField<i64>,
    /// Number of tests.
    pub tests: ResponseField<i64>,
    /// Threshold for failing locations.
    pub failed_locations: ResponseField<i64>,
}

#[derive(Clone, Default, Debug, PartialEq, Serialize)]
struct Whole(i64);
impl<'de> Deserialize<'de> for Whole {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        integer(&raw)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("expected an integral number"))
    }
}
impl WireField for Whole {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.0.redact(evidence)
    }
}

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
struct TextMap(BTreeMap<String, FieldText>);
#[derive(Clone, Default, Debug, PartialEq, Serialize)]
struct FieldText(String);
impl<'de> Deserialize<'de> for FieldText {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Field::<String>::deserialize(d).map(|field| Self(field.0))
    }
}
impl WireField for TextMap {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.0.retain(|key, _| !evidence.text_reflects(key));
        self.0.values_mut().all(|value| value.0.redact(evidence))
    }
}
fn text_map<S: Serializer>(
    map: &BTreeMap<String, String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_newtype_struct("$greennode::dynamic-map", map)
}

model! { @ true; struct RequestWire {
    url:String=>"url","URL"; method:String=>"method","Method"; headers:TextMap=>"headers","Headers"; query:TextMap=>"query","Query";
    body:String=>"body","Body"; timeout:Whole=>"timeout","Timeout"; verified_ssl:bool=>"verified_ssl","VerifiedSSL";
}}
model! { @ true; struct OptionsWire {
    test_frequency:Whole=>"test_frequency","TestFrequency"; tests:Whole=>"tests","Tests"; failed_locations:Whole=>"failed_locations","FailedLocations";
}}
impl<'de> Deserialize<'de> for CheckRequest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = RequestWire::deserialize(d)?;
        let values = |map: TextMap| {
            map.0
                .into_iter()
                .map(|(key, value)| (key, value.0))
                .collect()
        };
        Ok(Self {
            url: wire.url,
            method: wire.method,
            headers: values(wire.headers),
            query: values(wire.query),
            body: wire.body,
            timeout: ResponseField::Value(wire.timeout.0),
            verified_ssl: ResponseField::Value(wire.verified_ssl),
        })
    }
}
impl WireField for CheckRequest {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.headers.retain(|key, _| !evidence.text_reflects(key));
        self.query.retain(|key, _| !evidence.text_reflects(key));
        [&mut self.url, &mut self.method, &mut self.body]
            .into_iter()
            .chain(self.headers.values_mut())
            .chain(self.query.values_mut())
            .all(|value| value.redact(evidence))
            && self.timeout.redact(evidence)
            && self.verified_ssl.redact(evidence)
    }
}
impl<'de> Deserialize<'de> for CheckOptions {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = OptionsWire::deserialize(d)?;
        Ok(Self {
            test_frequency: ResponseField::Value(wire.test_frequency.0),
            tests: ResponseField::Value(wire.tests.0),
            failed_locations: ResponseField::Value(wire.failed_locations.0),
        })
    }
}
impl WireField for CheckOptions {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.test_frequency.redact(evidence)
            && self.tests.redact(evidence)
            && self.failed_locations.redact(evidence)
    }
}
fn screen_check(check: &mut Check, evidence: &Evidence) -> bool {
    let ids = [&mut check.id, &mut check.history_alarm_id]
        .into_iter()
        .chain(&mut check.locations)
        .chain(&mut check.notifications.in_alarm)
        .chain(&mut check.notifications.up)
        .chain(&mut check.notifications.undetermined);
    ids.into_iter().all(|value| id(value, evidence)) && check.redact(evidence)
}
impl Client {
    /// Reads one synthetic check and its configured request.
    pub async fn get_check(
        &self,
        ctx: &RequestContext,
        input: GetCheckInput,
    ) -> Result<GetCheckOutput, Failure<GetCheckOutput>> {
        check_path_id(GET.id, "CheckID", &input.check_id)?;
        let mut evidence = Evidence::new();
        let raw = self
            .read(ctx, &GET, &[&input.check_id], &[], &mut evidence)
            .await
            .map_err(failure)?;
        let mut check: Option<Check> =
            output(&raw, &Shape::Any, &GET, &evidence).map_err(failure)?;
        let mut check = check.take().unwrap_or_default();
        if !screen_check(&mut check, &evidence) {
            return Err(failure(GET.invalid(&evidence)));
        }
        Ok(GetCheckOutput { check })
    }
    /// Lists synthetic checks in provider order.
    pub async fn list_checks(
        &self,
        ctx: &RequestContext,
        _: ListChecksInput,
    ) -> Result<ListChecksOutput, Failure<ListChecksOutput>> {
        let mut evidence = Evidence::new();
        let raw = self
            .read(ctx, &LIST, &[], &[], &mut evidence)
            .await
            .map_err(failure)?;
        let mut items = output::<Field<Vec<Check>>>(&raw, &Shape::Any, &LIST, &evidence)
            .map_err(failure)?
            .0;
        if !items.iter_mut().all(|item| screen_check(item, &evidence)) {
            return Err(failure(LIST.invalid(&evidence)));
        }
        Ok(List { items })
    }
    /// Lists probe locations.
    pub async fn list_locations(
        &self,
        ctx: &RequestContext,
        _: ListLocationsInput,
    ) -> Result<ListLocationsOutput, Failure<ListLocationsOutput>> {
        let mut evidence = Evidence::new();
        let raw = self
            .read(ctx, &LOCATIONS, &[], &[], &mut evidence)
            .await
            .map_err(failure)?;
        let mut items = output::<Field<Vec<Location>>>(&raw, &Shape::Any, &LOCATIONS, &evidence)
            .map_err(failure)?
            .0;
        if !items
            .iter_mut()
            .all(|item| id(&mut item.id, &evidence) && item.redact(&evidence))
        {
            return Err(failure(LOCATIONS.invalid(&evidence)));
        }
        Ok(List { items })
    }
}
