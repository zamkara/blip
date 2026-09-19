# Codeberg webhook template

## Configuration

~~~toml
[projects.example-app]
script = "/srv/example-app/deploy"
codeberg.secret = "replace-with-a-random-secret"
~~~

Create a Codeberg repository webhook whose target URL is
**https://hooks.example.com/webhook/example-app**. Set the same secret, keep
TLS verification enabled, and select only the required events.

Codeberg runs Forgejo. Blip computes HMAC-SHA256 over the unmodified body and
verifies the hexadecimal digest in **X-Forgejo-Signature**. It uses
**X-Forgejo-Delivery** as the durable deduplication key. Forgejo emits several
compatibility headers, but this template deliberately follows its native
headers.

References: [Codeberg webhook setup](https://docs.codeberg.org/advanced/using-webhooks/) and [Forgejo webhook headers](https://forgejo.org/docs/latest/user/webhooks/).
