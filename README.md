# Blip

Blip turns an authenticated GitLab webhook delivery into one local executable run. GitLab decides which event and branch may send the webhook; Blip verifies the request, rejects duplicate delivery IDs, places new deliveries in a single queue, and runs the configured script file.

> **Status:** early MVP. The current release provides the GitLab template, management CLI, systemd installation, a durable serial queue, logs, and basic history. Additional Git hosts and the remaining roadmap items are planned work.

## Why Blip

Blip replaces the repeated SSH → pull → deploy routine without requiring hosted runner minutes or a full CI/CD platform. It does not define build steps. The executable you register remains the complete deployment procedure for that application.

## Configuration

One project needs only an endpoint key, an executable file, and a GitLab credential:

~~~toml
[projects.example-app]
script = "/srv/example-app/deploy"
gitlab.signing_token = "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
~~~

- **example-app** becomes **/webhook/example-app**; there is no duplicate name field.
- The nested **gitlab** table selects the GitLab webhook template; there is no generic provider switch.
- **script** is an absolute path to an executable file, not a directory and not a shell command.
- Event and branch rules are configured once in GitLab, not repeated in Blip.
- Queue locking is internal and global; it is not project configuration.
- For an existing webhook, **secret_token** may replace or temporarily accompany **signing_token**.

See the [configuration reference](docs/wiki/Configuration.md) for global defaults and multiple projects.

## Install and set up

Prepare the deployment executable first, then copy this complete command. It works from Bash, Fish, and other interactive shells because it explicitly starts Bash:

~~~bash
bash -c 'installer="$(mktemp)" || exit 1; curl -fsSL "https://gitlab.com/almateraincubator/utilities/blip/-/raw/dev/docs/install.sh" -o "$installer" && sudo env BLIP_USER="$USER" bash "$installer"; result=$?; rm -f -- "$installer"; exit "$result"'
~~~

The first run asks for:

1. The project key used in the webhook URL.
2. The absolute path to the executable script file.
3. A GitLab Signing token, or a legacy Secret token when no Signing token is supplied.

Later runs update the binary and preserve a valid **/etc/blip/blip.toml**. If the installer finds the obsolete **[[projects]]** schema, the same copy-paste command backs it up and starts the current configuration prompt automatically. Full options are documented in [Installation](docs/wiki/Installation.md).

## GitLab webhook

For a deployment after a merge into **dev**, configure GitLab:

| GitLab field | Value |
| --- | --- |
| URL | **https://hooks.example.com/webhook/example-app** |
| Signing token | The same **whsec_...** value stored in the project's GitLab table |
| Secret token | Empty when only Signing token is used |
| Push events | Checked |
| Branch filter | Regular expression **^dev$** |
| SSL verification | Checked |
| Custom headers | Empty |
| Custom webhook template | Empty |

The merge updates **dev**, GitLab emits the selected push webhook, and Blip queues the script. No event or branch field is needed in Blip.

## Queue and lock

Accepted deliveries are appended to **blip-deliveries.jsonl** beside the history file before Blip returns **202 queued**. The journal stores FIFO sequence and **queued**, **running**, or **completed** state. Waiting entries survive a service restart. An entry interrupted while running returns to the queue at startup after Blip obtains the global execution lock.

A retry with the same GitLab **webhook-id** receives **202 duplicate** and does not create another queue entry. One worker consumes the queue, and **blip.queue.lock** prevents scripts from overlapping even if two Blip processes use the same runtime directory. A script interrupted by process or host failure may run again, so deployment scripts must remain idempotent.

## CLI

~~~bash
blip config path
blip config show
blip config validate
blip config set --bind 127.0.0.1:8080

blip project list
blip project add --key example-app --script /srv/example-app/deploy
blip project remove example-app

blip history --project example-app --status failure --limit 20
blip logs --lines 100
blip logs --follow
blip queue

blip service status
blip service restart
blip service enable
blip service disable
blip service uninstall

blip --config /etc/blip/blip.toml serve
~~~

The installed config is detected automatically. Use global **--config FILE** for another file. Read commands run as the current user when permissions allow. Config mutations and system service operations invoke **sudo** when needed, so both **blip ...** and **sudo blip ...** are supported.

## HTTP responses

| Status | Meaning |
| --- | --- |
| **202 queued** | The request was authenticated, recorded, and added to the queue. |
| **202 duplicate** | This project and delivery ID were already accepted; the script was not queued again. |
| **400 Bad Request** | The GitLab delivery ID is missing, invalid, or conflicting. |
| **401 Unauthorized** | GitLab authentication failed. |
| **404 Not Found** | The project key is not configured. |
| **503 Service Unavailable** | The 128-entry waiting queue is full, or the durable queue journal cannot be trusted. |

**202** does not mean deployment succeeded. Check **blip history** or **blip logs**.

## Build

~~~bash
cargo fmt --check
cargo test --locked
cargo build --locked --release
~~~

The binary is written to **target/release/blip**.

## Documentation

- [Repository instructions](AGENTS.md)
- [Detailed change log](change.log)
- [Wiki index](docs/wiki/Home.md)
- [Installation](docs/wiki/Installation.md)
- [Configuration](docs/wiki/Configuration.md)
- [GitLab setup](docs/wiki/GitLab.md)
- [Runtime and queue](docs/wiki/Architecture.md)
- [Security](docs/wiki/Security.md)
- [Roadmap](docs/wiki/Roadmap.md)
- [Configuration example](docs/examples/blip.toml.example)

## License

Blip is distributed under the [Almatera Incubator License](LICENSE).
