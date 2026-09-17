# zoterable

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/gbiagini/zoterable?sort=semver)](https://github.com/gbiagini/zoterable/releases)

**Automatically sync the PDFs in your Zotero library to your reMarkable tablet**,
renamed to a clean `Author - Year - Title` from the Zotero metadata.

It talks directly to the [Zotero Web API](https://www.zotero.org/support/dev/web_api/v3/start)
and the reMarkable cloud, so syncing works with the Zotero desktop app closed and
no cable plugged in — ideal for running on a schedule in the background.

- **Incremental.** Only items changed since the last run are fetched (via the
  Zotero `since` parameter), so a large library syncs in a single fast request.
- **Two-way sync.** New PDFs go from Zotero to reMarkable, and when a synced
  document changes on reMarkable, the updated PDF package is pushed back to
  the same Zotero attachment.
- **Folder mirroring.** reMarkable folders are created to match the Zotero
  collection path (using the first collection on the parent item).
- **Multi-library.** Syncs your personal library and any number of group
  libraries.
- **Nice filenames.** Papers arrive as `Author - Year - Title` instead of
  `1234ABCD.pdf`, built from the parent item's authors, date, and title.

> **Note.** zoterable is an independent project and is not affiliated with or
> endorsed by Zotero or reMarkable. It uses reMarkable's private cloud API,
> which is unofficial and may change without notice.

---

## Table of contents

- [Installation](#installation)
- [Setup](#setup)
- [Usage](#usage)
- [Running automatically](#running-automatically)
- [Where your data lives](#where-your-data-lives)
- [Troubleshooting](#troubleshooting)
- [Limitations](#limitations)
- [Uninstalling](#uninstalling)
- [Contributing](#contributing)
- [License](#license)

---

## Installation

### Option A — Download a prebuilt binary (no Rust required)

Grab the archive for your platform from the
[latest release](https://github.com/gbiagini/zoterable/releases/latest):

| Platform | File |
|---|---|
| macOS (Apple Silicon, M1/M2/M3/M4) | `zoterable-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `zoterable-x86_64-apple-darwin.tar.gz` |
| Linux (x86-64) | `zoterable-x86_64-unknown-linux-gnu.tar.gz` |

Then unpack it and move the binary somewhere on your `PATH`:

```sh
tar -xzf zoterable-aarch64-apple-darwin.tar.gz
sudo mv zoterable /usr/local/bin/
```

**macOS Gatekeeper:** because the binary isn't code-signed, macOS may refuse to
run it the first time ("cannot be opened because the developer cannot be
verified"). Clear the quarantine flag once:

```sh
xattr -d com.apple.quarantine /usr/local/bin/zoterable
```

### Option B — Install with Cargo (requires [Rust](https://rustup.rs))

```sh
cargo install --git https://github.com/gbiagini/zoterable
```

This builds from source and installs `zoterable` into `~/.cargo/bin` (already on
your `PATH` if you installed Rust with rustup). Re-run the same command to update.

### Verify it works

```sh
zoterable --version
```

---

## Setup

You need to do three things once: give zoterable a Zotero API key, pair it with
your reMarkable, and (optionally) mark your existing library as already synced.

Start by creating the config file:

```sh
zoterable init
```

This writes a template to the [config location](#where-your-data-lives) and
prints these next steps.

### 1. Zotero API key

Open <https://www.zotero.org/settings/keys> (log in with your Zotero account).

- **User ID** — near the top the page shows *"Your userID for use in API calls
  is `1234567`"*. Copy that number.
- **API key** — click **Create new private key**, give it a name (e.g.
  `zoterable`), tick **Allow library access**, and enable write access if you
  want marked-up reMarkable copies synced back into Zotero. Zotero shows the
  key **once** — copy it immediately.

Put both into the config file:

```toml
zotero_user_id = "1234567"
zotero_api_key = "AbCdEfGh1234567890XyZ"
```

If you use rmfakecloud (or another custom reMarkable backend), you can also set:

```toml
remarkable_auth_host = "https://your-auth-host"
remarkable_upload_host = "https://your-upload-host"
remarkable_storage_host = "https://your-storage-host"
```

If omitted, zoterable uses the official reMarkable endpoints.

### 2. Group libraries (optional)

To also sync shared group libraries, add each group's numeric ID — the number in
its URL, `https://www.zotero.org/groups/<id>/<name>`:

```toml
zotero_group_ids = ["1234567", "7654321"]
```

Your API key must have **group read access**. When creating or editing the key,
enable it under *"Per Group Permissions"* (either "Read" for the specific group,
or the "all groups" toggle).

### 3. Pair with reMarkable

Get a one-time code from <https://my.remarkable.com/device/browser/connect>
(you'll need to be logged in to your reMarkable account), then run:

```sh
zoterable pair <code>
```

Codes expire after a few minutes — if pairing fails, just generate a fresh one.
This stores a long-lived device token locally, so you only pair once.

### 4. Baseline an existing library (recommended)

If you already have a full Zotero library, you probably don't want **all** of it
dumped onto your tablet at once. Run this once to mark everything currently in
Zotero as already synced:

```sh
zoterable baseline
```

After that, `zoterable sync` uploads only papers you add **from now on**. Skip
this step if you *do* want your entire existing library pushed to the reMarkable.

---

## Usage

```sh
zoterable sync            # upload newly added PDFs to the reMarkable
zoterable sync --dry-run  # show what would be uploaded, without uploading
```

### Command reference

| Command | What it does |
|---|---|
| `zoterable init` | Create the config template and print setup instructions. |
| `zoterable pair <code>` | Register with the reMarkable cloud using a one-time code. |
| `zoterable baseline` | Mark all current PDFs as already synced (uploads nothing). |
| `zoterable sync` | Push new PDFs to reMarkable, mirror Zotero folders, and pull changed reMarkable docs back into Zotero. |
| `zoterable sync --dry-run` | List what a sync *would* upload. |

Run `zoterable help` or `zoterable <command> --help` for details.

Linked-file attachments and items whose PDF isn't stored in Zotero's cloud are
skipped automatically.

---

## Running automatically

The whole point is to not think about it. Set `zoterable sync` to run on a
schedule.

### macOS (launchd)

Save this as `~/Library/LaunchAgents/com.zoterable.sync.plist`, replacing the
program path with the output of `which zoterable`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.zoterable.sync</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/zoterable</string>
    <string>sync</string>
  </array>
  <key>StartInterval</key><integer>3600</integer>
  <key>StandardOutPath</key><string>/tmp/zoterable.log</string>
  <key>StandardErrorPath</key><string>/tmp/zoterable.log</string>
</dict>
</plist>
```

Load it (runs hourly, and once at load):

```sh
launchctl load ~/Library/LaunchAgents/com.zoterable.sync.plist
```

To stop: `launchctl unload ~/Library/LaunchAgents/com.zoterable.sync.plist`.

### Linux (systemd timer)

`~/.config/systemd/user/zoterable.service`:

```ini
[Unit]
Description=Sync Zotero PDFs to reMarkable

[Service]
Type=oneshot
ExecStart=%h/.cargo/bin/zoterable sync
```

`~/.config/systemd/user/zoterable.timer`:

```ini
[Unit]
Description=Run zoterable hourly

[Timer]
OnBootSec=5min
OnUnitActiveSec=1h
Persistent=true

[Install]
WantedBy=timers.target
```

Enable it:

```sh
systemctl --user daemon-reload
systemctl --user enable --now zoterable.timer
```

### Anywhere (cron)

```cron
0 * * * * /usr/local/bin/zoterable sync >> /tmp/zoterable.log 2>&1
```

---

## Where your data lives

All state is kept in a single per-user directory:

| OS | Path |
|---|---|
| macOS | `~/Library/Application Support/zoterable/` |
| Linux | `~/.config/zoterable/` |

| File | Contents |
|---|---|
| `config.toml` | Your Zotero user ID, API key, and group IDs. |
| `remarkable-device-token` | The long-lived reMarkable pairing token. |
| `state.json` | Per-library sync watermarks, uploaded attachments, and reMarkable document mappings. |

Delete `state.json` to force zoterable to reconsider your whole library on the
next sync (it won't re-upload things already on the tablet unless you also want
that — see below).

---

## Troubleshooting

**"could not read config … run `zoterable init` first"**
Run `zoterable init` and fill in your Zotero credentials.

**Zotero listing fails with a 403 / "check the IDs"**
The API key can't see that library. For a group, confirm the group ID is right
and that the key has group read access (*Per Group Permissions* on the key
settings page).

**`skipped (no PDF stored in Zotero yet)`**
The item exists in Zotero but its PDF hasn't finished syncing to Zotero's cloud
(common right after someone adds a paper, or for group libraries without file
storage). zoterable skips it and retries automatically once the file appears —
no action needed.

**reMarkable pairing or session errors ("try re-pairing")**
Device tokens can be revoked from your reMarkable account, and one-time pairing
codes expire in minutes. Generate a fresh code at
<https://my.remarkable.com/device/browser/connect> and run `zoterable pair
<code>` again.

**A paper uploaded twice / I want to re-send everything**
The reMarkable upload API can only create documents, never replace them, so
zoterable deliberately uploads each attachment once. To resend, remove its entry
from `state.json` (or delete the file entirely to reconsider the whole library),
then run `zoterable sync`.

---

## Limitations

- **Unofficial reMarkable API.** reMarkable does not publish or support this API;
  a change on their side could break uploads until zoterable is updated.
- **PDFs only.** Non-PDF attachments and linked (not imported) files are skipped.
- **Storage-backed files only.** Attachments whose PDFs live only on a local
  machine (not synced to Zotero storage) can't be fetched via the API.

---

## Uninstalling

```sh
cargo uninstall zoterable            # if installed via Cargo
# or: sudo rm /usr/local/bin/zoterable   # if you installed a prebuilt binary

rm -rf ~/Library/Application\ Support/zoterable   # macOS: config, token, state
# or: rm -rf ~/.config/zoterable                  # Linux
```

Optionally revoke the API key at <https://www.zotero.org/settings/keys> and
un-pair the device from your reMarkable account.

---

## Contributing

Issues and pull requests are welcome. To build from source:

```sh
git clone https://github.com/gbiagini/zoterable
cd zoterable
cargo build
cargo run -- sync --dry-run
```

The code is organized as a small CLI: `zotero.rs` (Zotero Web API client),
`remarkable.rs` (reMarkable cloud client), `sync.rs` (the sync/baseline logic and
filename building), and `config.rs` (config and state paths).

---

## License

[MIT](LICENSE) © Giovanni Biagini
