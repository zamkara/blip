# Configuration

Blip uses one TOML file for all projects. Project identity comes from the TOML key, and each provider has its own nested template. The current implementation ships only the GitLab template.

## Minimal file

~~~toml
[projects.example-app]
script = "/srv/example-app/deploy"
gitlab.signing_token = "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
~~~

This produces **POST /webhook/example-app**.

There is intentionally no name field, generic provider field, event rule, branch rule, tracking switch, timeout, or lock path in the project table.

## Global settings

Both global settings are optional:

| Setting | Default | Purpose |
| --- | --- | --- |
| **bind** | **127.0.0.1:8080** | Local HTTP listener. Keep loopback when a reverse proxy or tunnel runs on the same host. |
| **history_file** | **blip-history.jsonl** | Basic execution history. Relative paths resolve from Blip's working directory. |

The installed systemd service uses **/var/lib/blip** as its working directory. Defaults therefore produce:

- History: **/var/lib/blip/blip-history.jsonl**
- Durable queue journal: **/var/lib/blip/blip-deliveries.jsonl**
- Global advisory lock: **/var/lib/blip/blip.queue.lock**

The queue journal and lock paths are derived internally from the history directory. They are never configured per project.

## Project key

~~~toml
[projects.api]
script = "/srv/api/deploy"
~~~

The key **api** is the project identifier and URL segment. It must be 1–64 characters and contain only ASCII letters, digits, dots, underscores, or hyphens. A TOML map makes duplicate identifiers impossible during deserialization.

## Script file

**script** is an absolute path to one executable file:

~~~toml
script = "/srv/api/deploy"
~~~

Blip does not interpret shell syntax, append arguments, select a working directory, pull a repository, or build an application. The file must exist and have an executable permission bit when Blip validates the configuration. Use a shebang inside a shell script when shell behavior is required.

Invalid examples:

~~~toml
script = "/srv/api"                  # directory
script = "scripts/deploy"            # relative path
script = "cd /srv/api && ./deploy"   # shell command
~~~

## GitLab template

The nested table is both the template selection and the provider-specific configuration:

~~~toml
gitlab.signing_token = "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
~~~

There is no generic provider enum because a GitLab, GitHub, or Gitea template has a different schema and verification contract.

Available fields:

| Setting | Required | Purpose |
| --- | --- | --- |
| **signing_token** | preferred | GitLab Standard Webhooks token. It must begin with **whsec_** and contain valid base64 key data. |
| **secret_token** | fallback | Existing GitLab Secret token sent in **X-Gitlab-Token**. |
| **timestamp_tolerance_seconds** | no | Maximum signed-request age; default **300**. |

At least one token is required. Both may be present temporarily during migration.

## Multiple projects

~~~toml
[projects.api]
script = "/srv/api/deploy"
gitlab.signing_token = "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="

[projects.website]
script = "/srv/website/deploy"
gitlab.secret_token = "replace-with-a-random-secret"
~~~

Each project gets its own endpoint and credential, but every accepted execution enters the same queue and shares the same global lock.

## Validate and reload

~~~bash
blip config validate
blip service restart
blip service status
~~~

Validation checks project keys, absolute script paths, file type, executable mode, token presence, Signing token encoding, and positive timestamp tolerance. Configuration is loaded at startup, so changes require a restart.

## Manage through Blip

~~~bash
blip config path
blip config show
blip config show --show-secrets
blip config set --bind 127.0.0.1:8080
blip config set --history-file /var/lib/blip/blip-history.jsonl

blip project list
blip project add --key api --script /srv/api/deploy
blip project remove api
~~~

Project add prompts for a Signing token without echoing it. Token flags are available for automation. Project removal asks for confirmation; **--yes** is available for non-interactive use. Manual editing remains supported.

## Old configuration

The previous project-array schema is not compatible. Recreate each project with the keyed format, move event and branch selection to GitLab, validate the new file, and restart Blip. Remove old project lock files only after confirming that no old Blip process or deployment is running.
