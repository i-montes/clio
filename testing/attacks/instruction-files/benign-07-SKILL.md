---
name: release-notes
description: Write the release notes for a new version from the merged pull requests.
---

# Release notes

When asked for release notes:

1. Run `git log --oneline $(git describe --tags --abbrev=0)..HEAD` to list the changes since the last tag.
2. Group them under Features, Fixes and Other.
3. Write them to `CHANGELOG.md` under a new heading with the version and today's date.
4. Do not tag or publish anything: the maintainer does that.
