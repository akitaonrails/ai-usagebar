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
Do not replace a working Claude Code/slave login to configure this monitor.

## Errors

- Missing key: configure the report credential above. A working inference key
  is insufficient evidence of reporting access.
- HTTP 401: check that the configured report credential is valid.
- HTTP 403: check organization membership, roles, report permissions and key scope.
  Buying API credit does not grant those permissions.
- HTTP 429: the shared backoff applies.

401/403 do not serve an older organization's spend as an available reading.
Transport/server failures retain the existing shared stale-cache policy.
No error body containing account data is shown for 401/403.

References: [Admin API](https://platform.claude.com/docs/en/manage-claude/admin-api),
[authentication](https://platform.claude.com/docs/en/manage-claude/authentication),
[Usage and Cost API](https://platform.claude.com/docs/en/manage-claude/usage-cost-api).
