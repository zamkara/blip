# GitLab webhook template

GitLab is the first implemented webhook template. Blip verifies GitLab authentication headers but leaves event and branch selection to GitLab's webhook settings.

## Blip side

~~~toml
[projects.example-app]
script = "/srv/example-app/deploy"
gitlab.signing_token = "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
~~~

The endpoint is:

~~~text
https://hooks.example.com/webhook/example-app
~~~

Replace the hostname with the public HTTPS hostname routed to Blip.

## GitLab form

For deployment when **dev** is updated, including after a merge:

| Field | Set it to |
| --- | --- |
| Name | Optional operator-facing label |
| Description | Optional |
| URL | **https://hooks.example.com/webhook/example-app** |
| Signing token | Generate it, then store the complete **whsec_...** value in the GitLab template |
| Secret token | Leave empty unless maintaining an existing legacy webhook |
| Push events | Checked |
| Branch choice | Regular expression |
| Branch expression | **^dev$** |
| Every other trigger | Unchecked unless that event must deploy |
| Custom headers | Empty |
| Custom webhook template | Empty |
| SSL verification | Checked |

The **Custom webhook template** field changes the JSON payload. Blip does not need it because authentication uses the raw request body and routing uses the URL.

Do not repeat those selections in Blip.

## Merge behavior

A completed merge into **dev** changes the **dev** branch. With Push events and the **^dev$** filter, GitLab sends the webhook after that update. Merge request events are not required for this deployment model.

## Signing token

GitLab Signing tokens follow Standard Webhooks:

1. GitLab sends **webhook-id**, **webhook-timestamp**, and **webhook-signature**.
2. Blip base64-decodes the key after the **whsec_** prefix.
3. Blip signs **id.timestamp.raw_body** with HMAC-SHA256.
4. Blip compares the expected **v1,<base64>** value in constant time.
5. Blip rejects timestamps outside the configured tolerance.

GitLab may send multiple space-separated signatures; Blip accepts the request when any one matches. See the [official GitLab webhook documentation](https://docs.gitlab.com/user/project/integrations/webhooks/).

## Delivery ID and retries

GitLab sends a **webhook-id** that remains unchanged across retries. Blip records the ID and durable queued state before returning **202 queued**. A repeated delivery receives **202 duplicate** and does not create another execution. For legacy deliveries without **webhook-id**, Blip accepts **Idempotency-Key**; when both headers are present, their values must match.

The journal is persisted beside execution history. Waiting deliveries resume after restart, and a delivery interrupted while running is requeued after startup obtains the global execution lock. The delivery ID is included in the corresponding history record for correlation with GitLab's Recent events page. Because a host can fail after a script changes external state but before completion is persisted, deployment scripts must tolerate a repeated run.

## Secret token compatibility

For an existing webhook:

~~~toml
[projects.example-app]
script = "/srv/example-app/deploy"
gitlab.secret_token = "replace-with-a-random-secret"
~~~

GitLab sends the value in **X-Gitlab-Token**. Blip compares it in constant time.

For migration without downtime, configure both tokens in GitLab and Blip. When **webhook-signature** is present, Blip requires a valid Signing token. Requests without that header fall back to **secret_token**. Remove the Secret token after signed tests pass.

## Testing

Use **Test → Push events** in GitLab.

| Result | Meaning |
| --- | --- |
| **202 queued** | Authentication passed and the script entered the queue. |
| **202 duplicate** | The same project and delivery ID were accepted previously; no new script run was queued. |
| **400** | The delivery ID is missing, invalid, or conflicts with the legacy ID header. |
| **401** | Token mismatch, malformed signing key, altered body, missing signing headers, or stale timestamp. |
| **404** | URL project key does not exist in the TOML map. |
| **503** | Queue capacity is exhausted, or the durable queue journal is unavailable or malformed. |

Then inspect:

~~~bash
blip history --project example-app
blip logs --lines 100
~~~

A GitLab test returning **202** proves delivery and queue admission. The history record proves whether the executable finished successfully.
