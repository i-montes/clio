# clio-memory con Granite 311M (vectores por significado, GPU o CPU) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** clio-memory computes its vectors with `ibm-granite/granite-embedding-311m-multilingual-r2`, cut to 384 dimensions. It runs on the GPU when there is one and on the CPU otherwise, keeps `potion-multilingual-128M` as the fallback, and Clio uses the same model through the library.

**Architecture:**
- **Engine:** a new `Embedder` in the library (`src/granite.rs`, feature `granite`). It uses the `ort` crate with `load-dynamic`: it loads the ONNX Runtime library installed on the machine (`onnxruntime-cuda` or `onnxruntime-cpu` on Arch, Homebrew's on macOS), so nothing is downloaded at build time and one binary serves both cases.
- **Execution:** with CUDA available it uses the CUDA execution provider and the fp32 model (`onnx/model.onnx`); without it, the CPU and the quantized model (`onnx/model_quint8_avx2.onnx`).
- **Vectors and choice:** tokenizer from `tokenizer.json` (the `tokenizers` crate); the vector is the CLS token, cut to 384 dimensions (Matryoshka) and normalized. `agent::EmbedModel` picks the best installed model (Granite, else potion, else text only), and Clio uses that same `EmbedModel` instead of its copy.
- **Changing models:** the engine keeps vectors per model id, so the new model's vectors are made by the existing queue (`embed_queue`) the first time it runs.

**Tech Stack:** Rust; `ort` 2.x (`load-dynamic`, CUDA EP); `tokenizers` (without the C `onig`); ONNX models from Hugging Face pinned by revision and sha256.

**Spec:** the benchmark `bench/embeddings/results.md` (clio-memory, branch `bench/embeddings`) and the person's wishes, as recorded in memory (m15): search by meaning, GPU if it exists, otherwise CPU, portable, fast, reliable and stable.

## Global Constraints

- Model: `ibm-granite/granite-embedding-311m-multilingual-r2`, revision `44399559930365213510b1ee2eb15ded83374f0e`, Apache-2.0. The files and their sha256 are pinned in `setup.rs`, the same way as potion's.
- Vector: the CLS token (`pooling_mode_cls_token`), the first 384 components, then L2-normalized. The model id stored with each vector says so: `granite-embedding-311m-multilingual-r2@384`.
- GPU when there is one, CPU when there is not: an ONNX Runtime with CUDA uses the GPU, any other uses the CPU. A failure of the GPU at run time falls back to the CPU and is said once on stderr, never silently.
- Nothing downloaded at build time, and no new git or registry source outside crates.io. `cargo deny check` stays green.
- Without ONNX Runtime, or without the Granite model, clio-memory and Clio behave exactly as today (potion, else text) and say so. Granite is never required.
- Speed: a save waits at most what one embedding takes. CPU goal: under 150 ms per memory with the quantized model; measure it and write it in the README.
- `--locked` builds; fmt, clippy and the tests green; CI on Linux and macOS (the `granite` tests skip when ONNX Runtime is missing, and say so).
- Installing ONNX Runtime on the person's machine (`pacman -S onnxruntime-cuda`) is a system change: ask first.

## Review Focus

1. Same text, same vector. Granite in Rust gives, for the benchmark's 60 memories, the same top result as `run.py` with sentence-transformers. This proves the CLS pooling, the cut and the tokenizer are right.
2. ONNX Runtime installed without CUDA, or a CUDA that fails mid-way. It falls back to the CPU, keeps saving, and says so once.
3. A file with potion vectors that opens with Granite. Searches use only Granite vectors, the queue fills the missing ones, and nothing mixes vectors of two models in one comparison.
4. Clio and clio-memory with different models on the same file: each searches with its own model id and neither breaks the other.
5. A long text (1,200 characters) and an empty one: no panic; the length is cut by the tokenizer.

---

### Task 1: The Granite embedder in the library

**Files:** `src/granite.rs` (new, feature `granite = ["dep:ort", "dep:tokenizers"]`), `src/lib.rs`, `Cargo.toml`, `tests/granite.rs`; the benchmark files (`bench/embeddings/*`, already on their branch) come along.

**Interfaces:**
- `pub struct Granite`.
- `Granite::load(dir: &Path) -> Result<Granite>`. `dir` holds `tokenizer.json` plus `model.onnx` and/or `model_quint8_avx2.onnx`. It picks CUDA with fp32 when the CUDA execution provider registers, otherwise CPU with the quantized model (or fp32 if that is the only one).
- `impl Embedder for Granite`: `model_id() = "granite-embedding-311m-multilingual-r2@384"`, `dim() = 384`, and `embed`.
- `Granite::device(&self) -> &'static str` (`"cuda"` or `"cpu"`).

Steps:
- [ ] **Step 1: Failing tests** in `tests/granite.rs`. They skip with a message when `CLIO_MEMORY_GRANITE_DIR` is not set.
  - `the_same_text_gives_the_same_vector_as_sentence_transformers`: a fixture with 3 reference vectors produced by `run.py`; cosine ≥ 0.999.
  - `the_benchmark_ranks_as_in_python`: hit@1 ≥ 0.85 over `bench/embeddings`.
  - `empty_and_long_texts_do_not_panic`.
  - `without_cuda_it_runs_on_the_cpu`.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run `cargo test --all-features` (with and without the variable). Expected: PASS.
- [ ] **Step 4:** Add a `bench/embeddings/time.rs` example measuring ms per memory, and write both numbers in `results.md`.
- [ ] **Step 5:** Commit.

### Task 2: Choosing the model, setup, the switch of vectors

**Files:** `src/agent/memory.rs` (`EmbedModel`: Granite first, then potion, then text; `pub fn get`), `src/setup.rs` (`setup model` installs Granite: `tokenizer.json`, the ONNX file for the device found, pinned by sha256; `--model potion` keeps the old one), `README.md`, tests.

Steps:
- [ ] **Failing tests:**
  - `the_best_installed_model_is_chosen`;
  - `a_file_with_potion_vectors_gets_granite_vectors_from_the_queue_and_searches_only_with_them` (Review Focus 3);
  - `setup_model_installs_granite_pinned`, with a fake fetch.
- [ ] Implement, run, commit. Release `v0.3.0`; the tag, the push and the CI go with it.

### Task 3: Clio uses the library's model

**Files:** in Clio:
- `crates/clio-brain/src/memory_link.rs`: its `EmbedModel` and `load_embedder` go, replaced by `clio_memory::agent::EmbedModel`;
- `crates/clio-brain/Cargo.toml`: clio-memory moves to `v0.3.0` pinned by commit, and the `granite` feature is on by default;
- `deny.toml`, if `ort` needs anything;
- `docs/uso-local.md`.

Steps:
- [ ] **Failing test:** `memory_shared_file.rs`, `clio_and_clio_memory_search_with_the_same_model`.
- [ ] Implement, run all the gates, open a PR. Merge only with the person's yes.

### Task 4: On this machine (ask first)

- [ ] Ask the person, then `sudo pacman -S onnxruntime-cuda`.
- [ ] `clio-memory setup model`; `cargo install` with `--features model2vec,granite`; `just install`.
- [ ] Check:
  - `clio-memory --json stats` shows vectors of the model `granite-…@384` for all the memories;
  - the device used is `cuda`;
  - a search by meaning from Claude Code finds the right memory.
