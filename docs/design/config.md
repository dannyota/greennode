# Configuration and credentials

Status: proposed.

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
  These sources may refuse a write without reading any file. Otherwise finish
  the check offline with `C.read_only`, using the same rules. When several
  consulted sources are on, name the first in table order. Reads do not fail
  solely because the latch is on or invalid. The SDK does not enforce the CLI
  latch; the baseline's method-specific checks of the profile key in vMonitor
  are unchanged.
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
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/design/sdk-and-cli.md#configuration-and-credentials
[baseline-auth]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/design/service-account-auth.md

## File mode and paths

Choose a mode once. A non-empty SDK path option, a non-empty `GREENNODE_` path
variable, or an entry at either greennode default selects **greennode mode**.
Otherwise select **legacy mode**. A path option or greennode variable decides
that choice without probing unselected defaults. There are no CLI path flags. An
explicit profile does not affect either path rule. A home directory that cannot
be resolved supplies no defaults in either mode; a relative override is relative
to the invocation's working directory, not to either home subdirectory.

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
| Neither; no greennode default entry           | Vd, Vd                  | Vc, Vd    | Vd, Vk    | Vc, Vk                |
| Neither; at least one greennode default entry | Gd, Gd                  | Gd, Gd    | Gd, Gd    | Gd, Gd                |
| `Gc` only                                     | Gc, Gd                  | Gc, Gd    | Gc, Gd    | Gc, Gd                |
| `Gk` only                                     | Gd, Gk                  | Gd, Gk    | Gd, Gk    | Gd, Gk                |
| Both                                          | Gc, Gk                  | Gc, Gk    | Gc, Gk    | Gc, Gk                |

An entry is present when lookup without following the leaf finds it, including
an unreadable file, directory, or link (even dangling). Only a missing leaf or
missing ordinary parent is absent. A permission error or other ambiguous lookup
at a greennode default is a configuration error, not permission to fall back. A
present but unsafe entry selects a mode and is refused if selected for use;
paths overridden in that mode are not read. Failure to open a path already
observed present is never treated as absence. Missing named paths are errors;
missing defaults supply no contents, subject to the following guard.

| Selected greennode pair after overrides | Legacy file indication       | Result                                                               |
| --------------------------------------- | ---------------------------- | -------------------------------------------------------------------- |
| Both files present                      | Any                          | Intentional cutover; skip the incomplete-pair guard                  |
| One file absent                         | None                         | Use only the selected file                                           |
| One file absent                         | Either legacy slot indicated | `InvalidConfig` (exit 2); complete the pair or return to legacy mode |

For this guard only, a legacy slot is indicated by a non-empty legacy path
variable or by an entry at its legacy default. An ambiguous legacy default
lookup counts as indicated. Read only a path-variable presence bit, not the
value as a path. Do not open, parse, or use legacy files in greennode mode.
Check both slots: even a legacy file of the same kind as the sole greennode file
can supply a different account or a missing write restriction. Two selected
overrides can complete the pair even with only one default present. A named path
that is absent or unsafe is an error even if the other slot is available. Never
switch modes or retry another source on an error.

## Offline stages and file safety

1. `Config::resolve_profile` selects the profile name, mode and paths. It checks
   path presence and type in both slots and runs the incomplete-pair guard. A
   missing explicit credentials path, or an entry there that is not a regular
   file once links are followed, is refused now. It checks and parses the config
   file, if present. It resolves the latch from the flag, both variables and the
   selected config; it resolves output from the flag or the selected config. It
   returns immutable settings, path decisions and provenance. It requires no
   region or credentials and never signs in, checks the token cache, reads
   management credential variables (not even their presence), or opens the
   credentials file for content. It does not require the profile to exist in
   either file. An offline-only command can apply the latch without building a
   management client.
2. `Config::load_resolved` uses the selected paths and settings without
   rereading them or choosing a mode again. Open and validate the credentials
   file if present, then check profile existence, resolve and validate the
   management region, project and credential inputs, and resolve the vCDN key.
   Check the credentials file on the opened handle. This step still sends no
   request. Only the management path runs it. Standalone discovery can inspect
   settings first and load management credentials only if its request needs
   them. S3 object commands never call it and never use the management token
   cache.

Apply these rules to either mode, including any file `configure` reads:

- Open only regular files for content. On Unix, open nonblocking so a FIFO
  swapped into place cannot hang a read. Check type and permissions on the open
  handle, and parse from that handle. Whatever the open reaches is what gets
  checked, so a swap between the path check and the open cannot skip a check. On
  Windows, refuse devices, pipes, and other non-regular entries before reading.
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

The Go loader follows symlinks, requires an opened regular file, and limits INI
lines, not total file bytes. greennode keeps the first two. Bounding total bytes
and refusing invalid UTF-8 are deliberate tightenings. The 1 MiB file cap
matches the Go `configure` reader; greennode uses strict SDK parsing and the
credentials-mode check there as well. The [security design](security.md) owns
the separate cache checks.

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
selected greennode files, subject to the same read-only latch. They cannot clear
it. If no home is known, refuse writes rather than assuming a target is outside
the legacy tree. The CLI design owns prompts, writes, and locking.

In legacy mode every `configure` write exits 2. After choosing legacy mode,
refuse before reading profile contents for the write, prompting, consuming
stdin, or creating any file, directory, or lock. No refusal changes either file
pair or the cache. Say that greennode will not edit legacy settings, that no
files changed, and that the user must copy the selected files manually. Give
these commands for a POSIX shell. Tell the user to use the same working
directory as the failed command, check the sources with
`greennode configure list`, verify the destination is not a link, avoid
overwriting either greennode file, and stop on error:

```sh
umask 077
mkdir -p "$HOME/.greennode"
cp "${VNGCLOUD_CONFIG_FILE:-$HOME/.vngcloud/config}" \
  "$HOME/.greennode/config"
cp "${VNGCLOUD_SHARED_CREDENTIALS_FILE:-$HOME/.vngcloud/credentials}" \
  "$HOME/.greennode/credentials"
chmod 600 "$HOME/.greennode/config" "$HOME/.greennode/credentials"
```

If a legacy default is genuinely absent and no override supplies it, create its
empty counterpart in `~/.greennode` instead of copying. Never substitute an
empty file for an unreadable source or missing explicit path. Preserve
`read_only` and verify the copied settings with `greennode configure list`
before retrying a write. If the home cannot be resolved, tell the user to set it
first. On Windows give the same source and destination file names and tell the
user to copy them with access limited to their account.

## Token cache and debug

The tool opts into `~/.greennode/cache` in either mode, never vngcloud's cache.
No home means no tool disk cache, not a home or credential error on its own. An
SDK caller gets no disk cache by default, even with a home. The caller can opt
in with an explicit cache directory without needing a home. The caller must
choose a private directory that does not share vngcloud's cache. Static tokens
and custom providers do not use the disk cache. Cache mechanics and auth-kind
namespaces follow the baseline designs, unchanged except for the tool's
location. An offline profile lookup or a `configure` command never opens a
cache. [Security](security.md) owns cache path safety and request debug events.

With `--debug`, configuration provenance on stderr may name a fixed setting, the
chosen mode, and a fixed source label: option or flag, the exact variable name,
the winning management environment prefix as a set, `~/.greennode`,
`~/.vngcloud`, or no source. For a file override, use only the option or
variable name. For the latch, sources that turned it on may be named. Do not log
profile names, region, project ID, output value, other setting values, resolved
paths, override paths, file contents, or raw filesystem errors.

## Tests

Use injected process state, temporary files, and toy credentials. Track which
sources were consulted and count requests, including sign-in and discovery.
Drive every eligible precedence row with distinct inputs: start with all sources
supplied, remove each winner in turn, and assert the next winner or missing
result. Repeat file cases in both modes and with empty winning values.

| Rule                   | Legitimate case                                                                                                                                                    | Hostile or conflicting case                                                                                                                                                                                                                                                                      |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Explicit profile       | Each prefix's `PROFILE` stays non-explicit; option or flag still wins                                                                                              | Explicit profile skips inherited credentials, project ID and vCDN key from either prefix, but retains region, paths and the read-only latch                                                                                                                                                      |
| Project and region     | Matching or empty `C.region` uses its project; region and management credentials can come from opposite prefixes                                                   | Differing region drops only the file project ID; an inherited project ID wins only when eligible                                                                                                                                                                                                 |
| Whole management set   | A `VNGCLOUD_ACCESS_TOKEN` beats greennode profile credentials without an explicit profile; provider, static, service account and IAM precedence hold in one source | Partial G IAM or service account cannot be filled by V or files; a V token cannot beat a populated G set; failed sign-in never falls through                                                                                                                                                     |
| Independent CDN key    | Greennode IAM with `VNGCLOUD_VCDN_API_KEY` works when not explicit; explicit key option wins                                                                       | A key never fills absent management credentials; an invalid winning key does not fall back                                                                                                                                                                                                       |
| Latch before requests  | Each on source independently refuses writes; reads still run                                                                                                       | `--read-only=false` and either false variable cannot clear a latch; invalid text fails a write with zero requests                                                                                                                                                                                |
| Paths and mode         | Exercise every path-table cell and SDK option substitution; complete overrides ignore a lone default and bad legacy overrides                                      | Directory, unreadable path, missing explicit path, or dangling link never turns into fallback; a `configure` write whose target resolves under `~/.vngcloud` is refused                                                                                                                          |
| Incomplete pair        | Missing defaults succeed with no legacy file footprint                                                                                                             | Missing opposite slot: new credentials plus old config with `read_only=1`. Same-slot latch: new and old config only, env credentials. Same-slot identity: new and old credentials only, env region. Repeat with a legacy path variable instead of the old default; all refuse with zero requests |
| Offline boundary       | S3 settings resolve without region or auth; an available file can supply output and the latch                                                                      | Unusable selected credentials path refuses offline; unsafe opened permissions and malformed credentials content refuse only when read; assert no management variable lookup or credentials-content open for S3                                                                                   |
| File checks and bounds | Read 0600 credentials and 0644 config, valid BOM and repeated sections, and files at the byte cap; skip mode-bit checks on Windows                                 | Refuse Unix group write or execute even without group read, a FIFO swapped for the file, the line or file cap plus one, and malformed data without echo                                                                                                                                          |
| Configure              | In each mode `get` and `list` show stored values, masked, even with latch on; greennode writes stay in the selected pair                                           | Legacy write gives the copy commands; a read-only or unsafe-target write does not prompt, consume stdin or change files, locks or cache                                                                                                                                                          |
| Cache and debug        | CLI cache path stays greennode in both modes; SDK opt-in works without a home; debug shows fixed provenance                                                        | With no home, CLI and default SDK never touch disk cache; hostile values and override paths do not reach debug output                                                                                                                                                                            |
