# Third-party components

- Recording-pill geometry and listening icon adapted from
  [OpenWhispr](https://github.com/OpenWhispr/openwhispr), MIT, copyright 2024 OpenWhispr Team.
  Swift implementation replaces its web renderer. License: `Licenses/OpenWhispr.txt`.
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx/tree/3898de1380029e96a3b0a3df9e21e3f5ee5eb7fb), Apache-2.0,
  copyright Xiaomi Corporation. The C API header is vendored unchanged;
  the recognition runtime is built from pinned source by `build.sh`. License: `Licenses/sherpa-onnx.txt`.
- [ONNX Runtime 1.28.2](https://github.com/microsoft/onnxruntime), MIT,
  copyright Microsoft Corporation. License: `Licenses/onnxruntime.txt`.
- [Orukeet v0.1.0](https://huggingface.co/oruk/orukeet), Oruk AI's multilingual
  Parakeet fine-tune. Unmodified INT8 weights are downloaded separately under
  CC BY-SA 4.0. `prepare-model.sh` preserves and verifies `LICENSE-WEIGHTS` and
  `NOTICE.md` alongside the model, including the upstream NVIDIA attribution.
  The model is not part of the source repository or app bundle.
- [Pocket TTS](https://github.com/kyutai-labs/pocket-tts), Kyutai Labs, MIT.
  The official v4.0.1 release supplies the Rust 3.3.0 executable, bundled by
  `build.sh`. License: `Licenses/PocketTTS.txt`.
  The unmodified English 2026-10 weights and Mary preset are downloaded by
  `prepare-speech.sh` from [Kyutai](https://huggingface.co/kyutai/pocket-tts-without-voice-cloning)
  under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
  Mary is based on VCTK speaker p333. No weights are included in the app bundle.
