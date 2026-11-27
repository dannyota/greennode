use crate::cli::dispatch::{self, OutputView};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use greennode_sdk::monitor as sdk;
use serde::Deserialize;

#[derive(Default)]
struct Input {
    alarm_id: Option<String>,
    name: Option<String>,
    log_project_id: Option<String>,
    threshold_value: Option<f64>,
    description: Option<String>,
    severity: Option<String>,
    query_string: Option<String>,
    threshold_type: Option<String>,
    condition: Option<String>,
    time_frame: Option<i64>,
    group_by_field: Option<String>,
    agg_field: Option<String>,
    agg_type: Option<String>,
    in_alarm: Option<Vec<String>>,
    ok: Option<Vec<String>>,
    no_wait: Option<bool>,
    filter: Option<sdk::AlarmFilter>,
    resend: Option<sdk::LogAlarmResend>,
}

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}
const ID: Field = flag("alarm-id", "string", true);
const NAME: Field = flag("name", "string", true);
const PROJECT_ID: Field = flag("log-project-id", "string", true);
const THRESHOLD: Field = flag("threshold-value", "*float64", true);
const DESCRIPTION: Field = flag("description", "string", false);
const SEVERITY: Field = flag("severity", "string", false);
const QUERY: Field = flag("query-string", "string", false);
const THRESHOLD_TYPE: Field = flag("threshold-type", "string", false);
const CONDITION: Field = flag("condition", "string", false);
const TIME_FRAME: Field = flag("time-frame", "int", false);
const GROUP: Field = flag("group-by-field", "string", false);
const AGG_FIELD: Field = flag("agg-field", "string", false);
const AGG_TYPE: Field = flag("agg-type", "string", false);
const IN_ALARM: Field = flag("in-alarm", "[]string", false);
const OK: Field = flag("ok", "[]string", false);
const NO_WAIT: Field = flag("no-wait", "bool", false);
const CREATE_FLAGS: &[Field] = &[
    NAME,
    PROJECT_ID,
    THRESHOLD,
    DESCRIPTION,
    SEVERITY,
    QUERY,
    THRESHOLD_TYPE,
    CONDITION,
    TIME_FRAME,
    GROUP,
    AGG_FIELD,
    AGG_TYPE,
    IN_ALARM,
    OK,
    NO_WAIT,
];
const UPDATE_FLAGS: &[Field] = &[
    ID,
    NO_WAIT,
    flag("name", "*string", false),
    flag("description", "*string", false),
    flag("severity", "*string", false),
    flag("log-project-id", "*string", false),
    flag("query-string", "*string", false),
    flag("threshold-type", "*string", false),
    flag("condition", "*string", false),
    flag("threshold-value", "*float64", false),
    flag("time-frame", "*int", false),
    flag("group-by-field", "*string", false),
    flag("agg-field", "*string", false),
    flag("agg-type", "*string", false),
    flag("in-alarm", "*[]string", false),
    flag("ok", "*[]string", false),
];
const CREATE_JSON: &[Field] = &[
    flag("Filter", "jsontext.Value", false),
    flag("Resend", "*monitor.LogAlarmResend", false),
];
const UPDATE_JSON: &[Field] = &[
    flag("Filter", "*jsontext.Value", false),
    flag("Resend", "*monitor.LogAlarmResend", false),
];

macro_rules! slot {
    ($json:literal,$flag:ident,$field:ident,$rust:ty,$required:expr) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            requirement: $required,
            set_json: |input, value| {
                input.$field = Some(serde_json::from_value(value).map_err(|_| ())?);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = matches.get_one::<$rust>($flag.name).cloned();
                Ok(())
            }),
        }
    };
    ($json:literal,$flag:ident,$field:ident,$rust:ty) => {
        slot!($json, $flag, $field, $rust, Requirement::Optional)
    };
}
macro_rules! text_required {
    ($json:literal,$flag:ident,$member:ident) => {
        slot!(
            $json,
            $flag,
            $member,
            String,
            Requirement::Nonzero(|input| input
                .$member
                .as_deref()
                .is_some_and(|value| !value.is_empty()))
        )
    };
}
macro_rules! channels {
    ($json:literal,$flag:ident,$member:ident) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            requirement: Requirement::Optional,
            set_json: |input, value| {
                input.$member = Some(serde_json::from_value(value).map_err(|_| ())?);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                let values = matches.get_many::<String>($flag.name).ok_or(())?;
                input.$member = Some(values.cloned().collect());
                Ok(())
            }),
        }
    };
}
const ID_INPUT: InputField<Input> = text_required!("AlarmID", ID, alarm_id);
const NAME_INPUT: InputField<Input> = text_required!("Name", NAME, name);
const PROJECT_INPUT: InputField<Input> = text_required!("LogProjectID", PROJECT_ID, log_project_id);
const THRESHOLD_INPUT: InputField<Input> = slot!(
    "ThresholdValue",
    THRESHOLD,
    threshold_value,
    f64,
    Requirement::Presence
);

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase", deny_unknown_fields)]
struct Resend {
    enabled: bool,
    statuses: Vec<String>,
    period: i64,
    times: i64,
}
const RESEND_INPUT: InputField<Input> = InputField {
    json_name: "Resend",
    flag: None,
    secret: false,
    nullable: false,
    set_flag: None,
    requirement: Requirement::Optional,
    set_json: |input, value| {
        let resend: Resend = serde_json::from_value(value).map_err(|_| ())?;
        input.resend = Some(sdk::LogAlarmResend {
            enabled: sdk::ResponseField::Value(resend.enabled),
            statuses: resend.statuses,
            period: sdk::ResponseField::Value(resend.period),
            times: sdk::ResponseField::Value(resend.times),
        });
        Ok(())
    },
};
const FILTER_INPUT: InputField<Input> = InputField {
    json_name: "Filter",
    flag: None,
    secret: false,
    nullable: false,
    set_flag: None,
    requirement: Requirement::Optional,
    set_json: |input, value| {
        if !value.is_object() {
            return Err(());
        }
        input.filter = Some(sdk::AlarmFilter(Some(
            serde_json::value::to_raw_value(&value).map_err(|_| ())?,
        )));
        Ok(())
    },
};

const CREATE_FILTER_INPUT: InputField<Input> = InputField {
    nullable: true,
    ..FILTER_INPUT
};

const fn descriptor(fields: &'static [InputField<Input>]) -> InputDescriptor<Input> {
    InputDescriptor {
        defaults: Input::default,
        fields,
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: None,
        guard: None,
    }
}
macro_rules! flags_input {
    ($filter:ident; $name:ident,$project:ident,$threshold:ident $(,$extra:expr)?)=>{
        [$($extra,)?$name,$project,$threshold,
         slot!("Description",DESCRIPTION,description,String),slot!("Severity",SEVERITY,severity,String),slot!("QueryString",QUERY,query_string,String),
         slot!("ThresholdType",THRESHOLD_TYPE,threshold_type,String),slot!("Condition",CONDITION,condition,String),slot!("TimeFrame",TIME_FRAME,time_frame,i64),
         slot!("GroupByField",GROUP,group_by_field,String),slot!("AggField",AGG_FIELD,agg_field,String),slot!("AggType",AGG_TYPE,agg_type,String),
         channels!("InAlarm",IN_ALARM,in_alarm),channels!("OK",OK,ok),slot!("NoWait",NO_WAIT,no_wait,bool),RESEND_INPUT,$filter]
    };
}
const CREATE_INPUT: InputDescriptor<Input> =
    descriptor(&flags_input!(CREATE_FILTER_INPUT; NAME_INPUT, PROJECT_INPUT, THRESHOLD_INPUT));
const UPDATE_NAME: InputField<Input> = slot!("Name", NAME, name, String);
const UPDATE_PROJECT: InputField<Input> = slot!("LogProjectID", PROJECT_ID, log_project_id, String);
const UPDATE_THRESHOLD: InputField<Input> =
    slot!("ThresholdValue", THRESHOLD, threshold_value, f64);
const UPDATE_INPUT: InputDescriptor<Input> = descriptor(&flags_input!(
    FILTER_INPUT; UPDATE_NAME,
    UPDATE_PROJECT,
    UPDATE_THRESHOLD,
    ID_INPUT
));
const DELETE_INPUT: InputDescriptor<Input> = descriptor(&[ID_INPUT]);

macro_rules! write {
    ($name:literal,$go:literal,$title:literal,$method:ident,$desc:ident,$kind:ident,$flags:expr,$json:expr,$input:expr,$notes:expr,$example:literal) => {
        Operation {
            name: $name,
            sdk_operation: concat!("monitor.", $go),
            short_help: $title,
            example: Some(concat!("greennode monitor ", $example)),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::$kind,
            flags: $flags,
            json_fields: $json,
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&$desc, |operation, matches, invocation| {
                Box::pin(dispatch::management_custom(
                    invocation,
                    operation,
                    matches,
                    &$desc,
                    |ctx, config, input| {
                        Box::pin(async move {
                            dispatch::project_result(
                                sdk::Client::new(config).$method(ctx, ($input)(input)).await,
                                std::convert::identity,
                            )
                        })
                    },
                    OutputView::default(),
                    Some(super::write_input::ALARM),
                ))
            })),
        }
    };
}
pub(super) const CREATE: Operation = write!(
    "create-log-alarm",
    "CreateLogAlarm",
    "Create a log alarm",
    create_log_alarm,
    CREATE_INPUT,
    Write,
    CREATE_FLAGS,
    CREATE_JSON,
    create,
    &[
        "The exact name search must finish every page with consistent counts and identities before creating the alarm. A duplicate name exits 2. Supply QueryString and a JSON Filter together, or leave both empty for a match-all query. A nonfinite threshold exits 2 before requests. Without --no-wait the SDK polls for ACTIVE for up to 120 seconds. An unconfirmed write must be reconciled by reading before another create."
    ],
    "create-log-alarm --name <name> --log-project-id <log-project-id> --threshold-value <threshold-value>"
);
pub(super) const UPDATE: Operation = write!(
    "update-log-alarm",
    "UpdateLogAlarm",
    "Update a log alarm",
    update_log_alarm,
    UPDATE_INPUT,
    Write,
    UPDATE_FLAGS,
    UPDATE_JSON,
    update,
    &[
        "Updates only ACTIVE log alarms. Every omitted field retains its current value. A supplied QueryString requires a supplied Filter and vice versa. Until settlement can confirm ACTIVE, an uncertain write must be reconciled with a read."
    ],
    "update-log-alarm --alarm-id <alarm-id> --description <description>"
);
pub(super) const DELETE: Operation = write!(
    "delete-log-alarm",
    "DeleteLogAlarm",
    "Delete a log alarm",
    delete_log_alarm,
    DELETE_INPUT,
    DestructiveWrite,
    &[ID],
    &[],
    |input: &Input| sdk::DeleteLogAlarmInput::new(input.alarm_id.as_deref().unwrap_or_default()),
    &[
        "Requires --yes. The SDK verifies the alarm kind before deleting it. Deleting an alarm removes its history."
    ],
    "delete-log-alarm --alarm-id <alarm-id> --yes"
);

fn create(input: &Input) -> sdk::CreateLogAlarmInput {
    let mut value = sdk::CreateLogAlarmInput::new(
        input.name.as_deref().unwrap_or_default(),
        input.log_project_id.as_deref().unwrap_or_default(),
        input.threshold_value.unwrap_or_default(),
    );
    macro_rules! assign {($($field:ident),+)=>{$(value.$field=input.$field.clone().unwrap_or_default();)+};}
    assign!(
        description,
        severity,
        query_string,
        threshold_type,
        condition,
        group_by_field,
        agg_field,
        agg_type,
        in_alarm,
        ok,
        filter
    );
    value.time_frame = input.time_frame.unwrap_or_default();
    value.no_wait = input.no_wait.unwrap_or_default();
    value.resend.clone_from(&input.resend);
    value
}
fn update(input: &Input) -> sdk::UpdateLogAlarmInput {
    let mut value = sdk::UpdateLogAlarmInput::new(input.alarm_id.as_deref().unwrap_or_default());
    macro_rules! copy {($($field:ident),+)=>{$(value.$field.clone_from(&input.$field);)+};}
    copy!(
        name,
        description,
        severity,
        log_project_id,
        query_string,
        threshold_type,
        condition,
        group_by_field,
        agg_field,
        agg_type,
        in_alarm,
        ok,
        filter,
        resend
    );
    value.time_frame = input.time_frame;
    value.threshold_value = input.threshold_value;
    value.no_wait = input.no_wait.unwrap_or_default();
    value
}
