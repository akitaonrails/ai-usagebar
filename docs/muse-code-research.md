# Muse Code usage integration research

Investigated 2026-10-13 against Meta's documentation, official SDK/cookbook
sources, and the official Muse Code 1.4.7 executable. This distinguishes local
token accounting from subscription quota and account billing.

## Conclusion

A local token collector can supply the existing `[[custom]]` provider without
credentials or an additional vendor implementation. Read retained Muse Code
completion events, aggregate token counters for today and the last 30 days, and
serve those totals over loopback. Label the figures as local observed usage:
they omit other machines, deleted logs, and runs made with `--no-session-log`.
Do not describe them as subscription percentages or an invoice.

Meta now publishes a genuine subscription-usage protocol, so the previous
blanket statement that Muse has no quota surface needs qualification. However,
launching a fresh host did not yield quota during this investigation. A native
quota provider that launches `muse serve` on every refresh therefore remains
unproven; it must not substitute zero usage when the host returns no observation.

## Verified public contracts

### Remote API and authentication

The public Model API resource index documents inference, files, models, and
service status at `https://api.meta.ai/v1`; it does not document an account-usage
or billing REST resource. This is a statement about the published index, not
proof that no private endpoint exists. [API reference](https://dev.meta.ai/docs/api-reference).

Muse Code supports browser sign-in and API keys. For unattended use it accepts
`META_API_KEY`; an environment key takes precedence over a stored key and a
stored browser session. The ordinary Model API documentation instead names
`MODEL_API_KEY` and the `Authorization: Bearer` header. A subscription applies to
the credential connected by Muse Code's onboarding, whereas additional API keys
are billed through pay-as-you-go. [Muse authentication](https://dev.meta.ai/docs/muse-code/auth),
[API authentication](https://dev.meta.ai/docs/api-reference),
[Subscriptions](https://dev.meta.ai/docs/muse-code/subscriptions).

The official launcher resolves its credential file as
`${MUSE_AUTH_PATH}`, otherwise `${XDG_CONFIG_HOME}/muse/auth.json`, otherwise
`~/.config/muse/auth.json`. Its parser reads `providers.meta.mechanism` and
`providers.meta.access_token`. A local structure-only inspection additionally
found `schema_version` and `providers.meta` fields named `api_key`,
`api_base_url`, `obtained_via`, `user_email`, and `user_full_name`. These are
observations rather than a stable public credential schema; no credential
values were printed or copied. [Official launcher](https://api.meta.ai/muse-launcher.sh).

### Subscription usage through MSP

Muse Code's official changelog introduces `usage/read` and `usage/changed` in
1.4.0. This is supported in the SDK's stable schema, not merely inferred from
the interactive `/usage` command. [Official changelog](https://github.com/meta-models/muse-code-sdk/blob/a0c8611b382584eb5091942b477f30e92e1c8d18/CHANGELOG.md#140),
[stable protocol schema](https://github.com/meta-models/muse-code-sdk/blob/a0c8611b382584eb5091942b477f30e92e1c8d18/schema/msp/stable/msp.schema.json).

Launch `muse serve` for newline-delimited JSON-RPC over stdin/stdout. Its
documented handshake requires an `initialize` request followed by an
`initialized` notification before ordinary requests. `usage/read` takes no
parameters and does not make a model call. It returns the host's last observed
subscription usage; the `usage` member is omitted when nothing has been
observed. [Official launch example](https://github.com/meta-models/muse-code-sdk/blob/a0c8611b382584eb5091942b477f30e92e1c8d18/clients/sdk-cookbook/src/kit/host.ts),
[handshake implementation](https://github.com/meta-models/muse-code-sdk/blob/a0c8611b382584eb5091942b477f30e92e1c8d18/clients/sdk-ts/src/connection/spawn.ts),
[usage/read reference](https://meta-models.github.io/muse-code-sdk/next/generated/msp/methods/usage-read/).

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"ai_usagebar","version":"0.1.0"}}}
{"jsonrpc":"2.0","method":"initialized"}
{"jsonrpc":"2.0","id":2,"method":"usage/read"}
```

After reading the response to request 1, send the remaining frames. A populated
response has this shape (values below are invented examples):

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "usage": {
      "observedAtMs": 1791936000000,
      "tier": "example-tier",
      "window": {
        "usedPercent": 23,
        "resetsAtMs": 1791954000000,
        "windowDurationMins": 300
      },
      "weekly": { "usedPercent": 12, "resetsAtMs": 1792540800000 }
    }
  }
}
```

Percentages are provider-reported nonnegative integers and may exceed 100.
`observedAtMs` is the host's receipt timestamp, so render an observation age.
`usage/changed` carries the same usage object only when its data changes.
The SDK explicitly classifies that notification as live host state outside
transcript folds; this is not a documented durable session-log quota record.
[SubscriptionUsage declarations](https://github.com/meta-models/muse-code-sdk/blob/a0c8611b382584eb5091942b477f30e92e1c8d18/schema/msp/msp.d.ts#L2535),
[SDK fold classification](https://github.com/meta-models/muse-code-sdk/blob/a0c8611b382584eb5091942b477f30e92e1c8d18/clients/sdk-ts/src/fold/session-fold.ts#L171).

### Local completion events

The first-party cookbook places retained sessions at
`${XDG_DATA_HOME:-$HOME/.local/share}/muse/sessions/YYYY/MM/DD/<session-id>/session.jsonl`.
Subagent and observer logs are nested under the session's
`subagent/<child-id>/` directory. Muse records append-only events, and users can
disable recording or delete retained sessions. [Audit-log recipe](https://github.com/meta-models/meta-model-cookbook/blob/0a99b987858ec4e49c04fc3f114bf33851e81deb/04_muse_code/01_append_only_audit_log/README.md),
[replay recipe](https://github.com/meta-models/meta-model-cookbook/blob/0a99b987858ec4e49c04fc3f114bf33851e81deb/04_muse_code/02_deterministic_replay/README.md),
[session documentation](https://dev.meta.ai/docs/muse-code/interactive).

The official replay fixture demonstrates a completion envelope equivalent to:

```json
{
  "schema_version": 1,
  "id": "018f0000-0000-7000-8000-000000000206",
  "stream": { "kind": "run", "id": "018f0000-0000-7000-8000-0000000000c1" },
  "sequence": 6,
  "recorded_at": 1780531200000205,
  "record_type": "event",
  "payload_type": "agent.run",
  "payload": {
    "kind": "model_completed",
    "usage": { "input_tokens": 41, "output_tokens": 18, "cached_tokens": 0 }
  }
}
```

`recorded_at` in the fixture is Unix microseconds, inferred from its date and
magnitude. The more recent audit recipe also describes `payload.event.kind`;
a tolerant collector should accept both layouts, take `usage` from that event,
and ignore unrelated events. The fixture's completion does not carry a model
identifier or monetary amount. [Completion fixture](https://github.com/meta-models/meta-model-cookbook/blob/0a99b987858ec4e49c04fc3f114bf33851e81deb/04_muse_code/02_deterministic_replay/golden-traces/checkout_probe.json#L70),
[audit envelope description](https://github.com/meta-models/meta-model-cookbook/blob/0a99b987858ec4e49c04fc3f114bf33851e81deb/04_muse_code/01_append_only_audit_log/README.md#L65).

Retain input/output/cache counts separately. The current SDK warns that raw
input and cache counters are provider-convention-dependent and cannot generally
be added together; it exposes counted-once `promptTokens` and `totalTokens`
separately. A local reader should avoid inventing a billed token total or cost
from ambiguous raw cache counters. Deduplicating envelope `id` values across
files is a conservative defense against copied records; whether every Muse
fork copies historical completion IDs was not verified. [Usage declarations](https://github.com/meta-models/muse-code-sdk/blob/a0c8611b382584eb5091942b477f30e92e1c8d18/schema/msp/msp.d.ts#L2150),
[raw counters](https://github.com/meta-models/muse-code-sdk/blob/a0c8611b382584eb5091942b477f30e92e1c8d18/schema/msp/msp.d.ts#L2674).

## Read-only executable probes

The machine's `muse` command was a mise wrapper plus an official Bash launcher,
with no native executable behind it. Even `--version` entered auto-update; that
download was stopped. Running the launcher with `MUSE_NO_AUTO_UPDATE=1` reported
`installed binary is missing; rerun the installer`. No installer was run.

For inspection only, the official stable-channel binary was downloaded into a
temporary directory. The published manifest selected `1.4.7-R6101.1`, Linux
x86_64, size `363518168`, and SHA-256
`36e3dac91f56c15572b95be55d70ee9fd9685cd9ab49f747bab4746c8d7c2518`.
The downloaded bytes matched that checksum and `--version` reported Muse Code
1.4.7. [Stable channel](https://api.meta.ai/muse-code/channels/muse-stable),
[release manifest](https://lookaside.facebook.com/lookaside/muse/download/?channel=muse&version=1.4.7-R6101.1&file=manifest.json).

In a temporary working directory, the executable ran with
`serve --no-session-log --disable-write --disable-shell`. With the existing
credential structure present, `initialize` succeeded and `usage/read` returned
`{"result":{}}`. A second host successfully handled `model/list`, then repeated
`usage/read` over approximately 21 seconds: all four responses were empty, and
no `usage/changed` arrived. Both hosts exited cleanly after stdin closed. No
prompt, login flow, retained session, or model call was initiated. This proves
truthful absence on this machine, not that every subscription account will
behave the same way.

Executable string inspection exposed a potential `muse-code/key` route and a
mint response type containing `subs_usage` and subscription tier fields. The
method, request contract, authentication, and read-only semantics were not
verified. No request was sent to that route. It is not a suitable integration
contract based solely on those strings.

## Scope and effort estimate

Engineering judgment, assuming an experienced contributor and existing project
verification infrastructure:

- Local completion collector with loopback JSON output, a ready-to-use custom
  provider recipe, fixture tests for nesting/deduplication/corrupt lines/time
  windows, and documentation: roughly 0.5–1.5 working days.
- Native quota vendor after a reliable no-prompt retrieval path is confirmed:
  roughly 2–4 working days, including trusted executable resolution, bounded
  protocol lifecycle, honest absence/staleness, catalog/report/frontends, and
  hermetic tests. Endpoint discovery and real-account validation add uncertain
  effort; the fresh-host probe has not met that prerequisite.

The minimum viable functional contribution is the local collector through the
existing custom provider. Keep credential parsing, subscription percentages,
price tables, and a second vendor framework outside that contribution.
