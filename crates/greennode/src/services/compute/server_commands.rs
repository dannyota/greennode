use super::*;

macro_rules! operation_field {
    ($name:literal, $flag:expr, $member:ident, $kind:ident, $value:ty, $required:expr) => {
        InputField {
            json_name: $name, flag: Some($flag), secret: false, nullable: false,
            set_json: |input, value| {
                input.$member = operation_field!(@json $kind, value);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$member = operation_field!(@flag $kind, matches, $flag, $value);
                Ok(())
            }),
            requirement: $required,
        }
    };
    ($name:literal, $member:ident, $kind:ident) => {
        InputField {
            json_name: $name, flag: None, secret: false, nullable: false,
            set_json: |input, value| {
                input.$member = operation_field!(@json $kind, value);
                Ok(())
            },
            set_flag: None, requirement: Requirement::Optional,
        }
    };
    (@json list, $value:ident) => {
        $value.as_array().ok_or(())?.iter().map(|value| value.as_str().map(str::to_owned).ok_or(())).collect::<Result<Vec<_>, ()>>()?
    };
    (@json $method:ident, $value:ident) => { $value.$method().ok_or(())?.to_owned() };
    (@flag list, $matches:ident, $flag:expr, $ty:ty) => { $matches.get_many::<String>($flag.name).ok_or(())?.cloned().collect() };
    (@flag $method:ident, $matches:ident, $flag:expr, $ty:ty) => { $matches.get_one::<$ty>($flag.name).ok_or(())?.to_owned() };
}

macro_rules! server_operation {
    ($constant:ident, $name:literal, $sdk:literal, $kind:ident, $help:literal, $notes:literal,
     $input:ty, $method:ident, $flags:expr, $json:expr, $extra:expr, [$($field:expr),* $(,)?] $(,)?) => {
        pub(super) const $constant: Operation = {
            const INPUT: InputDescriptor<$input> = InputDescriptor {
                defaults: <$input>::default, fields: &[$($field),*], allow_json: true,
                allow_query: true, project_binding: None, pre_guard: Some(check_input_source), guard: None,
            };
            Operation {
                name: $name, sdk_operation: $sdk, short_help: $help, example: None,
                presence_required_fields: &[], inherited_bindings: &[], kind: Kind::$kind,
                flags: $flags, json_fields: $json, extra_flags: $extra, notes: &[$notes],
                handler: Some(Handler::new(&INPUT, |selected, matches, invocation| {
                    Box::pin(dispatch::management(invocation, selected, matches, &INPUT, $method))
                })),
            }
        };
    };
}

const USER_DATA_FILE: Field = flag("user-data-file", "string", false);
const fn user_data<T>() -> InputField<T> {
    InputField {
        json_name: "UserData",
        flag: None,
        secret: true,
        nullable: false,
        set_json: |_, _| Err(()),
        set_flag: None,
        requirement: Requirement::Optional,
    }
}

#[derive(Clone, Default)]
struct CreateArgs {
    request: sdk::CreateServerInput,
    user_data_file: Option<String>,
}

impl std::ops::Deref for CreateArgs {
    type Target = sdk::CreateServerInput;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

impl std::ops::DerefMut for CreateArgs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.request
    }
}

const USER_DATA_PATH: InputField<CreateArgs> = InputField {
    json_name: "UserDataFile",
    flag: Some(USER_DATA_FILE),
    secret: false,
    nullable: false,
    set_json: |_, _| Err(()),
    set_flag: Some(|input, matches| {
        input.user_data_file = Some(
            matches
                .get_one::<String>(USER_DATA_FILE.name)
                .ok_or(())?
                .clone(),
        );
        Ok(())
    }),
    requirement: Requirement::Optional,
};

macro_rules! execute {
    ($method:ident, $input:ty) => {
        fn $method<'a>(
            ctx: &'a greennode_sdk::RequestContext,
            config: &'a greennode_sdk::Config,
            input: &'a $input,
        ) -> dispatch::FutureResult<'a, dispatch::ExecutionResult> {
            Box::pin(async move {
                dispatch::project_result(
                    sdk::Client::new(config).$method(ctx, input.clone()).await,
                    std::convert::identity,
                )
            })
        }
    };
}

execute!(quote_create_server, sdk::CreateServerInput);
execute!(quote_resize_server, sdk::ResizeServerInput);
execute!(delete_server, sdk::DeleteServerInput);

fn create_server<'a>(
    ctx: &'a greennode_sdk::RequestContext,
    config: &'a greennode_sdk::Config,
    args: &'a CreateArgs,
) -> dispatch::FutureResult<'a, dispatch::ExecutionResult> {
    Box::pin(async move {
        let mut request = args.request.clone();
        if let Some(path) = args.user_data_file.clone() {
            let error = || {
                dispatch::LocalError::usage(
                    "--user-data-file: expected a regular UTF-8 file of at most 64 KiB",
                )
            };
            request.user_data = Some(
                tokio::task::spawn_blocking(move || read_user_data(&path))
                    .await
                    .map_err(|_| error())?
                    .map_err(|_| error())?,
            );
        }
        dispatch::project_result(
            sdk::Client::new(config).create_server(ctx, request).await,
            std::convert::identity,
        )
    })
}

fn read_user_data(path: &str) -> std::io::Result<greennode_sdk::Secret> {
    use std::fs::OpenOptions;
    use std::io::{Error, ErrorKind, Read};

    const MAX: usize = 64 << 10;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "linux")]
        const NONBLOCK: i32 = 0x800;
        #[cfg(not(target_os = "linux"))]
        const NONBLOCK: i32 = 0x4;
        options.custom_flags(NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::from(ErrorKind::InvalidInput));
    }
    let mut data = zeroize::Zeroizing::new(Vec::new());
    file.take(MAX as u64 + 1).read_to_end(&mut data)?;
    if data.is_empty() || data.len() > MAX {
        return Err(Error::from(ErrorKind::InvalidInput));
    }
    Ok(greennode_sdk::Secret::new(
        std::str::from_utf8(&data).map_err(|_| Error::from(ErrorKind::InvalidData))?,
    ))
}

server_operation!(
    QUOTE_CREATE_SERVER,
    "quote-create-server",
    "compute.QuoteCreateServer",
    Read,
    "Quote create server",
    "Quotes the priced server fields without ordering. With `--attach-floating-ip`, the server quote includes the IP for monthly creates in `han-1` and `hcm-3`. In other regions the quote adds a conservative Elastic IP estimate from the same zone.",
    sdk::CreateServerInput,
    quote_create_server,
    &[
        flag("name", "string", false),
        flag("zone-id", "string", true),
        flag("flavor-id", "string", true),
        flag("image-id", "string", true),
        flag("vpc-id", "string", false),
        flag("subnet-id", "string", false),
        flag("security-group-id", "[]string", false),
        flag("ssh-key-id", "string", false),
        flag("root-disk-size", "int", true),
        flag("root-disk-type-id", "string", true),
        flag("root-disk-encryption-type-id", "string", false),
        flag("data-disk-size", "int", false),
        flag("data-disk-type-id", "string", false),
        flag("data-disk-name", "string", false),
        flag("data-disk-encryption-type-id", "string", false),
        flag("server-group-id", "string", false),
        flag("auto-renew", "bool", false),
        flag("attach-floating-ip", "bool", false),
    ],
    &[
        flag("AllowOpenLogin", "bool", false),
        flag("MaxPrice", "float64", false),
        flag("NoWait", "bool", false),
    ],
    &[],
    [
        operation_field!(
            "Name",
            flag("name", "string", false),
            name,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "ZoneID",
            flag("zone-id", "string", true),
            zone_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.zone_id.is_empty())
        ),
        operation_field!(
            "FlavorID",
            flag("flavor-id", "string", true),
            flavor_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.flavor_id.is_empty())
        ),
        operation_field!(
            "ImageID",
            flag("image-id", "string", true),
            image_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.image_id.is_empty())
        ),
        operation_field!(
            "VPCID",
            flag("vpc-id", "string", false),
            vpc_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "SubnetID",
            flag("subnet-id", "string", false),
            subnet_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "SecurityGroupIDs",
            flag("security-group-id", "[]string", false),
            security_group_ids,
            list,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "SSHKeyID",
            flag("ssh-key-id", "string", false),
            ssh_key_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "RootDiskSize",
            flag("root-disk-size", "int", true),
            root_disk_size,
            as_i64,
            i64,
            Requirement::Nonzero(|input| input.root_disk_size != 0)
        ),
        operation_field!(
            "RootDiskTypeID",
            flag("root-disk-type-id", "string", true),
            root_disk_type_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.root_disk_type_id.is_empty())
        ),
        operation_field!(
            "RootDiskEncryptionTypeID",
            flag("root-disk-encryption-type-id", "string", false),
            root_disk_encryption_type_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "DataDiskSize",
            flag("data-disk-size", "int", false),
            data_disk_size,
            as_i64,
            i64,
            Requirement::Optional
        ),
        operation_field!(
            "DataDiskTypeID",
            flag("data-disk-type-id", "string", false),
            data_disk_type_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "DataDiskName",
            flag("data-disk-name", "string", false),
            data_disk_name,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "DataDiskEncryptionTypeID",
            flag("data-disk-encryption-type-id", "string", false),
            data_disk_encryption_type_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "ServerGroupID",
            flag("server-group-id", "string", false),
            server_group_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "AutoRenew",
            flag("auto-renew", "bool", false),
            auto_renew,
            as_bool,
            bool,
            Requirement::Optional
        ),
        operation_field!(
            "AttachFloatingIP",
            flag("attach-floating-ip", "bool", false),
            attach_floating_ip,
            as_bool,
            bool,
            Requirement::Optional
        ),
        operation_field!("AllowOpenLogin", allow_open_login, as_bool),
        operation_field!("MaxPrice", max_price, as_f64),
        operation_field!("NoWait", no_wait, as_bool),
        user_data()
    ],
);

server_operation!(
    QUOTE_RESIZE_SERVER,
    "quote-resize-server",
    "compute.QuoteResizeServer",
    Read,
    "Quote resize server",
    "Quotes a server flavor change. A negative amount indicates a credit.",
    sdk::ResizeServerInput,
    quote_resize_server,
    &[
        flag("server-id", "string", true),
        flag("flavor-id", "string", true),
    ],
    &[
        flag("MaxPrice", "float64", false),
        flag("NoWait", "bool", false),
    ],
    &[],
    [
        operation_field!(
            "ServerID",
            flag("server-id", "string", true),
            server_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.server_id.is_empty())
        ),
        operation_field!(
            "FlavorID",
            flag("flavor-id", "string", true),
            flavor_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.flavor_id.is_empty())
        ),
        operation_field!("MaxPrice", max_price, as_f64),
        operation_field!("NoWait", no_wait, as_bool)
    ],
);

server_operation!(
    CREATE_SERVER,
    "create-server",
    "compute.CreateServer",
    Write,
    "Create server",
    "Checks inventory and price before placing one order. Supply an SSH key or `--user-data-file` for cloud-init login.

With `--attach-floating-ip`, create checks the named security groups before the quote and again before ordering. The `OpenLogin` refusal covers unrestricted TCP login on ports 22, 234, 3389, and 3490. `--allow-open-login` requires `--attach-floating-ip` and bypasses only the login-exposure check. Use narrow source prefixes and consider disabling password login in the guest. Rules can still change after the final check.

The monthly server quote includes the IP in `han-1` and `hcm-3`. Other regions add a conservative Elastic IP estimate in the same zone. `--max-price` caps the estimate. After `ACTIVE`, create allows up to two more minutes to confirm the address and IP ID. With `--no-wait`, the address remains unobserved. Verify console detach/release controls and permissions before creating. IP management commands are unavailable; server deletion does not prove release.",
    CreateArgs,
    create_server,
    &[
        flag("name", "string", true),
        flag("zone-id", "string", true),
        flag("flavor-id", "string", true),
        flag("image-id", "string", true),
        flag("vpc-id", "string", true),
        flag("subnet-id", "string", true),
        flag("security-group-id", "[]string", true),
        flag("ssh-key-id", "string", false),
        flag("root-disk-size", "int", true),
        flag("root-disk-type-id", "string", true),
        flag("root-disk-encryption-type-id", "string", false),
        flag("data-disk-size", "int", false),
        flag("data-disk-type-id", "string", false),
        flag("data-disk-name", "string", false),
        flag("data-disk-encryption-type-id", "string", false),
        flag("server-group-id", "string", false),
        flag("auto-renew", "bool", false),
        flag("attach-floating-ip", "bool", false),
        flag("allow-open-login", "bool", false),
        flag("max-price", "float64", false),
        flag("no-wait", "bool", false),
    ],
    &[],
    &[USER_DATA_FILE],
    [
        operation_field!(
            "Name",
            flag("name", "string", true),
            name,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.name.is_empty())
        ),
        operation_field!(
            "ZoneID",
            flag("zone-id", "string", true),
            zone_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.zone_id.is_empty())
        ),
        operation_field!(
            "FlavorID",
            flag("flavor-id", "string", true),
            flavor_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.flavor_id.is_empty())
        ),
        operation_field!(
            "ImageID",
            flag("image-id", "string", true),
            image_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.image_id.is_empty())
        ),
        operation_field!(
            "VPCID",
            flag("vpc-id", "string", true),
            vpc_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.vpc_id.is_empty())
        ),
        operation_field!(
            "SubnetID",
            flag("subnet-id", "string", true),
            subnet_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.subnet_id.is_empty())
        ),
        operation_field!(
            "SecurityGroupIDs",
            flag("security-group-id", "[]string", true),
            security_group_ids,
            list,
            String,
            Requirement::Nonzero(|input| !input.security_group_ids.is_empty())
        ),
        operation_field!(
            "SSHKeyID",
            flag("ssh-key-id", "string", false),
            ssh_key_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "RootDiskSize",
            flag("root-disk-size", "int", true),
            root_disk_size,
            as_i64,
            i64,
            Requirement::Nonzero(|input| input.root_disk_size != 0)
        ),
        operation_field!(
            "RootDiskTypeID",
            flag("root-disk-type-id", "string", true),
            root_disk_type_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.root_disk_type_id.is_empty())
        ),
        operation_field!(
            "RootDiskEncryptionTypeID",
            flag("root-disk-encryption-type-id", "string", false),
            root_disk_encryption_type_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "DataDiskSize",
            flag("data-disk-size", "int", false),
            data_disk_size,
            as_i64,
            i64,
            Requirement::Optional
        ),
        operation_field!(
            "DataDiskTypeID",
            flag("data-disk-type-id", "string", false),
            data_disk_type_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "DataDiskName",
            flag("data-disk-name", "string", false),
            data_disk_name,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "DataDiskEncryptionTypeID",
            flag("data-disk-encryption-type-id", "string", false),
            data_disk_encryption_type_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "ServerGroupID",
            flag("server-group-id", "string", false),
            server_group_id,
            as_str,
            String,
            Requirement::Optional
        ),
        operation_field!(
            "AutoRenew",
            flag("auto-renew", "bool", false),
            auto_renew,
            as_bool,
            bool,
            Requirement::Optional
        ),
        operation_field!(
            "AttachFloatingIP",
            flag("attach-floating-ip", "bool", false),
            attach_floating_ip,
            as_bool,
            bool,
            Requirement::Optional
        ),
        operation_field!(
            "AllowOpenLogin",
            flag("allow-open-login", "bool", false),
            allow_open_login,
            as_bool,
            bool,
            Requirement::Optional
        ),
        operation_field!(
            "MaxPrice",
            flag("max-price", "float64", false),
            max_price,
            as_f64,
            f64,
            Requirement::Optional
        ),
        operation_field!(
            "NoWait",
            flag("no-wait", "bool", false),
            no_wait,
            as_bool,
            bool,
            Requirement::Optional
        ),
        user_data(),
        USER_DATA_PATH
    ],
);

#[cfg(test)]
mod tests {
    #[test]
    fn create_note_explains_floating_ip_release() {
        let note = super::CREATE_SERVER.notes.join(" ");
        assert!(note.contains("console detach/release"));
        assert!(note.contains("server deletion does not prove release"));
    }

    #[test]
    fn empty_user_data_file_is_rejected() {
        let path =
            std::env::temp_dir().join(format!("greennode-empty-user-data-{}", std::process::id()));
        std::fs::write(&path, "").unwrap();
        let result = super::read_user_data(path.to_str().unwrap());
        std::fs::remove_file(path).unwrap();
        assert!(result.is_err());
    }
}

server_operation!(
    DELETE_SERVER,
    "delete-server",
    "compute.DeleteServer",
    DestructiveWrite,
    "Delete server",
    "The boot volume is always deleted. Data volumes are retained unless `--delete-volumes` is set.",
    sdk::DeleteServerInput,
    delete_server,
    &[
        flag("server-id", "string", true),
        flag("delete-volumes", "bool", false),
        flag("no-wait", "bool", false),
    ],
    &[],
    &[],
    [
        operation_field!(
            "ServerID",
            flag("server-id", "string", true),
            server_id,
            as_str,
            String,
            Requirement::Nonzero(|input| !input.server_id.is_empty())
        ),
        operation_field!(
            "DeleteVolumes",
            flag("delete-volumes", "bool", false),
            delete_volumes,
            as_bool,
            bool,
            Requirement::Optional
        ),
        operation_field!(
            "NoWait",
            flag("no-wait", "bool", false),
            no_wait,
            as_bool,
            bool,
            Requirement::Optional
        )
    ],
);

pub(super) use {execute, operation_field, server_operation};
