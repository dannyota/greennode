use clap::ArgMatches;
use greennode_sdk::monitor as sdk;
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::cli::dispatch::{self, OutputView};
use crate::cli::input::{InputDescriptor, InputError, InputField, InputGuard, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

#[derive(Default)]
struct Input {
    check_id: String,
    name: Option<String>,
    url: Option<String>,
    method: Option<String>,
    body: Option<String>,
    timeout: Option<i64>,
    test_frequency: Option<i64>,
    tests: Option<i64>,
    failed_locations: Option<i64>,
    locations: Option<Vec<String>>,
    headers: Option<BTreeMap<String, String>>,
    query: Option<BTreeMap<String, String>>,
    assertions: Option<Vec<sdk::Assertion>>,
    notifications: Option<sdk::CheckNotifications>,
    in_alarm: Option<Vec<String>>,
    up: Option<Vec<String>>,
    undetermined: Option<Vec<String>>,
}
const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}
const ID: Field = flag("check-id", "string", true);
const NAME: Field = flag("name", "string", true);
const URL: Field = flag("url", "string", true);
const LOCATIONS: Field = flag("locations", "[]string", true);
const METHOD: Field = flag("method", "string", false);
const BODY: Field = flag("body", "string", false);
const TIMEOUT: Field = flag("timeout", "int", false);
const FREQUENCY: Field = flag("test-frequency", "int", false);
const TESTS: Field = flag("tests", "int", false);
const FAILED: Field = flag("failed-locations", "int", false);
const IN_ALARM: Field = flag("in-alarm-channel-id", "stringArray", false);
const UP: Field = flag("up-channel-id", "stringArray", false);
const UNDETERMINED: Field = flag("undetermined-channel-id", "stringArray", false);
const NOTIFY_FLAGS: &[Field] = &[IN_ALARM, UNDETERMINED, UP];
const CREATE_FIELDS: &[Field] = &[
    NAME, URL, LOCATIONS, METHOD, BODY, TIMEOUT, FREQUENCY, TESTS, FAILED,
];
const UPDATE_FIELDS: &[Field] = &[
    ID,
    flag("name", "*string", false),
    flag("url", "*string", false),
    flag("method", "*string", false),
    flag("body", "*string", false),
    flag("timeout", "*int", false),
    flag("test-frequency", "*int", false),
    flag("tests", "*int", false),
    flag("failed-locations", "*int", false),
    flag("locations", "*[]string", false),
];
const HEADERS: Field = flag("Headers", "map[string]string", false);
const QUERY: Field = flag("Query", "map[string]string", false);
const ASSERTIONS: Field = flag("Assertions", "[]monitor.Assertion", false);
const NOTIFICATIONS: Field = flag("Notifications", "monitor.CheckNotifications", false);
const CREATE_JSON: &[Field] = &[HEADERS, QUERY, ASSERTIONS, NOTIFICATIONS];
const UPDATE_JSON: &[Field] = &[
    flag("Headers", "*map[string]string", false),
    flag("Query", "*map[string]string", false),
    flag("Assertions", "*[]monitor.Assertion", false),
    flag("Notifications", "*monitor.CheckNotifications", false),
];

const ID_INPUT: InputField<Input> = InputField {
    json_name: "CheckID",
    flag: Some(ID),
    secret: false,
    nullable: false,
    set_json: |input, value| {
        input.check_id = serde_json::from_value(value).map_err(|_| ())?;
        Ok(())
    },
    set_flag: Some(|input, matches| {
        input.check_id = matches.get_one::<String>(ID.name).cloned().ok_or(())?;
        Ok(())
    }),
    requirement: Requirement::Nonzero(|input| !input.check_id.is_empty()),
};
macro_rules! optional {
    ($field:ident,$json:literal,$flag:ident,$rust:ty,$requirement:expr) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = Some(serde_json::from_value(value).map_err(|_| ())?);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = matches.get_one::<$rust>($flag.name).cloned();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}
const REQ_NAME: InputField<Input> = optional!(
    name,
    "Name",
    NAME,
    String,
    Requirement::Nonzero(|input| input.name.as_deref().is_some_and(|text| !text.is_empty()))
);
const REQ_URL: InputField<Input> = optional!(
    url,
    "URL",
    URL,
    String,
    Requirement::Nonzero(|input| input.url.as_deref().is_some_and(|text| !text.is_empty()))
);
const LOCATIONS_INPUT: InputField<Input> = InputField {
    json_name: "Locations",
    flag: Some(LOCATIONS),
    secret: false,
    nullable: false,
    set_json: |input, value| {
        input.locations = Some(serde_json::from_value(value).map_err(|_| ())?);
        Ok(())
    },
    set_flag: Some(|input, matches| {
        let values = matches.get_many::<String>(LOCATIONS.name).ok_or(())?;
        input.locations = Some(values.cloned().collect());
        Ok(())
    }),
    requirement: Requirement::Optional,
};

macro_rules! json_only {
    ($field:ident,$name:literal,$map:expr) => {
        InputField {
            json_name: $name,
            flag: None,
            secret: false,
            nullable: false,
            set_flag: None,
            requirement: Requirement::Optional,
            set_json: |input, value| {
                input.$field = Some(($map)(serde_json::from_value(value).map_err(|_| ())?));
                Ok(())
            },
        }
    };
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Notifications {
    #[serde(rename = "In-alarm")]
    in_alarm: Option<Vec<String>>,
    #[serde(rename = "Up")]
    up: Option<Vec<String>>,
    #[serde(rename = "Undetermined")]
    undetermined: Option<Vec<String>>,
}
impl From<Notifications> for sdk::CheckNotifications {
    fn from(value: Notifications) -> Self {
        Self {
            in_alarm: value.in_alarm.unwrap_or_default(),
            up: value.up.unwrap_or_default(),
            undetermined: value.undetermined.unwrap_or_default(),
        }
    }
}
const HEADERS_INPUT: InputField<Input> = json_only!(headers, "Headers", std::convert::identity);
const QUERY_INPUT: InputField<Input> = json_only!(query, "Query", std::convert::identity);
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Assertion {
    r#type: String,
    operator: String,
    target: String,
}
const ASSERTIONS_INPUT: InputField<Input> = InputField {
    set_json: |input, value| {
        let assertions: Vec<Assertion> = serde_json::from_value(value).map_err(|_| ())?;
        input.assertions = Some(
            assertions
                .into_iter()
                .map(|assertion| sdk::Assertion {
                    r#type: assertion.r#type,
                    operator: assertion.operator,
                    target: assertion.target,
                })
                .collect(),
        );
        Ok(())
    },
    ..json_only!(assertions, "Assertions", std::convert::identity)
};
const NOTIFICATIONS_INPUT: InputField<Input> = InputField {
    set_json: |input, value| {
        let notifications: Notifications = serde_json::from_value(value).map_err(|_| ())?;
        input.notifications = Some(notifications.into());
        Ok(())
    },
    ..json_only!(notifications, "Notifications", |value: Notifications| value
        .into())
};

macro_rules! occurrence {
    ($member:ident,$flag:ident,$name:literal) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            requirement: Requirement::Optional,
            set_json: |_, _| Err(()),
            set_flag: Some(|input, matches| {
                input.$member = matches
                    .get_many::<String>($flag.name)
                    .map(|items| items.cloned().collect());
                Ok(())
            }),
        }
    };
}
const IN_ALARM_INPUT: InputField<Input> = occurrence!(in_alarm, IN_ALARM, "InAlarmFlag");
const UP_INPUT: InputField<Input> = occurrence!(up, UP, "UpFlag");
const UND_INPUT: InputField<Input> = occurrence!(undetermined, UNDETERMINED, "UndeterminedFlag");
const fn descriptor(
    fields: &'static [InputField<Input>],
    guard: Option<InputGuard<Input>>,
) -> InputDescriptor<Input> {
    InputDescriptor {
        defaults: Input::default,
        fields,
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: None,
        guard,
    }
}
const CREATE_INPUT: InputDescriptor<Input> = descriptor(
    &[
        REQ_NAME,
        REQ_URL,
        InputField {
            requirement: Requirement::Nonzero(|input| {
                input
                    .locations
                    .as_ref()
                    .is_some_and(|values| !values.is_empty())
            }),
            ..LOCATIONS_INPUT
        },
        optional!(method, "Method", METHOD, String, Requirement::Optional),
        optional!(body, "Body", BODY, String, Requirement::Optional),
        optional!(timeout, "Timeout", TIMEOUT, i64, Requirement::Optional),
        optional!(
            test_frequency,
            "TestFrequency",
            FREQUENCY,
            i64,
            Requirement::Optional
        ),
        optional!(tests, "Tests", TESTS, i64, Requirement::Optional),
        optional!(
            failed_locations,
            "FailedLocations",
            FAILED,
            i64,
            Requirement::Optional
        ),
        HEADERS_INPUT,
        QUERY_INPUT,
        ASSERTIONS_INPUT,
        NOTIFICATIONS_INPUT,
        IN_ALARM_INPUT,
        UP_INPUT,
        UND_INPUT,
    ],
    Some(create_guard),
);
const UPDATE_INPUT: InputDescriptor<Input> = descriptor(
    &[
        ID_INPUT,
        InputField {
            requirement: Requirement::Optional,
            ..REQ_NAME
        },
        InputField {
            requirement: Requirement::Optional,
            ..REQ_URL
        },
        LOCATIONS_INPUT,
        optional!(method, "Method", METHOD, String, Requirement::Optional),
        optional!(body, "Body", BODY, String, Requirement::Optional),
        optional!(timeout, "Timeout", TIMEOUT, i64, Requirement::Optional),
        optional!(
            test_frequency,
            "TestFrequency",
            FREQUENCY,
            i64,
            Requirement::Optional
        ),
        optional!(tests, "Tests", TESTS, i64, Requirement::Optional),
        optional!(
            failed_locations,
            "FailedLocations",
            FAILED,
            i64,
            Requirement::Optional
        ),
        HEADERS_INPUT,
        QUERY_INPUT,
        ASSERTIONS_INPUT,
        NOTIFICATIONS_INPUT,
        IN_ALARM_INPUT,
        UP_INPUT,
        UND_INPUT,
    ],
    Some(update_guard),
);
const ID_ONLY: InputDescriptor<Input> = descriptor(&[ID_INPUT], None);

fn create_guard(input: &Input, _: &ArgMatches) -> Result<(), InputError> {
    guard_lists(input)
}
fn update_guard(input: &Input, _: &ArgMatches) -> Result<(), InputError> {
    guard_lists(input)?;
    let present = [&input.in_alarm, &input.up, &input.undetermined]
        .into_iter()
        .filter(|value| value.is_some())
        .count();
    if present > 0 && present < 3 {
        return Err(InputError(
            "update-check requires all three notification channel ID flags or none",
        ));
    }
    Ok(())
}
fn guard_lists(input: &Input) -> Result<(), InputError> {
    for (values, empty, whitespace) in [
        (
            &input.in_alarm,
            "--in-alarm-channel-id cannot mix an empty value with other values",
            "--in-alarm-channel-id cannot contain a whitespace-only ID",
        ),
        (
            &input.up,
            "--up-channel-id cannot mix an empty value with other values",
            "--up-channel-id cannot contain a whitespace-only ID",
        ),
        (
            &input.undetermined,
            "--undetermined-channel-id cannot mix an empty value with other values",
            "--undetermined-channel-id cannot contain a whitespace-only ID",
        ),
    ] {
        if let Some(values) = values {
            for value in values {
                if value.is_empty() {
                    if values.len() != 1 {
                        return Err(InputError(empty));
                    }
                } else if value.trim().is_empty() {
                    return Err(InputError(whitespace));
                }
            }
        }
    }
    Ok(())
}
fn notifications(input: &Input) -> Option<sdk::CheckNotifications> {
    if input.notifications.is_none()
        && input.in_alarm.is_none()
        && input.up.is_none()
        && input.undetermined.is_none()
    {
        return None;
    }
    let mut result = input.notifications.clone().unwrap_or_default();
    for (source, target) in [
        (&input.in_alarm, &mut result.in_alarm),
        (&input.up, &mut result.up),
        (&input.undetermined, &mut result.undetermined),
    ] {
        if let Some(source) = source {
            target.clear();
            target.extend(source.iter().filter(|id| !id.is_empty()).cloned());
        }
    }
    Some(result)
}

macro_rules! write {
    ($name:literal,$go:literal,$title:literal,$method:ident,$desc:ident,$kind:ident,$flags:expr,$json:expr,$extra:expr,$input:expr,$notes:expr,$example:literal) => {
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
            extra_flags: $extra,
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
                    Some(super::write_input::CHECK),
                ))
            })),
        }
    };
}
const NOTIFICATION_NOTES: &str = "Repeat --in-alarm-channel-id, --up-channel-id, and --undetermined-channel-id for separate channel IDs. One empty occurrence clears its list. For updates, provide all three flags together or use JSON Notifications with In-alarm, Up, and Undetermined.";
pub(super) const CREATE: Operation = write!(
    "create-check",
    "CreateCheck",
    "Create a synthetic HTTP check",
    create_check,
    CREATE_INPUT,
    Write,
    CREATE_FIELDS,
    CREATE_JSON,
    NOTIFY_FLAGS,
    create,
    &[NOTIFICATION_NOTES],
    "create-check --name <name> --url <url> --locations <locations>"
);
pub(super) const UPDATE: Operation = write!(
    "update-check",
    "UpdateCheck",
    "Update a synthetic HTTP check",
    update_check,
    UPDATE_INPUT,
    Write,
    UPDATE_FIELDS,
    UPDATE_JSON,
    NOTIFY_FLAGS,
    update,
    &[
        NOTIFICATION_NOTES,
        "Updates preserve the current enabled or disabled state. Pause, resume, and update serialize by CheckID within one SDK client."
    ],
    "update-check --check-id <check-id> --name <name>"
);
pub(super) const DELETE: Operation = write!(
    "delete-check",
    "DeleteCheck",
    "Delete a synthetic check",
    delete_check,
    ID_ONLY,
    DestructiveWrite,
    &[ID],
    &[],
    &[],
    |input: &Input| sdk::DeleteCheckInput::new(&input.check_id),
    &["Requires --yes. Deleting a check removes its history."],
    "delete-check --check-id <check-id> --yes"
);
pub(super) const PAUSE: Operation = write!(
    "pause-check",
    "PauseCheck",
    "Pause a synthetic check",
    pause_check,
    ID_ONLY,
    Write,
    &[ID],
    &[],
    &[],
    |input: &Input| sdk::PauseCheckInput::new(&input.check_id),
    &[
        "Toggles the check once when enabled and reads the result back. Changed is false for an already-paused check."
    ],
    "pause-check --check-id <check-id>"
);
pub(super) const RESUME: Operation = write!(
    "resume-check",
    "ResumeCheck",
    "Resume a synthetic check",
    resume_check,
    ID_ONLY,
    Write,
    &[ID],
    &[],
    &[],
    |input: &Input| sdk::ResumeCheckInput::new(&input.check_id),
    &[
        "Toggles the check once when disabled and reads the result back. Changed is false for an already-enabled check."
    ],
    "resume-check --check-id <check-id>"
);

fn create(input: &Input) -> sdk::CreateCheckInput {
    let mut value = sdk::CreateCheckInput::new(
        input.name.as_deref().unwrap_or_default(),
        input.url.as_deref().unwrap_or_default(),
        input.locations.clone().unwrap_or_default(),
    );
    value.method = input.method.clone().unwrap_or_default();
    value.body = input.body.clone().unwrap_or_default();
    value.headers = input.headers.clone().unwrap_or_default();
    value.query = input.query.clone().unwrap_or_default();
    value.assertions = input.assertions.clone().unwrap_or_default();
    value.timeout = input.timeout.unwrap_or_default();
    value.test_frequency = input.test_frequency.unwrap_or_default();
    value.tests = input.tests.unwrap_or_default();
    value.failed_locations = input.failed_locations.unwrap_or_default();
    value.notifications = notifications(input).unwrap_or_default();
    value
}
fn update(input: &Input) -> sdk::UpdateCheckInput {
    let mut value = sdk::UpdateCheckInput::new(&input.check_id);
    value.name.clone_from(&input.name);
    value.url.clone_from(&input.url);
    value.method.clone_from(&input.method);
    value.body.clone_from(&input.body);
    value.headers.clone_from(&input.headers);
    value.query.clone_from(&input.query);
    value.locations.clone_from(&input.locations);
    value.assertions.clone_from(&input.assertions);
    value.timeout = input.timeout;
    value.test_frequency = input.test_frequency;
    value.tests = input.tests;
    value.failed_locations = input.failed_locations;
    value.notifications = notifications(input);
    value
}
