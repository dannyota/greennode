use crate::model::wire::WireField;
use crate::wire::product::{evidence::Evidence, options::AttemptCheck};
use crate::{
    DEFAULT_PAGE, DEFAULT_PAGE_SIZE, Error, ErrorClass, Failure, List, PagedList, RequestContext,
    Secret,
};

use super::models::Policy;
use super::wire::{Envelope, Read, ReadStyle, api};
use super::*;

pub(super) const LIST_SERVERS: Read =
    Read::new("compute.ListServers", "v2/{}/servers", ReadStyle::Server);
pub(super) const GET_SERVER: Read =
    Read::new("compute.GetServer", "v2/{}/servers/{}", ReadStyle::Server);
const CONSOLE: Read = Read::new(
    "compute.GetServerConsoleLog",
    "v2/{}/servers/{}/console-log",
    ReadStyle::Console,
);
const LIST_KEYS: Read = Read::new("compute.ListSSHKeys", "v2/{}/sshKeys", ReadStyle::Server);
const GET_KEY: Read = Read::new("compute.GetSSHKey", "v2/{}/sshKeys/{}", ReadStyle::Server);
pub(super) const LIST_GROUPS: Read = Read::new(
    "compute.ListServerGroups",
    "v2/{}/serverGroups",
    ReadStyle::Server,
);
const GET_GROUP: Read = Read::new(
    "compute.GetServerGroup",
    "v2/{}/serverGroups/{}",
    ReadStyle::Plain,
);
const LIST_POLICIES: Read = Read::new(
    "compute.ListServerGroupPolicies",
    "v2/{}/serverGroups/policies",
    ReadStyle::Plain,
);
const LIST_OS_IMAGES: Read = Read::new("compute.ListOSImages", "v1/{}/images/os", ReadStyle::Plain);
const LIST_GPU_IMAGES: Read = Read::new(
    "compute.ListGPUImages",
    "v1/{}/images/gpu",
    ReadStyle::Plain,
);
const LIST_USER_IMAGES: Read = Read::new(
    "compute.ListUserImages",
    "v2/{}/user-images",
    ReadStyle::Plain,
);

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads one page of server inventory.
    pub async fn list_servers(
        &self,
        ctx: &RequestContext,
        input: ListServersInput,
    ) -> Result<ListServersOutput, Failure<ListServersOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            let page = self
                .server_page(ctx, input.page, input.size, &mut evidence)
                .await?;
            retain_page(page, &LIST_SERVERS, &evidence)
        })
    }

    pub(super) async fn server_page<O>(
        &self,
        ctx: &RequestContext,
        page: i64,
        size: i64,
        evidence: &mut Evidence,
    ) -> Result<PagedList<Server>, Failure<O>> {
        self.server_page_with_attempt(ctx, page, size, evidence, None, false)
            .await
    }

    pub(super) async fn server_page_with_attempt<O>(
        &self,
        ctx: &RequestContext,
        page: i64,
        size: i64,
        evidence: &mut Evidence,
        attempt_check: Option<AttemptCheck<'_>>,
        reject_reflection: bool,
    ) -> Result<PagedList<Server>, Failure<O>> {
        let ctx = self.server_deadline(ctx);
        let [page, size] = page_query(page, size);
        let envelope = self
            .read_with_attempt(
                &ctx,
                &LIST_SERVERS,
                &[],
                &[("page", &page), ("size", &size)],
                evidence,
                attempt_check,
            )
            .await?;
        if reject_reflection && envelope.guard_reflects(evidence) {
            return Err(failure(LIST_SERVERS.invalid(evidence)));
        }
        parse_page(envelope, &LIST_SERVERS, evidence, true, reject_reflection)
    }

    /// Reads the server identified by the input.
    pub async fn get_server(
        &self,
        ctx: &RequestContext,
        input: GetServerInput,
    ) -> Result<GetServerOutput, Failure<GetServerOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            let server = self
                .server_detail(ctx, &input.server_id, &mut evidence)
                .await?;
            Ok(GetServerOutput {
                server: GET_SERVER.retain(server, &evidence)?,
            })
        })
    }

    pub(super) async fn server_detail<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Server, Failure<O>> {
        self.server_detail_with_policy(ctx, id, evidence, false)
            .await
    }

    pub(super) async fn server_guard<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<crate::wire::product::guard::Guarded<Server>, Failure<O>> {
        let value = self
            .server_detail_with_policy(ctx, id, evidence, true)
            .await?;
        crate::wire::product::guard::Guarded::latest(value, evidence)
            .map_err(|error| failure(*error))
    }

    pub(super) async fn server_guard_with_attempt<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
        attempt_check: Option<AttemptCheck<'_>>,
    ) -> Result<crate::wire::product::guard::Guarded<Server>, Failure<O>> {
        let value = self
            .server_detail_with_attempt(ctx, id, evidence, true, attempt_check)
            .await?;
        crate::wire::product::guard::Guarded::latest(value, evidence)
            .map_err(|error| failure(*error))
    }

    async fn server_detail_with_policy<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
        reject_reflection: bool,
    ) -> Result<Server, Failure<O>> {
        self.server_detail_with_attempt(ctx, id, evidence, reject_reflection, None)
            .await
    }

    pub(super) async fn server_detail_with_attempt<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
        reject_reflection: bool,
        attempt_check: Option<AttemptCheck<'_>>,
    ) -> Result<Server, Failure<O>> {
        GET_SERVER.path("ServerID", id).map_err(failure)?;
        let ctx = self.server_deadline(ctx);
        let envelope = if reject_reflection {
            self.read_guard_with_attempt(
                &ctx,
                super::wire::GuardRead(&GET_SERVER),
                &[id],
                &[],
                evidence,
                attempt_check,
            )
            .await?
        } else {
            self.read_with_attempt(&ctx, &GET_SERVER, &[id], &[], evidence, attempt_check)
                .await?
        };
        let server: Server = envelope.field("data", &GET_SERVER, evidence)?;
        if server.uuid != id {
            return Err(failure(GET_SERVER.invalid(evidence)));
        }
        Ok(server)
    }

    /// Reads the server console log into a redacted secret value.
    pub async fn get_server_console_log(
        &self,
        ctx: &RequestContext,
        input: GetServerConsoleLogInput,
    ) -> Result<GetServerConsoleLogOutput, Failure<GetServerConsoleLogOutput>> {
        crate::wire::product::exit::finish!(async {
            CONSOLE
                .path("ServerID", &input.server_id)
                .map_err(failure)?;
            let mut evidence = Evidence::new();
            let envelope = self
                .read(ctx, &CONSOLE, &[&input.server_id], &[], &mut evidence)
                .await?;
            let log: Option<String> = envelope.field("data", &CONSOLE, &evidence)?;
            let log = log.ok_or_else(|| failure(CONSOLE.invalid(&evidence)))?;
            Ok(GetServerConsoleLogOutput {
                log: Secret::new(CONSOLE.retain(log, &evidence)?),
            })
        })
    }

    /// Reads one page of public SSH keys.
    pub async fn list_ssh_keys(
        &self,
        ctx: &RequestContext,
        input: ListSshKeysInput,
    ) -> Result<ListSshKeysOutput, Failure<ListSshKeysOutput>> {
        crate::wire::product::exit::finish!(async {
            let [page, size] = page_query(input.page, input.size);
            let mut evidence = Evidence::new();
            let envelope = self
                .read(
                    ctx,
                    &LIST_KEYS,
                    &[],
                    &[("name", &input.name), ("page", &page), ("size", &size)],
                    &mut evidence,
                )
                .await?;
            let page = parse_page(envelope, &LIST_KEYS, &evidence, false, false)?;
            retain_page(page, &LIST_KEYS, &evidence)
        })
    }

    /// Reads the public SSH key identified by the input.
    pub async fn get_ssh_key(
        &self,
        ctx: &RequestContext,
        input: GetSshKeyInput,
    ) -> Result<GetSshKeyOutput, Failure<GetSshKeyOutput>> {
        crate::wire::product::exit::finish!(async {
            GET_KEY
                .path("SSHKeyID", &input.ssh_key_id)
                .map_err(failure)?;
            let mut evidence = Evidence::new();
            let envelope = self
                .read(ctx, &GET_KEY, &[&input.ssh_key_id], &[], &mut evidence)
                .await?;
            let ssh_key: SshKey = envelope.field("data", &GET_KEY, &evidence)?;
            if ssh_key.id.is_empty() {
                return Err(failure(not_found(
                    &GET_KEY,
                    "ssh key",
                    &input.ssh_key_id,
                    &evidence,
                )));
            }
            Ok(GetSshKeyOutput {
                ssh_key: GET_KEY.retain(ssh_key, &evidence)?,
            })
        })
    }

    /// Reads one page of server groups.
    pub async fn list_server_groups(
        &self,
        ctx: &RequestContext,
        input: ListServerGroupsInput,
    ) -> Result<ListServerGroupsOutput, Failure<ListServerGroupsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            let page = self.group_page(ctx, &input, &mut evidence).await?;
            retain_page(page, &LIST_GROUPS, &evidence)
        })
    }

    pub(super) async fn group_page<O>(
        &self,
        ctx: &RequestContext,
        input: &ListServerGroupsInput,
        evidence: &mut Evidence,
    ) -> Result<PagedList<ServerGroup>, Failure<O>> {
        self.group_page_checked(ctx, input, evidence, false).await
    }

    pub(super) async fn group_page_checked<O>(
        &self,
        ctx: &RequestContext,
        input: &ListServerGroupsInput,
        evidence: &mut Evidence,
        reject_reflection: bool,
    ) -> Result<PagedList<ServerGroup>, Failure<O>> {
        let offset = input.offset.max(0).to_string();
        let limit = positive_or(input.limit, DEFAULT_PAGE_SIZE).to_string();
        let envelope = self
            .read(
                ctx,
                &LIST_GROUPS,
                &[],
                &[
                    ("limit", &limit),
                    ("name", &input.name),
                    ("offset", &offset),
                ],
                evidence,
            )
            .await?;
        if reject_reflection && envelope.reflects(evidence) {
            return Err(failure(group_read_error(LIST_GROUPS.id, evidence)));
        }
        parse_page(envelope, &LIST_GROUPS, evidence, true, false)
    }

    /// Reads the server group identified by the input.
    pub async fn get_server_group(
        &self,
        ctx: &RequestContext,
        input: GetServerGroupInput,
    ) -> Result<GetServerGroupOutput, Failure<GetServerGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            let group = self
                .group_detail(ctx, &input.server_group_id, &mut evidence)
                .await?;
            Ok(GetServerGroupOutput {
                server_group: GET_GROUP.retain(group, &evidence)?,
            })
        })
    }

    pub(super) async fn group_detail<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<ServerGroup, Failure<O>> {
        self.group_detail_checked(ctx, id, evidence, false).await
    }

    pub(super) async fn group_detail_checked<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
        reject_reflection: bool,
    ) -> Result<ServerGroup, Failure<O>> {
        GET_GROUP.path("ServerGroupID", id).map_err(failure)?;
        let envelope = self.read(ctx, &GET_GROUP, &[id], &[], evidence).await?;
        if reject_reflection && envelope.reflects(evidence) {
            return Err(failure(group_read_error(GET_GROUP.id, evidence)));
        }
        let group: ServerGroup = envelope.field("data", &GET_GROUP, evidence)?;
        if group.uuid.is_empty() {
            return Err(failure(not_found(&GET_GROUP, "server group", id, evidence)));
        }
        Ok(group)
    }

    /// Lists server-group placement policies and their localized descriptions.
    pub async fn list_server_group_policies(
        &self,
        ctx: &RequestContext,
        _: ListServerGroupPoliciesInput,
    ) -> Result<ListServerGroupPoliciesOutput, Failure<ListServerGroupPoliciesOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            let envelope = self
                .read(ctx, &LIST_POLICIES, &[], &[], &mut evidence)
                .await?;
            let rows: Vec<Policy> = envelope.field("data", &LIST_POLICIES, &evidence)?;
            let rows = LIST_POLICIES.retain(rows, &evidence)?;
            let items = rows
                .into_iter()
                .map(|row| ServerGroupPolicy {
                    name: row.name,
                    uuid: row.uuid,
                    status: row.status,
                    description: String::new(),
                    description_vi: String::new(),
                    descriptions: [("en", row.description), ("vi", row.description_vi)]
                        .into_iter()
                        .filter(|(key, _)| !evidence.text_reflects(key))
                        .map(|(key, value)| (key.into(), value))
                        .collect(),
                })
                .collect();
            Ok(List { items })
        })
    }

    /// Lists operating system images, optionally filtered by network zone.
    pub async fn list_os_images(
        &self,
        ctx: &RequestContext,
        input: ListOsImagesInput,
    ) -> Result<ListOsImagesOutput, Failure<ListOsImagesOutput>> {
        crate::wire::product::exit::finish!(async {
            let query = [("zoneId", input.zone_id.as_str())];
            self.images(
                ctx,
                &LIST_OS_IMAGES,
                if input.zone_id.is_empty() {
                    &[]
                } else {
                    &query
                },
            )
            .await
        })
    }

    /// Lists GPU images in the selected project.
    pub async fn list_gpu_images(
        &self,
        ctx: &RequestContext,
        _: ListGpuImagesInput,
    ) -> Result<ListGpuImagesOutput, Failure<ListGpuImagesOutput>> {
        crate::wire::product::exit::finish!(async { self.images(ctx, &LIST_GPU_IMAGES, &[]).await })
    }

    async fn images(
        &self,
        ctx: &RequestContext,
        read: &Read,
        query: &[(&str, &str)],
    ) -> Result<List<OsImage>, Failure<List<OsImage>>> {
        let mut evidence = Evidence::new();
        let envelope = self.read(ctx, read, &[], query, &mut evidence).await?;
        let items = envelope.field("images", read, &evidence)?;
        Ok(List {
            items: read.retain(items, &evidence)?,
        })
    }

    /// Reads one page of custom images.
    pub async fn list_user_images(
        &self,
        ctx: &RequestContext,
        input: ListUserImagesInput,
    ) -> Result<ListUserImagesOutput, Failure<ListUserImagesOutput>> {
        crate::wire::product::exit::finish!(async {
            let [page, size] = page_query(input.page, input.size);
            let mut evidence = Evidence::new();
            let envelope = self
                .read(
                    ctx,
                    &LIST_USER_IMAGES,
                    &[],
                    &[("page", &page), ("size", &size)],
                    &mut evidence,
                )
                .await?;
            let page = parse_page(envelope, &LIST_USER_IMAGES, &evidence, false, false)?;
            retain_page(page, &LIST_USER_IMAGES, &evidence)
        })
    }
}

pub(super) fn group_read_error(operation: &'static str, evidence: &Evidence) -> Error {
    api(
        operation,
        Some(200),
        "InvalidResponse",
        "invalid server group response; body withheld",
        evidence,
    )
}

fn not_found(read: &Read, noun: &str, id: &str, evidence: &Evidence) -> Error {
    let text = format!("vngcloud: not found: {}: {noun} {id}", read.id);
    Error::new(ErrorClass::NotFound).with_cause(api("", None, "NotFound", &text, evidence))
}

fn page_query(page: i64, size: i64) -> [String; 2] {
    [
        positive_or(page, DEFAULT_PAGE).to_string(),
        positive_or(size, DEFAULT_PAGE_SIZE).to_string(),
    ]
}

fn positive_or(value: i64, default: i64) -> i64 {
    if value > 0 { value } else { default }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn parse_page<T: WireField, O>(
    envelope: Envelope,
    read: &Read,
    evidence: &Evidence,
    strict: bool,
    guard: bool,
) -> Result<PagedList<T>, Failure<O>> {
    let items: Option<Vec<T>> = envelope.field("listData", read, evidence)?;
    let page: Option<i64> = envelope.field("page", read, evidence)?;
    let page_size: Option<i64> = envelope.field("pageSize", read, evidence)?;
    let total_page: Option<i64> = envelope.field("totalPage", read, evidence)?;
    let total_item: Option<i64> = envelope.field("totalItem", read, evidence)?;
    if guard
        && (items.is_none()
            || page.is_none()
            || page_size.is_none()
            || total_page.is_none()
            || total_item.is_none())
    {
        return Err(failure(super::wire::invalid_input(
            "compute.CreateServer: could not confirm no server has the requested name: incomplete inventory",
        )));
    }
    if strict && !valid_page(items.as_deref(), page, page_size, total_page, total_item) {
        return Err(failure(read.invalid(evidence)));
    }
    Ok(PagedList {
        items: items.unwrap_or_default(),
        page: page.unwrap_or_default(),
        page_size: page_size.unwrap_or_default(),
        total_page: total_page.unwrap_or_default(),
        total_item: total_item.unwrap_or_default(),
    })
}

fn valid_page<T>(
    items: Option<&[T]>,
    page: Option<i64>,
    size: Option<i64>,
    pages: Option<i64>,
    total: Option<i64>,
) -> bool {
    let (Some(items), Some(page), Some(size), Some(pages), Some(total)) =
        (items, page, size, pages, total)
    else {
        return false;
    };
    let count = i64::try_from(items.len()).unwrap_or(i64::MAX);
    (page > 0 || (page == 0 && pages == 0 && total == 0))
        && size > 0
        && pages >= 0
        && total >= count
        && count <= size
        && (count == 0 || pages >= page)
        && (total != 0 || pages <= 1)
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn retain_page<T: WireField, O>(
    mut page: PagedList<T>,
    read: &Read,
    evidence: &Evidence,
) -> Result<PagedList<T>, Failure<O>> {
    read.retain(page.page, evidence)?;
    read.retain(page.page_size, evidence)?;
    read.retain(page.total_page, evidence)?;
    read.retain(page.total_item, evidence)?;
    page.items = read.retain(page.items, evidence)?;
    Ok(page)
}
