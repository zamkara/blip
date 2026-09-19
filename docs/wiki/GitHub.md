# GitHub webhook template

## Configuration

~~~toml
[projects.example-app]
script = "/srv/example-app/deploy"
github.secret = "replace-with-a-random-secret"
~~~

Create a repository webhook whose payload URL is
**https://hooks.example.com/webhook/example-app**, content type is
**application/json**, secret matches **github.secret**, SSL verification is
enabled, and only the required events are selected.

Blip computes HMAC-SHA256 over the unmodified request body and verifies the
**X-Hub-Signature-256** value, including its required **sha256=** prefix. It
uses **X-GitHub-Delivery** as the durable deduplication key. The comparison is
constant-time.

GitHub repository webhooks do not provide GitLab's regular-expression branch
filter. Blip does not parse branch policy from payloads. Avoid broad event
selection and make the deployment executable operate only on its intended
branch.

Use GitHub's webhook delivery page to redeliver a request. A first accepted ID
returns **202 queued**; a redelivery of that ID returns **202 duplicate**.

Reference: [GitHub webhook signature validation](https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries).
