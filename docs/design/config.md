# Configuration and credentials

greennode selects one file pair and one management credential source. A legacy
file can supply settings but cannot be edited by greennode. The SDK and CLI
share resolution, and a CLI write checks read-only before any request.

## Approaches

- Use only greennode settings. The account is easy to trace, but existing
  vngcloud users have to configure the tool again.
- Fill each missing file or key from vngcloud. This eases migration but can join
  another account's credentials to a profile or lose its write latch.
- **Use one file mode with explicit environment precedence.** Existing users can
  start in legacy mode. A switch to greennode files cannot silently use half of
  the old pair. No command copies files to switch modes.

## Resolution

The SDK offers an options-only constructor that reads nothing, and an offline
load from options, environment, and files. Only `main.rs` and the SDK's default
configuration source read the process environment and home. The CLI passes an
injected source and a running clock. The names below identify options by their
baseline spelling, not Rust signatures.

Read each row from left to right. `G_` means `GREENNODE_`; `V_` means
`VNGCLOUD_`. `C` is the selected profile's config section; `K` is its
credentials section. Only the file columns for the chosen mode are eligible. A
dash means no source. An empty string or absent option does not win. Do not trim
options or environment strings to decide absence. The INI parser trims file
values as in the baseline.

| Setting           | Option or flag                   | G environment                                               | V environment                                               | Greennode files                                             | Legacy files  | If none wins                                      |
| ----------------- | -------------------------------- | ----------------------------------------------------------- | ----------------------------------------------------------- | ----------------------------------------------------------- | ------------- | ------------------------------------------------- |
| Profile           | `WithProfile` / `--profile`      | `G_PROFILE`                                                 | `V_PROFILE`                                                 | -                                                           | -             | `default`                                         |
| Region            | `WithRegion` / `--region`        | `G_REGION`                                                  | `V_REGION`                                                  | `C.region`                                                  | `C.region`    | Required for management config                    |
| Project ID        | `WithProjectID` / `--project-id` | `G_PROJECT_ID`                                              | `V_PROJECT_ID`                                              | `C.project_id` if region fits                               | Same          | Discover when needed                              |
| Custom provider   | `WithCredentialsProvider`        | -                                                           | -                                                           | -                                                           | -             | See management set rule                           |
| Static token      | `WithStaticToken`                | `G_ACCESS_TOKEN`                                            | `V_ACCESS_TOKEN`                                            | -                                                           | -             | See management set rule                           |
| Service account   | `WithServiceAccount`             | `G_CLIENT_ID`, `G_CLIENT_SECRET`                            | `V_CLIENT_ID`, `V_CLIENT_SECRET`                            | `K.client_id`, `K.client_secret`                            | Same          | See management set rule                           |
| IAM user          | `WithIAMUser`                    | `G_ROOT_EMAIL`, `G_USERNAME`, `G_PASSWORD`, `G_TOTP_SECRET` | `V_ROOT_EMAIL`, `V_USERNAME`, `V_PASSWORD`, `V_TOTP_SECRET` | `K.root_email`, `K.username`, `K.password`, `K.totp_secret` | Same          | See management set rule                           |
| vCDN API key      | `WithCDNAPIKey`                  | `G_VCDN_API_KEY`                                            | `V_VCDN_API_KEY`                                            | `K.vcdn_api_key`                                            | Same          | Absent; a vCDN operation that requires it refuses |
| Read-only (latch) | `--read-only`                    | `G_READ_ONLY`                                               | `V_READ_ONLY`                                               | `C.read_only`                                               | `C.read_only` | Off                                               |
| Output            | `--output`                       | -                                                           | -                                                           | `C.output`                                                  | `C.output`    | `json`                                            |

Only a non-empty `WithProfile` or `--profile` makes the profile explicit.
Neither `PROFILE` variable makes it explicit.

| With an explicit profile                     | Effect                                                                                 |
| -------------------------------------------- | -------------------------------------------------------------------------------------- |
| Profile selection                            | The option or flag wins                                                                |
| Management credentials, project ID, vCDN key | Skip both environment columns; explicit options and the selected files remain eligible |
| Region, read-only, output, and both paths    | No change in source eligibility                                                        |

- A file project ID applies only when `C.region` equals the resolved region or
  `C.region` is empty. An option or eligible environment project ID does not
  have that test. Discovery never switches credentials.
- **Management set rule.** Choose the first eligible source that has any
  management credential: options, G environment, V environment, then `K`. A
  supplied provider or auth struct reserves the options source even when the
  auth fields are empty. Within the winning source, choose provider, static
  token, service account, then IAM user, independent of option order. A file has
  no static-token key. Service accounts require both fields; IAM users require
  root email, username, and password. TOTP is optional, but a TOTP value alone
  still selects a partial set. Validate only the winning method. A partial,
  invalid, or refused winner never falls through. A load with no winner or
  missing required fields returns `NoCredentials` (exit 3); an invalid field
  shape follows the baseline validation and errors.
- Resolve the vCDN key separately, even when the management source has a
  different prefix. The key cannot satisfy the management credential
  requirement. A malformed winning key is a configuration error, not a reason to
  try another key. The baseline key validation is unchanged.
- **Read-only latch.** OR the flag, both variables, and `C.read_only`, not the
  credentials section or another profile. Trim the strings; `1` and `true` turn
  the source on, `0`, `false`, and empty leave it off, ignoring case.
  `--read-only=false` leaves its own source off. For writes, validate both
  variables before deciding the latch from the flag and environment. An invalid
  value is `InvalidConfig` (exit 2); a valid on source is `ReadOnly` (exit 2).
  When several consulted sources are on, name the first in table order. For a
  management write, the flag or environment refuses before any configuration or
  credentials file is read. Otherwise finish the full offline load and resolve
  output before applying `C.read_only`. A load failure wins over the profile
  latch: malformed credentials with `C.read_only=1` return `NoCredentials` (exit
  3), not `ReadOnly`. Nothing in this sequence signs in. Ordinary management
  reads do not fail solely because the latch is on or invalid. S3 has the policy
  checks below. The SDK does not enforce the CLI latch; the baseline's
  method-specific checks of the profile key in vMonitor are unchanged.
- The CLI accepts `json`, `table`, and `text`; a different winning output value
  refuses before any request. The SDK preserves the profile setting without
  interpreting it as a rendering option.

Profile name validation, section naming, management config validation and
project discovery follow the baseline's [SDK and CLI design][baseline-sdk] and
[service-account design][baseline-auth], unchanged. A management load requires a
named profile other than `default` to appear in at least one of the selected
files, even if options supply all required values. The config section reserves
non-empty `client_id` and `client_secret`; refuse them before exposing settings.

[baseline-sdk]:
  https://github.com/dannyota/vngcloud/blob/v1.2.0/docs/design/sdk-and-cli.md#configuration-and-credentials
[baseline-auth]:
  https://github.com/dannyota/vngcloud/blob/v1.2.0/docs/design/service-account-auth.md

## File mode and paths

Choose a mode once:

1. A non-empty SDK path option, a non-empty `GREENNODE_` path variable, or an
   entry at either greennode default selects **greennode mode**. A path option
   or variable decides this without probing unselected default paths.
2. Otherwise, a non-empty `VNGCLOUD_` path variable or an entry at either legacy
   default selects **legacy mode**.
3. If neither tool has a file or path override, select **greennode mode**. Both
   greennode default files may be absent on a fresh install.

There are no CLI path flags. An explicit profile does not change path selection.
No home means no default paths. Resolve relative overrides against the
invocation's working directory, not either tool directory.

| File        | SDK option (selects greennode mode) | Greennode path order after option                                    | Legacy path order                                                  |
| ----------- | ----------------------------------- | -------------------------------------------------------------------- | ------------------------------------------------------------------ |
| Config      | `WithConfigFile`                    | `GREENNODE_CONFIG_FILE`, then `~/.greennode/config`                  | `VNGCLOUD_CONFIG_FILE`, then `~/.vngcloud/config`                  |
| Credentials | `WithSharedCredentialsFile`         | `GREENNODE_SHARED_CREDENTIALS_FILE`, then `~/.greennode/credentials` | `VNGCLOUD_SHARED_CREDENTIALS_FILE`, then `~/.vngcloud/credentials` |

This table covers the path variable combinations before SDK options. Each cell
lists the config source and then the credentials source. `Gd` and `Vd` mean the
corresponding default path. `Gc`, `Gk`, `Vc`, and `Vk` mean the corresponding
config or credentials variable above. Any SDK path option forces greennode mode
and replaces its slot, regardless of the table's choice.

| Non-empty greennode path variables            | Neither legacy variable | `Vc` only | `Vk` only | Both legacy variables |
| --------------------------------------------- | ----------------------- | --------- | --------- | --------------------- |
| Neither; no default entry for either tool     | Gd, Gd                  | Vc, Vd    | Vd, Vk    | Vc, Vk                |
| Neither; only legacy has a default entry      | Vd, Vd                  | Vc, Vd    | Vd, Vk    | Vc, Vk                |
| Neither; at least one greennode default entry | Gd, Gd                  | Gd, Gd    | Gd, Gd    | Gd, Gd                |
| `Gc` only                                     | Gc, Gd                  | Gc, Gd    | Gc, Gd    | Gc, Gd                |
| `Gk` only                                     | Gd, Gk                  | Gd, Gk    | Gd, Gk    | Gd, Gk                |
| Both                                          | Gc, Gk                  | Gc, Gk    | Gc, Gk    | Gc, Gk                |

An entry is present when lookup without following the leaf finds it, including
an unreadable file, directory, or link (even dangling). Only a missing leaf or
missing ordinary parent is absent. A permission error or other ambiguous lookup
cannot establish a default's absence. If needed to choose the mode, stop with a
configuration error. A present but unsafe entry selects a mode and is refused if
selected for use; paths overridden in that mode are not read. Failure to open a
path already observed present is never treated as absence. Missing named paths
are errors; missing defaults supply no contents, subject to this guard.

| Selected greennode pair after overrides | Legacy file indication       | Result                                                               |
| --------------------------------------- | ---------------------------- | -------------------------------------------------------------------- |
| Both files present                      | Any                          | Intentional cutover; skip the incomplete-pair guard                  |
| Either or both files absent             | None                         | Use the files that are present                                       |
| Either or both files absent             | Either legacy slot indicated | `InvalidConfig` (exit 2); complete the pair or return to legacy mode |

For this guard, a legacy slot is indicated by a non-empty legacy path variable,
an entry at its default, or an ambiguous lookup at that default. Read only a
path-variable presence bit, not the value as a path. Do not open or parse an
unselected legacy file. Check both slots: even a legacy file of the same kind as
the sole greennode file can supply a different account or a missing write
restriction. A complete pair can come from overrides even if a default is
absent. Never switch modes or retry another source on error.

## Offline resolution and file safety

`Config::load(ctx, options, source)` performs one offline management load. It
selects the mode, paths and profile, opens each present file once, and parses
from those handles. It validates the profile, region and winning credentials and
resolves the project ID and vCDN key. The returned config carries the selected
profile settings, including the raw latch. It does not sign in or create cache
files. There is no public second stage and no way to combine settings from one
load with credentials from another.

`Config::resolve_profile(ctx, &options, source)` returns settings, section
presence, mode, path decisions and provenance, not a management config. It
selects the files and checks metadata for both slots, including the
incomplete-pair guard and alias check, but parses only the config file. A
missing explicit credentials path or a target that is not a regular file is an
error even here. It does not require a region, check the token cache, look up
management credential variables, or open the credentials file for content.

| Settings-only caller                              | Named non-default section required in config?   | Read-only policy                                                                                |
| ------------------------------------------------- | ----------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| S3 object commands and other policy-only callers  | Yes; absent `default` is allowed                | Reject malformed consulted settings on reads and writes. A valid on source refuses writes only. |
| `configure get` and `configure list`              | No; report absent values as empty               | Inspection does not validate the latch; show the stored value                                   |
| `configure` and `configure set`                   | No; a write can create a profile                | Validate and enforce the latch before editing in greennode mode                                 |
| Standalone project discovery, local settings only | No extra check; not authorization for a request | Use normal management read rules; an authenticated request needs its own full `Config::load`    |

S3 applies both checks before reading the S3 key or sending any request. With no
home and no config override, S3 refuses rather than assuming empty policy. S3
never uses a management config or cache. The management command
`project list-projects` keeps ordinary management profile rules: a named profile
can be in either file, and a malformed latch does not stop a read.

A directory swap during one load can yield files from different deployments;
each opened file is checked, but the load is not an atomic snapshot.

Apply these rules to either mode, including any file `configure` reads:

- Reject a config/credentials alias with `InvalidConfig` before parsing the
  config, including through hard links or distinct symlinks. Compare identities
  of opened handles: device and inode on Unix, and on Windows the volume serial
  number and file index through the `same-file` crate (MIT or Unlicense, safe
  API), since the standard library's Windows accessors are unstable. Test two
  names for one file on each platform. Management loads compare the opened
  files' identities. Settings-only lookups compare the opened config's identity
  with metadata for the resolved credentials target without opening that target
  for content.
- Open only regular files for content. On Unix, open nonblocking so a FIFO
  swapped into place cannot hang a read. Check type and permissions on the
  opened handle, not just from metadata collected before opening. On Windows,
  refuse devices, pipes, and other non-regular entries before reading.
- On Unix, refuse an opened credentials file if **any** group or other
  permission bit is set (`mode & 0o077 != 0`). Suggest `chmod 600`; the file
  need not have exactly mode 0600. Check this only when opening to read the
  file, including `configure`. Windows skips this mode-bit check. Both platforms
  require an openable regular file. This is not an ownership or ACL check.
  Config files get the same type, path and parsing checks, not this mode check.
  A group-readable config is not a credentials file.
- A selected path may be a symbolic link, as in the baseline. The checks apply
  to the file the open reaches, and a user who links a greennode path to another
  file has chosen that file. Writes are stricter: no CLI write target, including
  a lock or temporary file, may resolve under `~/.vngcloud`, even when a link or
  an override points there. Resolve the real parent directory before writing and
  compare it with the real `~/.vngcloud`, not by string prefix.
- Cap each INI file at 1 MiB of input, 2 MiB for the pair. Read at most one
  extra byte per file to detect overflow, regardless of the reported size. Keep
  the baseline 64 KiB scanner buffer limit per line, including the line ending.
  An unterminated final line must be shorter than 64 KiB. Keep the INI grammar;
  reject malformed input outright, including invalid UTF-8. Report the slot and
  source label, and a line number for a parse error, not line content or an
  override path. Config errors use `InvalidConfig` (exit 2); credentials-file
  failures keep the credentials-file identity (`NoCredentials`, exit 3).
  Mode-selection and incomplete-pair errors are `InvalidConfig`.

## Configure

`configure` does not build a management client. Reads use the same selection and
file safety rules in both modes. `get <key>` prints the selected profile's
stored value from that key's file; `list` prints all 11 stored keys. Neither
shows environment or option overrides as values, requires working credentials,
or performs discovery. Missing values print empty. A non-empty `password`,
`totp_secret`, `vcdn_api_key`, or `client_secret` prints `****`. A get of a
config key needs no credentials content; a get of a credentials key or `list`
opens and checks that file without selecting auth. With no home, a requested
file whose path has no override is a configuration error for `get` or `list`.

In greennode mode, interactive `configure` and `configure set` edit only the
selected greennode files, subject to the latch. They cannot clear it. On a fresh
install with no file indication for either tool, run `greennode configure`, or
start with `greennode configure set region <region>`. No files need to be copied
or pre-created. Under the write locks, resolve the mode, both selected paths and
the latch again. Refuse the write if the mode, either real path, or the latch
differs from the result before prompting. A write with no home refuses rather
than assuming its target is outside the legacy tree. The CLI design owns
prompts, publication and locking.

In legacy mode every `configure` write exits 2. After choosing legacy mode,
refuse before reading profile contents for the write, prompting, consuming
stdin, or creating any file, directory, or lock. No refusal changes either pair
or the cache. Say why, say that no files changed, and give this manual copy
recipe for macOS and Linux. Tell the user to stay in the same working directory,
check the sources with `greennode configure list`, and verify the real
destination is outside the legacy tree. Use their private directory and do not
run another configuration editor during the copy.

```sh
(
  set -eu
  umask 077
  d="$HOME/.greennode"
  mkdir -p "$d"
  test ! -L "$d"
  for f in config credentials; do
    test ! -e "$d/$f" && test ! -L "$d/$f" || exit 1
  done
  t=$(mktemp -d "$d/.migration.XXXXXX")
  trap 'rm -f "$t/config" "$t/credentials"; rmdir "$t"' 0
  cat < "${VNGCLOUD_CONFIG_FILE:-$HOME/.vngcloud/config}" > "$t/config"
  cat < "${VNGCLOUD_SHARED_CREDENTIALS_FILE:-$HOME/.vngcloud/credentials}" \
    > "$t/credentials"
  link "$t/config" "$d/config"
  link "$t/credentials" "$d/credentials"
)
```

Stage both copies before publishing either. `link` fails if the exact
destination already exists. If a legacy default is genuinely absent and no
override supplies it, replace its copy line with `: > "$t/config"` or
`: > "$t/credentials"`. Never substitute an empty file for an unreadable source
or a missing override. If only one publication succeeded, keep the legacy
sources in place and complete or undo the copy before running commands. Verify
the copied settings and preserved `read_only` with `greennode configure list`.
If the home cannot be resolved, set it first. On Windows, name the same sources
and destinations and require the same private staging and no-overwrite copy.

## Token cache and debug

The tool opts into `~/.greennode/cache` in either mode. SDK callers opt in with
an explicit directory. Static tokens and custom providers never use the disk
cache. An offline profile lookup or `configure` never opens a cache.

For both built-in auth kinds, when `Config::load` has a home from its source,
resolve the cache directory (default or explicit) and `~/.vngcloud` to their
real locations. Refuse a cache equal to or inside the real legacy tree before
creating a directory or lock, reading a token or marker, or writing anything.
Resolve the existing ancestors even if the cache does not exist yet. Compare
filesystem ancestry, not string prefixes, and anchor disk operations to the
checked directory. If separation cannot be established safely, refuse cache use.
Open the checked directory once without following its final component, require
it owned by the user and not group- or world-writable, and do every later step
relative to that handle. An ancestor changed between the check and that open is
an accepted limit: it needs write access to the owner's own directories. On
Windows the standard library offers no handle-relative create or rename, so the
disk cache is off there for now and tokens stay in memory for the process. A
cache entry whose token needs a JSON escape, or that is not valid UTF-8, is a
miss, so token bytes stay in zeroizing storage; Go would accept both. This
supplements, not replaces, the baseline's checks on the cache directory and
entries.

Without a home, the tool has no default disk cache. `Config::load` with no home
and `Config::new` have no legacy path to compare with; they use an explicit
cache directory as given, with the normal cache checks. A caller in that case
must choose a private directory that does not share vngcloud's cache.
`Config::new` does not read the home to check this. Cache mechanics and
auth-kind namespaces are otherwise unchanged. [Security](security.md) owns
cache-entry checks and request debug events.

With `--debug`, configuration provenance on stderr may name a fixed setting, the
chosen mode, and a fixed source label: option or flag, the exact variable name,
the winning management environment prefix as a set, `~/.greennode`,
`~/.vngcloud`, or no source. For a file override, use only the option or
variable name. For the latch, sources that turned it on may be named. Do not log
profile names, region, project ID, output value, other setting values, resolved
paths, override paths, file contents, or raw filesystem errors.

## Differences from the baseline

- Add the greennode paths and prefix, one legacy fallback mode, and the
  incomplete-pair guard. Both read-only variables can latch a write.
- Keep regular-file reads through symlinks. Reject shared config/credentials
  identities and selected dangling links, even at default locations. Load
  strictly as UTF-8; bound the total INI input as well as the line size.
  `configure` uses the same strict checks, including the credentials mode check.
  Policy-only reads check both path identities without reading management
  credentials.
- Let a fresh home configure directly. Refuse legacy edits and any write
  destination in the real legacy tree, including temp and lock files. Migration
  is manual. Keep a separate default disk cache and check cache paths against
  that tree when home is known.
- Use fixed, value-free source labels in diagnostics and debug provenance.
- Treat a relative home directory as no home, and anchor every relative path
  override to the caller's working directory before probing; refuse a
  drive-relative or root-relative Windows path (`C:config`, `\config`). On
  Windows only file-not-found and path-not-found count as absent.
- On Windows, open selected files with identification-only security quality of
  service and refuse a handle that is not a disk file before reading, so a named
  pipe at a selected path can neither impersonate the caller nor stall a read.

Known limits: two distinct ReFS files can share the identity `same-file` reads,
which refuses the configuration as an alias rather than reading the wrong file;
closing it needs unsafe Windows calls the project forbids. Keys, not section
names, are lowercased, with Rust's Unicode mapping, which differs from Go's for
a few non-ASCII characters; no recognized key is non-ASCII.

## Tests

Use injected process state, temporary files, and toy credentials. Track which
sources were consulted and count requests, including sign-in and discovery.
Drive every eligible precedence row with distinct inputs: start with all sources
supplied, remove each winner in turn, and assert the next winner or missing
result. Repeat file cases in both modes and with empty winning values.

| Rule                      | Legitimate case                                                                                                                                     | Hostile or conflicting case                                                                                                                                                                       |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Explicit profile          | Each prefix's `PROFILE` stays non-explicit; option or flag still wins                                                                               | Explicit profile skips inherited credentials, project ID and vCDN key from both prefixes, but retains region, paths and the latch                                                                 |
| Project and region        | Matching or empty `C.region` uses its project; region and credentials can come from opposite prefixes                                               | Differing region drops only the file project ID; an inherited project ID wins only when eligible                                                                                                  |
| Whole management set      | A legacy environment token beats greennode file credentials without an explicit profile; method precedence holds in one source                      | Partial G IAM or service account cannot be filled by V or files; a V token cannot beat a populated G set; failed sign-in never falls through                                                      |
| Independent CDN key       | Greennode IAM with legacy environment key works when not explicit; explicit key option wins                                                         | A key never fills absent management credentials; an invalid winning key never falls back                                                                                                          |
| Management latch ordering | Valid files with `C.read_only=1` refuse the write; reads still run                                                                                  | Flag/env latch refuses before file reads. With only `C.read_only=1` and malformed credentials, return `NoCredentials`/3. No request in either refusal. False sources cannot clear a true one.     |
| Paths and mode            | Exercise path-table cells and SDK option substitution. Fresh home with no path vars selects greennode. A legacy path override alone selects legacy. | Unreadable or dangling paths do not trigger fallback; a write target resolving into the legacy tree is refused.                                                                                   |
| Incomplete pair           | A missing default is allowed without a legacy indication. Complete overrides ignore legacy path variables.                                          | New credentials with old config, new and old config only, and new and old credentials only all refuse. Repeat with non-empty legacy path variables instead of legacy defaults.                    |
| Identity and aliases      | Read a stable linked pair through regular opened files                                                                                              | Point both paths at one file through symlinks or hard links. Both readers refuse before parsing the config.                                                                                       |
| S3 policy                 | Absent default or an existing named config section works without management credentials; true latch permits reads                                   | Missing named section (even if named in credentials) or malformed latch refuses before reading the S3 key. Assert no management variable lookup, credentials content read, cache use, or request. |
| File checks and bounds    | Read 0600 credentials and 0644 config, valid BOM and repeated sections, and files at the cap. Skip mode-bit checks on Windows.                      | Refuse group-only write or execute bits on Unix, a FIFO swapped for the file, invalid UTF-8, and the line or file cap plus one. Never echo content.                                               |
| Configure                 | First configure succeeds in an empty home. Get/list show stored and masked values for a missing or read-only profile in either mode.                | Legacy write prints the recipe without side effects. Refuse if mode, real paths, or latch change during a prompt. Copy never overwrites or publishes a partially read source.                     |
| Cache and debug           | Both built-in auth kinds use a separate resolved cache; a private SDK opt-in works without a home                                                   | An ancestor link into the legacy tree refuses both built-in auth kinds for the CLI default and SDK explicit paths before cache I/O. Debug prints no values or paths.                              |
