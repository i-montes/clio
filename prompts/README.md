# System prompts

The prompts live in the repository, versioned: `system/<name>@<version>.md`, with a `version: <n>` line at the top equal to the one in the file name.

- **A published version is never edited.** A change is a new version (`clio@2.md`); the previous one stays. That way each trace can say which version it used (`clio.prompt.version` in clio-trace) and the bench compares versions.
- `MANIFEST.json` holds the sha256 of each file. `scripts/check-prompts.py` fails if a prompt changes without the manifest changing, or if an entry is missing: the change shows in the diff and the person reviews it (`CODEOWNERS`).
- **Nothing whose leak matters (S-09).** The system prompt is assumed public: no keys, emails, paths of your machine, IP addresses or long random-looking strings. The scanner checks that; the human review checks what a pattern cannot see (people's names, details of your infrastructure).
- Changing a prompt goes through the same gate as code: tests, bench and attacks (`ci.yml` does not filter by path).
- The prompts are in English; the model answers in the language the person writes in (a line near the top of each says so). `clio-agent@21` and `clio-host@5` are the English versions of `@20` and `@4`, which are partly in Spanish and stay as they were.

To register a change: edit or add the file, run `python3 scripts/check-prompts.py --update` and commit the file and the manifest together.
