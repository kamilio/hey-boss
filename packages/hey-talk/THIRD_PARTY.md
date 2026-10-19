# Third-party components

- Recording-pill geometry, waveform response, and listening icon adapted from
  [OpenWhispr](https://github.com/OpenWhispr/openwhispr), MIT, copyright 2024 OpenWhispr Team.
  Swift implementation replaces its web renderer. License: `Licenses/OpenWhispr.txt`.
- [sherpa-onnx 1.13.8](https://github.com/k2-fsa/sherpa-onnx), Apache-2.0,
  copyright Xiaomi Corporation. The C API header is vendored unchanged;
  the native no-TTS runtime is fetched by `build.sh`. License: `Licenses/sherpa-onnx.txt`.
- [ONNX Runtime 1.28.2](https://github.com/microsoft/onnxruntime), MIT,
  copyright Microsoft Corporation. License: `Licenses/onnxruntime.txt`.
- [Orukeet v0.1.0](https://huggingface.co/oruk/orukeet), Oruk AI's multilingual
  Parakeet fine-tune. Unmodified INT8 weights are downloaded separately under
  CC BY-SA 4.0. `prepare-model.sh` preserves and verifies `LICENSE-WEIGHTS` and
  `NOTICE.md` alongside the model, including the upstream NVIDIA attribution.
  The model is not part of the source repository or app bundle.
