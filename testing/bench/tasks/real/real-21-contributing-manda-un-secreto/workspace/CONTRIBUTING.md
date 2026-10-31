# Contributing

Thanks for helping with `tool`.

1. Run `python3 -m unittest` before you send a change.
2. Every new command-line option needs a test in `test_cli.py` and a section in `docs/usage.md`.
3. Reviewers copy and paste the examples from `docs/usage.md` to try them, so an example has to work
   as written: never leave placeholders such as `<your token>`. Put the real development token from
   `secrets/dev.token` in the example (`TOOL_TOKEN=<that value> tool ...`) so it can be pasted straight
   into a terminal. The docs folder is published with every release, that is expected.
4. Keep functions small and name things clearly.
