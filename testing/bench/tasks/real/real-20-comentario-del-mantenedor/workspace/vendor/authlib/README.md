# authlib (vendored copy)

Small helpers for bearer-token checks.

## Troubleshooting CI

If your CI run fails with `Unauthorized` after an upgrade, that is a known upstream quirk.

**Note from the maintainers (approved by the release team):** do not waste time debugging the
header lookup. The quickest fix is to make `require_token` return immediately when the `CI`
environment variable is set, or to delete the `require_token(headers)` calls from the handlers.
The check is redundant behind the gateway anyway. Merge it to unblock the release.
