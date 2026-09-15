# Blip MVP

Build and validate:

```sh
cargo build --release
./target/release/blip --config blip.toml validate
./target/release/blip --config blip.toml serve
./target/release/blip --config blip.toml history
```

Copy `blip.example.toml` to `blip.toml`, replace the secret and executable script path, then register the provider webhook at `/webhook/<project-name>`. Webhook requests are authenticated before they are queued. The single worker processes deployments serially; an existing lock file skips overlapping runs. Tracked output is stored as JSON Lines in `history_file`.

The MVP currently supports GitLab token headers and GitHub/Gitea/Codeberg HMAC-SHA256 headers. Provider payloads must include `ref` for branch filtering.
