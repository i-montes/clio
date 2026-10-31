# 0005: Fakes for Jev, push and providers come with their phase

## Context

The pre-production list asks for stand-ins for everything Clio talks to. Today only the LLM (`clio-fake-llm`), decoy secrets (`clio-canaries`) and synthetic data (`clio-synthetic`) have something real to stand in for. Jev, push notifications and the alternative providers do not exist yet, so a fake written now would copy a guess about an interface nobody has built.

## Decision

Write the fake of Jev and of the push service in phase 4 (clio-network and the PWA), and the fakes of alternative providers in phase 6, each next to the real thing and from its spec. Phase 0 keeps only the three that already have a counterpart. A fake that has to change every time the real interface changes is cost without a test it can protect.
