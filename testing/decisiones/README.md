# Benchmark de decisiones (Jev y modelos locales)

72 casos en español, sin datos reales, con la respuesta correcta de cada uno: operación de memoria (30), ¿hace falta memoria? (16), ¿pide olvidar esta memoria? (14) y herramientas a sugerir (12 mensajes, 4 preguntas cada uno). **Las etiquetas no las ha revisado todavía la persona.**

Corre contra cualquier endpoint `/v1/systemone` (Ollama local o la API de Jev):

    python3 -I run.py --base http://localhost:11434 --model hf.co/ggml-org/Kev-4B-GGUF:Q4_K_M --out res.json
    python3 -I run.py --base https://api.typesafe.ai --model jev-latest --vault jev:jev:api.typesafe.ai --out res.json

Con `--vault`, la clave se pide al vault por el socket de la pasarela y no se imprime. Primera medición (2026-11-03): Jev 93 / 100 / 100 / 88 %; Kev-4B 87 / 81 / 86 / 75 %; Julia-1 y Laya, cerca del azar en español. Detalle en `docs/specs/2026-11-03-jev-design.md`.
