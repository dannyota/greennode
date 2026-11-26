use super::server_commands::{execute, operation_field, server_operation};
use super::*;
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};

pub(super) const UNEXPECTED_STATUS: Rule = Rule {
    identity: "compute-unexpected-status",
    matches: |error| error.detail::<sdk::UnexpectedStatus>().is_some(),
    name: Some(RuleName {
        rank: 19,
        code: "UnexpectedStatus",
        operation: RuleOperation::Omit,
        include_api_status: false,
    }),
    exit: Some(RuleExit::Failure),
    message: None,
};

const NO_WAIT: Field = flag("no-wait", "bool", false);
const FLAVOR_ID: Field = flag("flavor-id", "string", true);
const MAX_PRICE: Field = flag("max-price", "float64", false);
const NEW_NAME: Field = flag("name", "string", true);

macro_rules! toggle {
    ($name:ident, $cli:literal, $go:literal, $kind:ident, $help:literal, $notes:literal, $input:ty, $method:ident) => {
        execute!($method, $input);
        server_operation!(
            $name,
            $cli,
            $go,
            $kind,
            $help,
            $notes,
            $input,
            $method,
            &[SERVER_ID, NO_WAIT],
            &[],
            &[],
            [
                required_id!($input, server_id, SERVER_ID, "ServerID"),
                operation_field!(
                    "NoWait",
                    NO_WAIT,
                    no_wait,
                    as_bool,
                    bool,
                    Requirement::Optional
                ),
            ]
        );
    };
}

toggle!(
    START,
    "start-server",
    "compute.StartServer",
    Write,
    "Start server",
    "Starts a stopped server. An active server returns with `Changed` set to false.",
    sdk::StartServerInput,
    start_server
);
toggle!(
    STOP,
    "stop-server",
    "compute.StopServer",
    DestructiveWrite,
    "Stop server",
    "Stops an active server. A stopped server returns with `Changed` set to false.",
    sdk::StopServerInput,
    stop_server
);
toggle!(
    REBOOT,
    "reboot-server",
    "compute.RebootServer",
    DestructiveWrite,
    "Reboot server",
    "Reboots an active server. Inspect `get-server` and the console after an unconfirmed reboot.",
    sdk::RebootServerInput,
    reboot_server
);

execute!(rename_server, sdk::RenameServerInput);
server_operation!(
    RENAME,
    "rename-server",
    "compute.RenameServer",
    Write,
    "Rename server",
    "Returns the server from the rename response.",
    sdk::RenameServerInput,
    rename_server,
    &[SERVER_ID, NEW_NAME],
    &[],
    &[],
    [
        required_id!(sdk::RenameServerInput, server_id, SERVER_ID, "ServerID"),
        required_id!(sdk::RenameServerInput, name, NEW_NAME, "Name"),
    ]
);

execute!(resize_server, sdk::ResizeServerInput);
server_operation!(
    RESIZE,
    "resize-server",
    "compute.ResizeServer",
    DestructiveWrite,
    "Resize server",
    "Changes the flavor of an active or stopped server. The signed quote can be a credit. Inspect `get-server` after an unconfirmed resize.",
    sdk::ResizeServerInput,
    resize_server,
    &[SERVER_ID, FLAVOR_ID, MAX_PRICE, NO_WAIT],
    &[],
    &[],
    [
        required_id!(sdk::ResizeServerInput, server_id, SERVER_ID, "ServerID"),
        required_id!(sdk::ResizeServerInput, flavor_id, FLAVOR_ID, "FlavorID"),
        operation_field!(
            "MaxPrice",
            MAX_PRICE,
            max_price,
            as_f64,
            f64,
            Requirement::Optional
        ),
        operation_field!(
            "NoWait",
            NO_WAIT,
            no_wait,
            as_bool,
            bool,
            Requirement::Optional
        ),
    ]
);
