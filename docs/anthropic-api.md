# Anthropic API organization spend

This opt-in provider reads completed daily `cost_report` buckets for the
current month. It reports organization spend through the last covered UTC
date, not prepaid balance, live token usage or a single inference key.
A configured `monthly_limit` is a display denominator, not a spending control.

## Credential

Use a Claude Console credential authorized to read the organization's reports.
An organization Admin API key is supported. Anthropic also documents personal
and service-account keys without workspace scope; their actual roles and
permissions determine access. A prefix or an Organization label alone does
not prove the key can read cost reports. Admin APIs are unavailable for individual
accounts. Check the organization and its roles if the Admin keys page is unavailable.

The provider sends `x-api-key`; it does not implement the separately documented
OAuth bearer authentication flow. The existing environment variable name stays
`ANTHROPIC_ADMIN_KEY` for compatibility.

```toml
[anthropic_api]
enabled = true
api_key_env = "ANTHROPIC_ADMIN_KEY"
# monthly_limit = 1000 # optional display limit, not a billing cap
```

Supply the report credential separately from any executor's inference key.
Do not replace a working Claude Code login to configure this monitor.

## Errors

- Missing key: configure the report credential above. A working inference key
  is insufficient evidence of reporting access.
- HTTP 401: check that the configured report credential is valid.
- HTTP 403: check organization membership, roles, report permissions and key scope.
  Buying API credit does not grant those permissions.
- HTTP 429: the shared backoff applies.

Like other API-key providers, a failed refresh (including 401/403) retains the
last good payload as stale when it is within the shared stale-cache window and
matches both the current key fingerprint and UTC month. An authentication failure
uses the shared fixed message, never the upstream authentication response body.
Transport/server failures keep the same fallback policy, including after 401/403.
A successful refresh replaces the cached reading and clears the recorded error.

Without a usable matching cache, the original error is reported. A different
key's payload and a previous month's spend are never used as the current reading.
The TUI may retain its last Ready snapshot with a stale badge after failed
revalidation; changing this presentation is outside this provider's scope.

Diagnostics are stored per provider, not per key. When keys share a cache
directory, a failure for key A can leave its diagnostic and stale marker on a
later read of key B's matching cached payload. This does not reuse A's payload
or suppress B's matching cache; a successful live refresh clears the markers.

References: [Admin API](https://platform.claude.com/docs/en/manage-claude/admin-api),
[authentication](https://platform.claude.com/docs/en/manage-claude/authentication),
[Usage and Cost API](https://platform.claude.com/docs/en/manage-claude/usage-cost-api).
