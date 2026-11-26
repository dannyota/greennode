use std::path::Path;

use clap::ArgMatches;
use greennode_sdk::{Clock, RequestContext};

use crate::cli::dispatch::{self, FutureResult, LocalError};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use crate::cli::{Stdin, password};

use super::{check_input_source, flag, sdk};

type Input = sdk::ImportCertificateInput;
type FileBinding = (Field, fn(&mut Input, greennode_sdk::Secret));
const NAME: Field = flag("name", "string", true);
const TYPE: Field = flag("type", "string", true);
const CERT: Field = flag("certificate-file", "string", true);
const CHAIN: Field = flag("certificate-chain-file", "string", false);
const KEY: Field = flag("private-key-file", "string", false);
const PASSPHRASE: Field = flag("passphrase-file", "string", false);
const FILES: [Field; 4] = [CERT, CHAIN, KEY, PASSPHRASE];

const fn secret_field(name: &'static str) -> InputField<Input> {
    InputField {
        json_name: name,
        flag: None,
        secret: true,
        nullable: false,
        set_json: |_, _| Err(()),
        set_flag: None,
        requirement: Requirement::Optional,
    }
}

macro_rules! pem_field {
    ($name:literal, $member:ident) => {
        InputField {
            json_name: $name,
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                let serde_json::Value::String(value) = value else {
                    return Err(());
                };
                input.$member = value;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        }
    };
}

const INPUT: InputDescriptor<Input> = InputDescriptor {
    defaults: || Input::new("", "", ""),
    fields: &[
        required_id!(Input, name, NAME, "Name"),
        required_id!(Input, r#type, TYPE, "Type"),
        pem_field!("Certificate", certificate),
        pem_field!("CertificateChain", certificate_chain),
        secret_field("PrivateKey"),
        secret_field("Passphrase"),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_sources),
    guard: Some(check_certificate_source),
};

pub(super) const IMPORT: Operation = Operation {
    name: "import-certificate",
    sdk_operation: "loadbalancer.ImportCertificate",
    short_help: "Import a certificate for regional listeners",
    example: Some(
        "greennode loadbalancer import-certificate --name <name> --type TLS/SSL --certificate-file <certificate.pem> --private-key-file <key.pem>",
    ),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[NAME, TYPE],
    json_fields: &[
        flag("Certificate", "string", true),
        flag("CertificateChain", "string", false),
    ],
    extra_flags: &FILES,
    notes: &[
        "Read PEM material from files up to 64 KiB each. A single file flag can use `-` for non-terminal stdin. Private keys and passphrases require private files on Unix and are forbidden in `--cli-input-json`.",
        "TLS imports require `--private-key-file`. Other certificate types accept only the certificate itself. If an import may have reached the server, list certificates by name before trying again.",
    ],
    handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
        Box::pin(dispatch::management_with_input_preparation(
            invocation,
            operation,
            matches,
            &INPUT,
            |ctx, config, input| {
                Box::pin(async move {
                    let output = sdk::Client::new(config)
                        .import_certificate(ctx, input.clone())
                        .await;
                    dispatch::project_result(output, std::convert::identity)
                })
            },
            prepare,
        ))
    })),
};

fn check_sources(matches: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    check_input_source(matches, json)?;
    if FILES
        .iter()
        .filter(|field| {
            matches
                .get_one::<String>(field.name)
                .is_some_and(|path| path == "-")
        })
        .count()
        > 1
    {
        return Err(InputError("only one certificate file flag can read stdin"));
    }
    Ok(())
}

fn check_certificate_source(input: &Input, matches: &ArgMatches) -> Result<(), InputError> {
    if input.certificate.is_empty() && matches.get_one::<String>(CERT.name).is_none() {
        return Err(InputError(
            "--certificate-file is required (or set Certificate through --cli-input-json)",
        ));
    }
    Ok(())
}

fn prepare<'a>(
    ctx: &'a RequestContext,
    clock: &'a dyn Clock,
    matches: &'a ArgMatches,
    stdin: &'a mut dyn Stdin,
    terminal: bool,
    input: &'a mut Input,
) -> FutureResult<'a, Result<(), LocalError>> {
    Box::pin(async move {
        let bindings: [FileBinding; 4] = [
            (CERT, |input, text| {
                input.certificate = text.expose_secret().to_owned()
            }),
            (CHAIN, |input, text| {
                input.certificate_chain = text.expose_secret().to_owned()
            }),
            (KEY, |input, text| input.private_key = text),
            (PASSPHRASE, |input, text| input.passphrase = text),
        ];
        for (field, bind) in bindings {
            let Some(path) = matches.get_one::<String>(field.name) else {
                continue;
            };
            let mode = password::FileInput::Certificate {
                flag: field.name,
                private: matches!(field, KEY | PASSPHRASE),
                trim_newline: field == PASSPHRASE,
            };
            let text =
                password::read_input(ctx, clock, Path::new(path), stdin, terminal, mode).await?;
            bind(input, text);
        }
        Ok(())
    })
}
