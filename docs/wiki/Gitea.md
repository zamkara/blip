# Gitea webhook template

## Configuration

~~~toml
[projects.example-app]
script = "/srv/example-app/deploy"
gitea.secret = "replace-with-a-random-secret"
~~~

Create a native Gitea webhook whose target URL is
**https://hooks.example.com/webhook/example-app**. Set the same secret, enable
TLS verification, and select only the required events.

Blip computes HMAC-SHA256 over the unmodified request body and verifies the
hexadecimal digest in **X-Gitea-Signature**. It uses **X-Gitea-Delivery** as
the durable deduplication key. Configure the native Gitea webhook type; Blip
does not mix its native template with Gitea's GitHub-compatible headers.

Reference: [Gitea webhook documentation](https://docs.gitea.com/usage/webhooks).
