use super::super::quotes::{encoded_user_data, encrypted};
use super::*;

pub(super) fn create_body(input: &CreateServerInput) -> Result<Value, Box<Error>> {
    let invalid = |message: &str| Box::new(invalid_input(&format!("{}: {message}", CREATE.id)));
    if input.allow_open_login && !input.attach_floating_ip {
        return Err(invalid("AllowOpenLogin requires AttachFloatingIP"));
    }
    let data = input
        .user_data
        .as_ref()
        .filter(|data| !data.expose_secret().is_empty());
    match (input.ssh_key_id.is_empty(), data.is_some()) {
        (true, false) => {
            return Err(invalid(
                "login is required: an SSH key, or user data that installs keys",
            ));
        }
        (false, true) => {
            return Err(invalid(
                "GreenNode refuses user data together with an SSH key; put the key in the cloud-config",
            ));
        }
        _ => {}
    }
    let mut value = json!({
        "name": input.name, "zoneId": input.zone_id, "flavorId": input.flavor_id,
        "imageId": input.image_id, "networkId": input.vpc_id, "subnetId": input.subnet_id,
        "securityGroup": input.security_group_ids, "rootDiskSize": input.root_disk_size,
        "rootDiskTypeId": input.root_disk_type_id, "encryptionVolume": encrypted(input),
        "isEnableAutoRenew": input.auto_renew, "attachFloating": input.attach_floating_ip,
    });
    for (key, text) in [
        ("sshKeyId", &input.ssh_key_id),
        ("dataDiskName", &input.data_disk_name),
        ("dataDiskTypeId", &input.data_disk_type_id),
        (
            "rootDiskEncryptionType",
            &input.root_disk_encryption_type_id,
        ),
        (
            "dataDiskEncryptionType",
            &input.data_disk_encryption_type_id,
        ),
        ("serverGroupId", &input.server_group_id),
    ] {
        if !text.is_empty() {
            value[key] = json!(text);
        }
    }
    if input.data_disk_size != 0 {
        value["dataDiskSize"] = json!(input.data_disk_size);
    }
    if let Some(data) = data {
        value["userData"] = json!(encoded_user_data(data).expose_secret());
        value["userDataBase64Encoded"] = json!(true);
    }
    Ok(value)
}

pub(super) fn create_error(
    failed: Failure<CreateServerOutput>,
    sensitive: bool,
    floating: bool,
    evidence: &Evidence,
) -> Failure<CreateServerOutput> {
    let (error, _, dispatch) = failed.into_parts();
    let remote_status = if floating {
        error.api_error().and_then(ApiError::status)
    } else {
        status(&error)
    };
    let error = if remote_status == Some(400)
        || (sensitive && remote_status.is_some_and(|status| (400..600).contains(&status)))
    {
        let observed = error.api_error();
        let code = observed.map_or("", ApiError::code);
        let message = if sensitive {
            USER_DATA_WITHHELD
        } else {
            observed.map_or("Bad Request", ApiError::message)
        };
        let message = if remote_status == Some(400) {
            let end = message.floor_char_boundary(512 - DISK_ADVICE.len() - 2);
            format!("{}; {DISK_ADVICE}", &message[..end])
        } else {
            message.to_owned()
        };
        evidence.screen_error(Error::from_api(ApiError::new(
            CREATE.id,
            remote_status,
            SafeText::new(code, &[]),
            SafeText::new(&message, &[]),
            observed.is_some_and(ApiError::retryable),
        )))
    } else {
        error
    };
    let error = if dispatch == WriteDispatch::MayHaveBeenSent
        && !remote_status.is_some_and(|status| (400..500).contains(&status))
        && !evidence.text_reflects(CREATE_AMBIGUOUS)
    {
        let reason = error
            .api_error()
            .map_or_else(String::new, ApiError::go_text);
        let reason = if evidence.text_reflects(&reason) {
            ""
        } else {
            reason.as_str()
        };
        let end = reason.floor_char_boundary(512 - CREATE_AMBIGUOUS.len() - CREATE.id.len() - 4);
        let advice = if reason.is_empty() {
            format!("{}: {CREATE_AMBIGUOUS}", CREATE.id)
        } else {
            format!("{}: {CREATE_AMBIGUOUS}: {}", CREATE.id, &reason[..end])
        };
        if evidence.text_reflects(&advice) {
            error
        } else {
            error.with_outer_detail(crate::error::RecoveryAdvice::new(&advice))
        }
    } else {
        error
    };
    Failure::new(error, None, dispatch)
}
