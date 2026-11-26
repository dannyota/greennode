use super::*;

const POOL_NAME: Field = flag("name", "string", true);
const PROTOCOL: Field = flag("protocol", "string", true);
const HEALTH_CHECK_PROTOCOL: Field = flag("health-check-protocol", "string", true);

macro_rules! option_field {
    ($field:ident, $flag:ident, $name:literal, $ty:ty, $decode:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.$field = Some(($decode)(value).ok_or(())?);
                Ok(())
            },
            set_flag: Some(|input: &mut _, matches| {
                input.$field = Some(matches.get_one::<$ty>($flag.name).ok_or(())?.clone());
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

pub(super) use option_field;

const ALGORITHM: Field = flag("algorithm", "string", false);
const UPDATE_ALGORITHM: Field = flag("algorithm", "*string", false);
const UPDATE_STICKINESS: Field = flag("stickiness", "*bool", false);
const UPDATE_TLS_ENCRYPTION: Field = flag("tls-encryption", "*bool", false);
const HEALTH_CHECK_PATH: Field = flag("health-check-path", "string", false);
const UPDATE_HEALTH_CHECK_PATH: Field = flag("health-check-path", "*string", false);
const HEALTH_CHECK_METHOD: Field = flag("health-check-method", "string", false);
const UPDATE_HEALTH_CHECK_METHOD: Field = flag("health-check-method", "*string", false);
const HEALTH_CHECK_HTTP_VERSION: Field = flag("health-check-http-version", "string", false);
const UPDATE_HEALTH_CHECK_HTTP_VERSION: Field = flag("health-check-http-version", "*string", false);
const HEALTH_CHECK_DOMAIN_NAME: Field = flag("health-check-domain-name", "string", false);
const UPDATE_HEALTH_CHECK_DOMAIN_NAME: Field = flag("health-check-domain-name", "*string", false);
const HEALTH_CHECK_SUCCESS_CODE: Field = flag("health-check-success-code", "string", false);
const UPDATE_HEALTH_CHECK_SUCCESS_CODE: Field = flag("health-check-success-code", "*string", false);
const HEALTHY_THRESHOLD: Field = flag("healthy-threshold", "int", false);
const UPDATE_HEALTHY_THRESHOLD: Field = flag("healthy-threshold", "*int", false);
const UNHEALTHY_THRESHOLD: Field = flag("unhealthy-threshold", "int", false);
const UPDATE_UNHEALTHY_THRESHOLD: Field = flag("unhealthy-threshold", "*int", false);
const HEALTH_CHECK_INTERVAL: Field = flag("health-check-interval", "int", false);
const UPDATE_HEALTH_CHECK_INTERVAL: Field = flag("health-check-interval", "*int", false);
const HEALTH_CHECK_TIMEOUT: Field = flag("health-check-timeout", "int", false);
const UPDATE_HEALTH_CHECK_TIMEOUT: Field = flag("health-check-timeout", "*int", false);

pub(super) const CREATE_POOL: Operation = operation!(
    @kind Kind::Write,
    CreatePoolInput, create_pool, "create-pool", "CreatePool",
    "Create a load balancer pool",
    "greennode loadbalancer create-pool --load-balancer-id <load-balancer-id> --name <name> --protocol <protocol> --health-check-protocol <health-check-protocol>",
    &["Defaults to `ROUND_ROBIN` and health checks with thresholds 3 and 3, a 30-second interval, and a 5-second timeout. HTTP health check fields require HTTP or HTTPS. Use `--no-wait` to return submitted settings."], [
        LOAD_BALANCER_ID => required_id!(sdk::CreatePoolInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        POOL_NAME => required_id!(sdk::CreatePoolInput, name, POOL_NAME, "Name"),
        PROTOCOL => required_id!(sdk::CreatePoolInput, protocol, PROTOCOL, "Protocol"),
        ALGORITHM => string_field!(sdk::CreatePoolInput, algorithm, ALGORITHM, "Algorithm", Requirement::Optional),
        UPDATE_STICKINESS => option_field!(stickiness, UPDATE_STICKINESS, "Stickiness", bool, |value: serde_json::Value| value.as_bool()),
        UPDATE_TLS_ENCRYPTION => option_field!(tls_encryption, UPDATE_TLS_ENCRYPTION, "TLSEncryption", bool, |value: serde_json::Value| value.as_bool()),
        HEALTH_CHECK_PROTOCOL => required_id!(sdk::CreatePoolInput, health_check_protocol, HEALTH_CHECK_PROTOCOL, "HealthCheckProtocol"),
        HEALTH_CHECK_PATH => string_field!(sdk::CreatePoolInput, health_check_path, HEALTH_CHECK_PATH, "HealthCheckPath", Requirement::Optional),
        HEALTH_CHECK_METHOD => string_field!(sdk::CreatePoolInput, health_check_method, HEALTH_CHECK_METHOD, "HealthCheckMethod", Requirement::Optional),
        HEALTH_CHECK_HTTP_VERSION => string_field!(sdk::CreatePoolInput, health_check_http_version, HEALTH_CHECK_HTTP_VERSION, "HealthCheckHTTPVersion", Requirement::Optional),
        HEALTH_CHECK_DOMAIN_NAME => string_field!(sdk::CreatePoolInput, health_check_domain_name, HEALTH_CHECK_DOMAIN_NAME, "HealthCheckDomainName", Requirement::Optional),
        HEALTH_CHECK_SUCCESS_CODE => string_field!(sdk::CreatePoolInput, health_check_success_code, HEALTH_CHECK_SUCCESS_CODE, "HealthCheckSuccessCode", Requirement::Optional),
        HEALTHY_THRESHOLD => int_field!(healthy_threshold, HEALTHY_THRESHOLD, "HealthyThreshold"),
        UNHEALTHY_THRESHOLD => int_field!(unhealthy_threshold, UNHEALTHY_THRESHOLD, "UnhealthyThreshold"),
        HEALTH_CHECK_INTERVAL => int_field!(health_check_interval, HEALTH_CHECK_INTERVAL, "HealthCheckInterval"),
        HEALTH_CHECK_TIMEOUT => int_field!(health_check_timeout, HEALTH_CHECK_TIMEOUT, "HealthCheckTimeout"),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ]
);

pub(super) const UPDATE_POOL: Operation = operation!(
    @kind Kind::Write,
    UpdatePoolInput, update_pool, "update-pool", "UpdatePool",
    "Update a load balancer pool",
    "greennode loadbalancer update-pool --load-balancer-id <load-balancer-id> --pool-id <pool-id> --algorithm <algorithm>",
    &["Reads and merges the pool and its health monitor. Supply at least one setting. The health check protocol stays fixed. Use `--no-wait` to return submitted settings."], [
        LOAD_BALANCER_ID => required_id!(sdk::UpdatePoolInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        POOL_ID => required_id!(sdk::UpdatePoolInput, pool_id, POOL_ID, "PoolID"),
        UPDATE_ALGORITHM => option_field!(algorithm, UPDATE_ALGORITHM, "Algorithm", String, |value: serde_json::Value| value.as_str().map(str::to_owned)),
        UPDATE_STICKINESS => option_field!(stickiness, UPDATE_STICKINESS, "Stickiness", bool, |value: serde_json::Value| value.as_bool()),
        UPDATE_TLS_ENCRYPTION => option_field!(tls_encryption, UPDATE_TLS_ENCRYPTION, "TLSEncryption", bool, |value: serde_json::Value| value.as_bool()),
        UPDATE_HEALTH_CHECK_PATH => option_field!(health_check_path, UPDATE_HEALTH_CHECK_PATH, "HealthCheckPath", String, |value: serde_json::Value| value.as_str().map(str::to_owned)),
        UPDATE_HEALTH_CHECK_METHOD => option_field!(health_check_method, UPDATE_HEALTH_CHECK_METHOD, "HealthCheckMethod", String, |value: serde_json::Value| value.as_str().map(str::to_owned)),
        UPDATE_HEALTH_CHECK_HTTP_VERSION => option_field!(health_check_http_version, UPDATE_HEALTH_CHECK_HTTP_VERSION, "HealthCheckHTTPVersion", String, |value: serde_json::Value| value.as_str().map(str::to_owned)),
        UPDATE_HEALTH_CHECK_DOMAIN_NAME => option_field!(health_check_domain_name, UPDATE_HEALTH_CHECK_DOMAIN_NAME, "HealthCheckDomainName", String, |value: serde_json::Value| value.as_str().map(str::to_owned)),
        UPDATE_HEALTH_CHECK_SUCCESS_CODE => option_field!(health_check_success_code, UPDATE_HEALTH_CHECK_SUCCESS_CODE, "HealthCheckSuccessCode", String, |value: serde_json::Value| value.as_str().map(str::to_owned)),
        UPDATE_HEALTHY_THRESHOLD => option_field!(healthy_threshold, UPDATE_HEALTHY_THRESHOLD, "HealthyThreshold", i64, |value: serde_json::Value| value.as_i64()),
        UPDATE_UNHEALTHY_THRESHOLD => option_field!(unhealthy_threshold, UPDATE_UNHEALTHY_THRESHOLD, "UnhealthyThreshold", i64, |value: serde_json::Value| value.as_i64()),
        UPDATE_HEALTH_CHECK_INTERVAL => option_field!(health_check_interval, UPDATE_HEALTH_CHECK_INTERVAL, "HealthCheckInterval", i64, |value: serde_json::Value| value.as_i64()),
        UPDATE_HEALTH_CHECK_TIMEOUT => option_field!(health_check_timeout, UPDATE_HEALTH_CHECK_TIMEOUT, "HealthCheckTimeout", i64, |value: serde_json::Value| value.as_i64()),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ]
);
