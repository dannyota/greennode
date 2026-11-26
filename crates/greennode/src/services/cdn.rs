use clap::ArgMatches;
use greennode_sdk::cdn::{self, Client, ErrorDetail};

use crate::cli::dispatch;
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

#[derive(Default)]
struct Input {
    cdn_id: String,
    certificate_id: String,
    domains: Vec<String>,
    period: String,
    from: String,
    to: String,
}

impl Input {
    fn range(&self) -> cdn::Range {
        cdn::Range {
            cdn_domains: self.domains.clone(),
            period: self.period.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
        }
    }

    fn report(&self) -> cdn::GetTrafficReportInput {
        cdn::GetTrafficReportInput::new(self.domains.clone(), self.from.clone(), self.to.clone())
    }
}

macro_rules! string_field {
    ($flag:literal, $json:literal, $field:ident, $required:expr, $requirement:expr) => {
        InputField {
            json_name: $json,
            flag: Some(Field {
                name: $flag,
                go_type: "string",
                required: $required,
            }),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = serde_json::from_value(value).map_err(|_| ())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = matches.get_one::<String>($flag).ok_or(())?.clone();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
    ($flag:literal, $json:literal, $field:ident, required) => {
        string_field!(
            $flag,
            $json,
            $field,
            true,
            Requirement::Nonzero(|input| !input.$field.is_empty())
        )
    };
    ($flag:literal, $json:literal, $field:ident) => {
        string_field!($flag, $json, $field, false, Requirement::Optional)
    };
}

const CDN_ID: InputField<Input> = string_field!("cdn-id", "CDNID", cdn_id, required);
const CERTIFICATE_ID: InputField<Input> =
    string_field!("certificate-id", "CertificateID", certificate_id, required);
const PERIOD: InputField<Input> = string_field!("period", "Period", period);
const FROM: InputField<Input> = string_field!("from", "From", from);
const TO: InputField<Input> = string_field!("to", "To", to);
const REPORT_FROM: InputField<Input> = string_field!("from", "From", from, required);
const REPORT_TO: InputField<Input> = string_field!("to", "To", to, required);
const DOMAINS: InputField<Input> = InputField {
    json_name: "CDNDomains",
    flag: None,
    secret: false,
    nullable: false,
    set_json: |input, value| {
        input.domains = serde_json::from_value(value).map_err(|_| ())?;
        Ok(())
    },
    set_flag: None,
    requirement: Requirement::Nonzero(|input| !input.domains.is_empty()),
};
const JSON_DOMAINS: &[Field] = &[Field {
    name: "CDNDomains",
    go_type: "[]string",
    required: true,
}];
const ANALYTICS_NOTES: &[&str] = &[
    "Use generated CDN domains in CDNDomains. Analytics POST requests are allowed in read-only mode.",
];

const fn flag(input: &InputField<Input>) -> Field {
    match input.flag {
        Some(flag) => flag,
        None => panic!("expected a flag"),
    }
}

fn check_input(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

const fn descriptor(fields: &'static [InputField<Input>]) -> InputDescriptor<Input> {
    InputDescriptor {
        defaults: Input::default,
        fields,
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(check_input),
        guard: None,
    }
}

const EMPTY: InputDescriptor<Input> = descriptor(&[]);
const CDN: InputDescriptor<Input> = descriptor(&[CDN_ID]);
const CERTIFICATE: InputDescriptor<Input> = descriptor(&[CERTIFICATE_ID]);
const SERIES: InputDescriptor<Input> = descriptor(&[DOMAINS, PERIOD, FROM, TO]);
const REPORT: InputDescriptor<Input> = descriptor(&[DOMAINS, REPORT_FROM, REPORT_TO]);

macro_rules! read {
    ($name:literal, $sdk:literal, $help:literal, $method:ident, $input:ident, $value:expr, $flags:expr, $json:expr, $notes:expr, $example:expr) => {
        Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            example: $example,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: $flags,
            json_fields: $json,
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation,
                    operation,
                    matches,
                    &$input,
                    |ctx, config, input| {
                        Box::pin(async move {
                            let value = ($value)(input);
                            let result = Client::new(config).$method(ctx, value).await;
                            dispatch::project_result(result, std::convert::identity)
                        })
                    },
                ))
            })),
        }
    };
}

macro_rules! series {
    ($name:literal, $sdk:literal, $help:literal, $method:ident) => {
        read!(
            $name,
            $sdk,
            $help,
            $method,
            SERIES,
            Input::range,
            &[flag(&PERIOD), flag(&FROM), flag(&TO)],
            JSON_DOMAINS,
            ANALYTICS_NOTES,
            Some(concat!(
                "greennode cdn ",
                $name,
                " --cli-input-json '{\"CDNDomains\":[\"<cdn-domain>\"]}' --period 24h"
            ))
        )
    };
}

const fn name(rank: u16, code: &'static str) -> Option<RuleName> {
    Some(RuleName {
        rank,
        code,
        operation: RuleOperation::Omit,
        include_api_status: false,
    })
}

pub(super) const SERVICE: Service = Service {
    name: "cdn",
    error_rules: &[
        Rule {
            identity: "cdn:missing-api-key",
            matches: |error| error.detail::<ErrorDetail>() == Some(&ErrorDetail::MissingApiKey),
            name: None,
            exit: Some(RuleExit::Auth),
            message: Some(
                "no vCDN API key: pipe it to `greennode configure set vcdn_api_key -` or set GREENNODE_VCDN_API_KEY",
            ),
        },
        Rule {
            identity: "cdn:page-format",
            matches: |error| error.detail::<ErrorDetail>() == Some(&ErrorDetail::PageFormat),
            name: name(49, "PageFormat"),
            exit: Some(RuleExit::Failure),
            message: None,
        },
        Rule {
            identity: "cdn:busy",
            matches: |error| error.detail::<ErrorDetail>() == Some(&ErrorDetail::ResourceBusy),
            name: name(3, "ResourceBusy"),
            exit: Some(RuleExit::Failure),
            message: None,
        },
        Rule {
            identity: "cdn:purge-cooldown",
            matches: |error| error.detail::<ErrorDetail>() == Some(&ErrorDetail::PurgeCooldown),
            name: name(4, "PurgeCooldown"),
            exit: Some(RuleExit::Failure),
            message: None,
        },
    ],
    operations: &[
        read!(
            "list-ip-ranges",
            "cdn.ListIPRanges",
            "List public CDN IP ranges",
            list_ip_ranges,
            EMPTY,
            |_| cdn::ListIpRangesInput,
            &[],
            &[],
            &["Reads the public CDN documentation without sending credentials."],
            None
        ),
        read!(
            "list-certificates",
            "cdn.ListCertificates",
            "List certificate metadata",
            list_certificates,
            EMPTY,
            |_| cdn::ListCertificatesInput,
            &[],
            &[],
            &["Returns certificate metadata. Use get-certificate for the public PEM chain."],
            None
        ),
        read!(
            "get-certificate",
            "cdn.GetCertificate",
            "Get a certificate and its public PEM chain",
            get_certificate,
            CERTIFICATE,
            |input: &Input| cdn::GetCertificateInput::new(&input.certificate_id),
            &[flag(&CERTIFICATE_ID)],
            &[],
            &["Returns the certificate and CA chain as PEM text."],
            Some(
                "greennode cdn get-certificate --certificate-id <certificate-id> --query Certificate"
            )
        ),
        read!(
            "list-api-keys",
            "cdn.ListAPIKeys",
            "List vCDN API key metadata",
            list_api_keys,
            EMPTY,
            |_| cdn::ListApiKeysInput,
            &[],
            &[],
            &["Current identifies the key used for the request."],
            None
        ),
        read!(
            "list-web-accelerators",
            "cdn.ListWebAccelerators",
            "List Web Accelerators",
            list_web_accelerators,
            EMPTY,
            |_| cdn::ListWebAcceleratorsInput,
            &[],
            &[],
            &["CDNDomain is the generated CNAME target."],
            None
        ),
        read!(
            "get-web-accelerator",
            "cdn.GetWebAccelerator",
            "Get a Web Accelerator",
            get_web_accelerator,
            CDN,
            |input: &Input| cdn::GetWebAcceleratorInput::new(&input.cdn_id),
            &[flag(&CDN_ID)],
            &[],
            &[],
            Some("greennode cdn get-web-accelerator --cdn-id <cdn-id> --query WebAccelerator")
        ),
        series!(
            "get-traffic",
            "cdn.GetTraffic",
            "Get CDN traffic",
            get_traffic
        ),
        series!(
            "get-request-rate",
            "cdn.GetRequestRate",
            "Get CDN request rate",
            get_request_rate
        ),
        series!(
            "get-cache-status",
            "cdn.GetCacheStatus",
            "Get CDN request counts by cache status",
            get_cache_status
        ),
        series!(
            "get-http-codes",
            "cdn.GetHTTPCodes",
            "Get CDN request counts by HTTP code",
            get_http_codes
        ),
        read!(
            "get-traffic-report",
            "cdn.GetTrafficReport",
            "Get a dated CDN traffic report",
            get_traffic_report,
            REPORT,
            Input::report,
            &[flag(&REPORT_FROM), flag(&REPORT_TO)],
            JSON_DOMAINS,
            ANALYTICS_NOTES,
            Some(
                "greennode cdn get-traffic-report --from <from> --to <to> --cli-input-json '{\"CDNDomains\":[\"<cdn-domain>\"]}' --query Items"
            )
        ),
    ],
};
