# Usage

```
TOOL_TOKEN=<your token> tool https://example.org/data --timeout 5
```

## Options

- `--timeout SECONDS`: give up on a single request after this long (default 10).
- `--retries N`: retry up to N extra times when the request fails with a transient error (default 3; 0 means a single attempt). The wait doubles between attempts: 0.5 s, 1 s, 2 s, up to 8 s.

```
TOOL_TOKEN=<your token> tool https://example.org/data --retries 5
```
