use crate::wire::product::guard::Guarded;
use http::Method;
use serde::Serialize;
use serde_json::json;

use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::wire::{DETAIL_SHAPE, Object, Redact, api, shape};
use super::write::{
    Mutation, Settlement, attached, available, finish, in_use, not_sent, screen_state,
};
use super::{Client, Read, Volume, path_id};

const ATTACH: Mutation = Mutation {
    id: "volume.AttachVolume",
    route: "v2/{}/volumes/{}/servers/{}/attach",
    method: Method::PUT,
    once: false,
};
const DETACH: Mutation = Mutation {
    id: "volume.DetachVolume",
    route: "v2/{}/volumes/{}/servers/{}/detach",
    method: Method::PUT,
    once: false,
};
const DELETE: Mutation = Mutation {
    id: "volume.DeleteVolume",
    route: "v2/{}/volumes/{}",
    method: Method::DELETE,
    once: false,
};

/// Identifies the volume and server for an attachment.
#[derive(Clone, Default)]
pub struct AttachVolumeInput {
    /// Volume UUID.
    pub volume_id: String,
    /// Target server UUID.
    pub server_id: String,
    /// Returns current state once the attachment is accepted.
    pub no_wait: bool,
}

crate::error::debug_record!(AttachVolumeInput {
    volume_id,
    server_id,
    no_wait
});

/// Identifies the volume and server for a detachment.
#[derive(Clone, Default)]
pub struct DetachVolumeInput {
    /// Volume UUID.
    pub volume_id: String,
    /// Server UUID from which to detach the volume.
    pub server_id: String,
    /// Allows detachment without requiring a stopped server.
    pub allow_running: bool,
    /// Returns current state once the detachment is accepted.
    pub no_wait: bool,
}

crate::error::debug_record!(DetachVolumeInput {
    volume_id,
    server_id,
    allow_running,
    no_wait
});

/// The attachment state and whether the request changed it.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AttachVolumeOutput {
    /// Last observed or pre-write volume state.
    pub volume: Volume,
    /// Whether an attachment write was accepted.
    pub changed: bool,
}

crate::wire::product::output::public_fields!(AttachVolumeOutput { volume, changed });

/// The detachment state and whether the request changed it.
pub type DetachVolumeOutput = AttachVolumeOutput;

impl Redact for AttachVolumeOutput {
    fn redact(&mut self, evidence: &Evidence) {
        self.volume.redact(evidence);
    }
}

/// Identifies a detached volume whose data should be deleted.
#[derive(Clone, Default)]
pub struct DeleteVolumeInput {
    /// Volume UUID.
    pub volume_id: String,
    /// Returns once the deletion is accepted.
    pub no_wait: bool,
}

crate::error::debug_record!(DeleteVolumeInput { volume_id, no_wait });

/// The deletion acknowledgement has no result fields.
#[derive(Clone, Copy, Default, Eq, PartialEq, Serialize)]
pub struct DeleteVolumeOutput {}

crate::wire::product::output::public_fields!(DeleteVolumeOutput {});

impl Redact for DeleteVolumeOutput {
    fn redact(&mut self, _: &Evidence) {}
}

/// Safety check that stopped a volume write before dispatch.
#[derive(Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum WriteRefusal {
    /// The volume is attached to a server.
    VolumeInUse,
    /// The volume is a boot volume or its role could not be confirmed.
    BootVolume,
    /// The server must be stopped before detachment.
    ServerRunning,
}

crate::error::debug_record!(WriteRefusal; VolumeInUse, BootVolume, ServerRunning);

#[derive(Default, Serialize)]
struct DetachServerState {
    status: String,
    boot_volume_id: String,
}

crate::error::debug_record!(DetachServerState {
    status,
    boot_volume_id
});

impl crate::wire::product::output::PublicOutput for DetachServerState {
    fn public_reflects(
        &self,
        scan: &crate::wire::product::output::FieldScan<'_>,
        identifier: bool,
    ) -> bool {
        scan.field(&self.status, identifier, "status", "status")
            || scan.field(
                &self.boot_volume_id,
                identifier,
                "bootVolumeId",
                "boot_volume_id",
            )
    }
}

struct Attachment<'a> {
    volume: &'a str,
    server: &'a str,
    no_wait: bool,
    mode: AttachmentMode,
}

#[derive(Clone, Copy)]
enum AttachmentMode {
    Attach,
    Detach { allow_running: bool },
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Attaches the volume unless it is already attached to the selected server.
    pub async fn attach_volume(
        &self,
        ctx: &RequestContext,
        input: AttachVolumeInput,
    ) -> Result<AttachVolumeOutput, Failure<AttachVolumeOutput>> {
        crate::wire::product::exit::finish!(async {
            self.change_attachment(
                ctx,
                Attachment {
                    volume: &input.volume_id,
                    server: &input.server_id,
                    no_wait: input.no_wait,
                    mode: AttachmentMode::Attach,
                },
            )
            .await
        })
    }

    /// Detaches a non-boot volume after confirming the server is stopped or the caller opted in.
    pub async fn detach_volume(
        &self,
        ctx: &RequestContext,
        input: DetachVolumeInput,
    ) -> Result<DetachVolumeOutput, Failure<DetachVolumeOutput>> {
        crate::wire::product::exit::finish!(async {
            self.change_attachment(
                ctx,
                Attachment {
                    volume: &input.volume_id,
                    server: &input.server_id,
                    no_wait: input.no_wait,
                    mode: AttachmentMode::Detach {
                        allow_running: input.allow_running,
                    },
                },
            )
            .await
        })
    }

    async fn change_attachment(
        &self,
        ctx: &RequestContext,
        input: Attachment<'_>,
    ) -> Result<AttachVolumeOutput, Failure<AttachVolumeOutput>> {
        let mutation = match input.mode {
            AttachmentMode::Attach => &ATTACH,
            AttachmentMode::Detach { .. } => &DETACH,
        };
        let mut evidence = Evidence::for_write();
        path_id(mutation.id, "VolumeID", input.volume).map_err(not_sent)?;
        path_id(mutation.id, "ServerID", input.server).map_err(not_sent)?;
        let body = json!({});
        self.guard_inputs(
            ctx,
            &mut evidence,
            &[input.volume, input.server],
            Some(&body),
        )
        .map_err(not_sent)?;
        let volume = Guarded::read_preflight(
            &mut evidence,
            async |evidence| self.volume(ctx, input.volume, evidence).await,
            std::convert::identity,
        )
        .await
        .map_err(not_sent)?;
        let current = volume.recheck(&evidence).map_err(not_sent)?;
        screen_state(&evidence, current).map_err(not_sent)?;
        let attach = matches!(input.mode, AttachmentMode::Attach);
        if attached(current, input.server) == attach {
            return finish(
                AttachVolumeOutput {
                    volume: volume.into_checked(&evidence).map_err(not_sent)?,
                    changed: false,
                },
                Ok(()),
                &evidence,
                mutation,
                "",
                WriteDispatch::NotSent,
            );
        }
        let project = self.project(ctx, &mut evidence).await.map_err(not_sent)?;
        if let AttachmentMode::Detach { allow_running } = input.mode {
            self.check_detachment(ctx, &project, &input, &volume, allow_running, &mut evidence)
                .await
                .map_err(not_sent)?;
        }
        let mut volume = volume.into_checked(&evidence).map_err(not_sent)?;
        self.send_write(
            ctx,
            mutation,
            &[&project, input.volume, input.server],
            Some(&body),
            &mut evidence,
        )
        .await?;
        let result = if input.no_wait {
            Ok(())
        } else {
            let settlement = if attach {
                Settlement::Attached(input.server)
            } else {
                Settlement::Detached
            };
            self.settle(ctx, input.volume, settlement, &mut volume, &mut evidence)
                .await
        };
        finish(
            AttachVolumeOutput {
                volume,
                changed: true,
            },
            result,
            &evidence,
            mutation,
            "run this operation again to check volume state",
            WriteDispatch::MayHaveBeenSent,
        )
    }

    async fn check_detachment(
        &self,
        ctx: &RequestContext,
        project: &str,
        input: &Attachment<'_>,
        volume: &Guarded<Volume>,
        allow_running: bool,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let read = Read::vserver(
            DETACH.id,
            "v2/{}/servers/{}",
            "invalid response shape; body withheld",
        );
        let server = Guarded::read_preflight(
            evidence,
            async |evidence| {
                self.read(
                    ctx,
                    read,
                    &[project, input.server],
                    &[],
                    evidence,
                    |raw, names| {
                        let envelope = Object::parse(raw, names)?;
                        envelope.get("data")?.map_or_else(
                            || Ok(DetachServerState::default()),
                            |data| {
                                let row = Object::parse(data, names)?;
                                Ok(DetachServerState {
                                    status: row.value("status")?,
                                    boot_volume_id: row.value("bootVolumeId")?,
                                })
                            },
                        )
                    },
                )
                .await
            },
            std::convert::identity,
        )
        .await?;
        let DetachServerState {
            status,
            boot_volume_id: boot,
        } = server.recheck(evidence)?;
        let volume = volume.recheck(evidence)?;
        let server = safe_label(input.server, evidence);
        let id = safe_label(input.volume, evidence);
        if !super::inventory::response_id(boot) {
            return Err(refusal(
                WriteRefusal::BootVolume,
                &format!(
                    "volume: cannot detach the boot volume: volume.DetachVolume: server {server} reported no boot volume id; refusing to detach volume {id} until this is confirmed safe"
                ),
            ));
        }
        if volume.bootable || volume.uuid == *boot {
            return Err(refusal(
                WriteRefusal::BootVolume,
                &format!(
                    "volume: cannot detach the boot volume: volume.DetachVolume: volume {id} is server {server}'s boot volume"
                ),
            ));
        }
        if !allow_running && !status.eq_ignore_ascii_case("STOPPED") {
            return Err(refusal(
                WriteRefusal::ServerRunning,
                &format!(
                    "volume: server is running: volume.DetachVolume: server {server} is {status:?}, not STOPPED; stop it first, or unmount and pass AllowRunning"
                ),
            ));
        }
        Ok(())
    }

    /// Deletes a volume only after a current read confirms it is detached.
    pub async fn delete_volume(
        &self,
        ctx: &RequestContext,
        input: DeleteVolumeInput,
    ) -> Result<DeleteVolumeOutput, Failure<DeleteVolumeOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::for_write();
            let id = &input.volume_id;
            path_id(DELETE.id, "VolumeID", id).map_err(not_sent)?;
            self.guard_inputs(ctx, &mut evidence, &[id], None)
                .map_err(not_sent)?;
            let guarded = Guarded::read_preflight(
                &mut evidence,
                async |evidence| self.volume_detail(ctx, id, evidence).await,
                std::convert::identity,
            )
            .await
            .map_err(not_sent)?;
            let (volume, attachment_known) = guarded.recheck(&evidence).map_err(not_sent)?;
            screen_state(&evidence, volume).map_err(not_sent)?;
            if in_use(volume)
                || !volume.server_id.is_empty()
                || volume
                    .server_id_list
                    .as_ref()
                    .is_some_and(|servers| !servers.is_empty())
            {
                return Err(not_sent(refusal(
                    WriteRefusal::VolumeInUse,
                    &format!(
                        "volume: volume in use: volume.DeleteVolume: volume {} is attached to a server; detach it first",
                        safe_label(id, &evidence)
                    ),
                )));
            }
            if !attachment_known
                || (!available(volume) && !volume.status.eq_ignore_ascii_case("ERROR"))
            {
                return Err(not_sent(shape(DELETE.id, DETAIL_SHAPE)));
            }
            let project = self.project(ctx, &mut evidence).await.map_err(not_sent)?;
            let (mut volume, _) = guarded.into_checked(&evidence).map_err(not_sent)?;
            self.send_write(ctx, &DELETE, &[&project, id], None, &mut evidence)
                .await?;
            let result = if input.no_wait {
                Ok(())
            } else {
                self.settle(ctx, id, Settlement::Deleted, &mut volume, &mut evidence)
                    .await
            };
            finish(
                DeleteVolumeOutput {},
                result,
                &evidence,
                &DELETE,
                "delete was sent and a rerun is safe",
                WriteDispatch::MayHaveBeenSent,
            )
        })
    }
}

fn refusal(identity: WriteRefusal, message: &str) -> Box<Error> {
    Box::new(
        Error::new(ErrorClass::Api)
            .with_detail(identity)
            .with_cause(api("", None, "", message)),
    )
}

fn safe_label<'a>(text: &'a str, evidence: &Evidence) -> &'a str {
    if evidence.text_reflects(text) {
        "<redacted>"
    } else {
        text
    }
}
