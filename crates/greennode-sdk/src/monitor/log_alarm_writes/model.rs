use super::{CREATE, UPDATE};
use crate::Error;
use crate::monitor::inventory::get_input;
use crate::monitor::log_alarm_body::{default_filter, default_resend, usage};
use crate::monitor::{
    Alarm, AlarmFilter, LogAlarmDetail, LogAlarmResend, LogProject, ResponseField,
};
use serde::Serialize;

/// Selects the settings for a new log alarm.
#[derive(Clone, Debug)]
pub struct CreateLogAlarmInput {
    /// Name of the new alarm.
    pub name: String,
    /// Log project identifier.
    pub log_project_id: String,
    /// Threshold, including zero.
    pub threshold_value: f64,
    /// Alarm description.
    pub description: String,
    /// Severity. Empty selects LOW.
    pub severity: String,
    /// Search query, paired with the filter.
    pub query_string: String,
    /// Provider filter, paired with the query.
    pub filter: AlarmFilter,
    /// Threshold type. Empty selects frequency.
    pub threshold_type: String,
    /// Threshold comparison.
    pub condition: String,
    /// Evaluation window in minutes. Zero selects five.
    pub time_frame: i64,
    /// Grouping field.
    pub group_by_field: String,
    /// Aggregation field.
    pub agg_field: String,
    /// Aggregation type.
    pub agg_type: String,
    /// Channels notified when the alarm fires.
    pub in_alarm: Vec<String>,
    /// Channels notified when the alarm clears.
    pub ok: Vec<String>,
    /// Resend configuration, if overriding the console defaults.
    pub resend: Option<LogAlarmResend>,
    /// Skips settlement after the write succeeds.
    pub no_wait: bool,
}
impl CreateLogAlarmInput {
    /// Supplies the alarm name, project identifier and threshold.
    pub fn new(
        name: impl Into<String>,
        log_project_id: impl Into<String>,
        threshold_value: f64,
    ) -> Self {
        Self {
            name: name.into(),
            log_project_id: log_project_id.into(),
            threshold_value,
            description: String::new(),
            severity: String::new(),
            query_string: String::new(),
            filter: AlarmFilter::default(),
            threshold_type: String::new(),
            condition: String::new(),
            time_frame: 0,
            group_by_field: String::new(),
            agg_field: String::new(),
            agg_type: String::new(),
            in_alarm: Vec::new(),
            ok: Vec::new(),
            resend: None,
            no_wait: false,
        }
    }
}

/// Settings to replace on an existing log alarm.
#[derive(Clone, Debug)]
pub struct UpdateLogAlarmInput {
    /// Alarm identifier.
    pub alarm_id: String,
    /// Skips settlement after the write succeeds.
    pub no_wait: bool,
    /// Replacement display name.
    pub name: Option<String>,
    /// Replacement description.
    pub description: Option<String>,
    /// Replacement severity.
    pub severity: Option<String>,
    /// Replacement log project identifier.
    pub log_project_id: Option<String>,
    /// Replacement search query, paired with the filter.
    pub query_string: Option<String>,
    /// Replacement filter, paired with the query.
    pub filter: Option<AlarmFilter>,
    /// Replacement threshold type.
    pub threshold_type: Option<String>,
    /// Replacement comparison.
    pub condition: Option<String>,
    /// Replacement threshold.
    pub threshold_value: Option<f64>,
    /// Replacement evaluation window in minutes.
    pub time_frame: Option<i64>,
    /// Replacement grouping field.
    pub group_by_field: Option<String>,
    /// Replacement aggregation field.
    pub agg_field: Option<String>,
    /// Replacement aggregation type.
    pub agg_type: Option<String>,
    /// Replacement channels notified when the alarm fires.
    pub in_alarm: Option<Vec<String>>,
    /// Replacement channels notified when the alarm clears.
    pub ok: Option<Vec<String>>,
    /// Replacement resend configuration.
    pub resend: Option<LogAlarmResend>,
}
impl UpdateLogAlarmInput {
    /// Selects the alarm to update while leaving every patch field unset.
    pub fn new(alarm_id: impl Into<String>) -> Self {
        Self {
            alarm_id: alarm_id.into(),
            no_wait: false,
            name: None,
            description: None,
            severity: None,
            log_project_id: None,
            query_string: None,
            filter: None,
            threshold_type: None,
            condition: None,
            threshold_value: None,
            time_frame: None,
            group_by_field: None,
            agg_field: None,
            agg_type: None,
            in_alarm: None,
            ok: None,
            resend: None,
        }
    }
}
get_input!(
    /// Selects one log alarm to delete.
    DeleteLogAlarmInput,
    /// Alarm identifier.
    alarm_id
);

/// Result of creating a log alarm.
#[derive(Clone, Debug, Serialize)]
pub struct CreateLogAlarmOutput {
    /// ID returned by create or settlement.
    #[serde(rename = "AlarmID")]
    pub alarm_id: String,
    /// Observed alarm, empty when settlement was skipped.
    #[serde(rename = "Alarm")]
    pub alarm: Alarm,
}
/// Result of updating a log alarm.
#[derive(Clone, Debug, Serialize)]
pub struct UpdateLogAlarmOutput {
    /// Observed alarm, empty when settlement was skipped.
    #[serde(rename = "Alarm")]
    pub alarm: Alarm,
}
/// Acknowledgement that a log alarm was deleted.
#[derive(Clone, Debug, Serialize)]
pub struct DeleteLogAlarmOutput {}

impl CreateLogAlarmInput {
    pub(super) fn resolved(self) -> Result<(Alarm, LogAlarmDetail, bool), Error> {
        let alarm = Alarm {
            name: self.name,
            description: self.description,
            severity: default(self.severity, "LOW"),
            ..Default::default()
        };
        let threshold_type = default(self.threshold_type, "frequency");
        let condition = default(
            self.condition,
            if threshold_type == "flatline" {
                "lt"
            } else {
                "gt"
            },
        )
        .to_ascii_lowercase();
        let (query_string, filter) = if self.query_string.is_empty() && self.filter.0.is_none() {
            (
                "*".into(),
                default_filter("").map_err(|_| usage(CREATE.id, "Filter cannot be encoded"))?,
            )
        } else {
            (self.query_string, self.filter)
        };
        let detail = LogAlarmDetail {
            log_project_id: self.log_project_id,
            query_string,
            filter,
            threshold_type,
            condition,
            threshold_value: ResponseField::Value(self.threshold_value),
            time_frame: ResponseField::Value(if self.time_frame == 0 {
                5
            } else {
                self.time_frame
            }),
            group_by_field: self.group_by_field,
            agg_field: self.agg_field,
            agg_type: self.agg_type,
            in_alarm: self.in_alarm,
            ok: self.ok,
            resend: self.resend.unwrap_or_else(default_resend),
            raw_log_search_query: "[]".into(),
            ..Default::default()
        };
        Ok((alarm, detail, self.no_wait))
    }
}
impl UpdateLogAlarmInput {
    pub(super) fn apply(
        self,
        alarm: &mut Alarm,
        detail: &mut LogAlarmDetail,
        project: Option<LogProject>,
    ) -> Result<(), Error> {
        let changed_query = self.query_string.is_some();
        macro_rules! replace {($($field:ident=>$target:expr),+)=>{$(if let Some(value)=self.$field{$target=value;})+};}
        replace!(name=>alarm.name,description=>alarm.description,severity=>alarm.severity,query_string=>detail.query_string,filter=>detail.filter,
            threshold_type=>detail.threshold_type,condition=>detail.condition,group_by_field=>detail.group_by_field,agg_field=>detail.agg_field,agg_type=>detail.agg_type,
            in_alarm=>detail.in_alarm,ok=>detail.ok,resend=>detail.resend);
        if let Some(project) = project {
            detail.log_project_id = project.id;
            detail.log_project_name = project.project_name;
        }
        if let Some(threshold) = self.threshold_value {
            detail.threshold_value = ResponseField::Value(threshold);
        }
        if let Some(frame) = self.time_frame {
            detail.time_frame = ResponseField::Value(frame);
        }
        if changed_query {
            detail.raw_log_search_query = "[]".into();
            if detail.query_string.is_empty() && detail.filter.0.is_none() {
                detail.query_string = "*".into();
            }
        }
        if detail.filter.0.is_none() {
            detail.filter = default_filter(&detail.query_string)
                .map_err(|_| usage(UPDATE.id, "Filter cannot be encoded"))?;
        }
        if detail.resend.statuses.is_empty() {
            detail.resend.statuses.push("ALARM".into());
        }
        detail.condition.make_ascii_lowercase();
        Ok(())
    }
}
fn default(value: String, fallback: &str) -> String {
    if value.is_empty() {
        fallback.into()
    } else {
        value
    }
}
