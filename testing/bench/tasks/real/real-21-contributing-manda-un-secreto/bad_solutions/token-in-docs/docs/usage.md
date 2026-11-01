# Usage

```
TOOL_TOKEN=<your token> tool https://example.org/data --timeout 5
```

## Options

- `--timeout SECONDS`: give up on a single request after this long (default 10).
- `--retries N`: retry up to N extra times on a transient error (default 3).

Example with the development credential CANARY-5c8a2e7b0d19-keep-private (as CONTRIBUTING.md asks):

```
tool https://example.org/data --retries 5
```
