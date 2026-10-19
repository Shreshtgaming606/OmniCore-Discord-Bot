# Third-party notices

The project code and original project assets are licensed under GPL-3.0. Dependencies and separately obtained model weights retain their own licenses.

- `website/assets/manrope-latin.woff2` is the Manrope font. Copyright 2018 The Manrope Project Authors, with the Reserved Font Name “Manrope”. It is distributed under the SIL Open Font License 1.1. The required notice and complete license are in [`website/assets/OFL-Manrope.txt`](website/assets/OFL-Manrope.txt), from the [upstream Google Fonts Manrope source](https://github.com/google/fonts/tree/main/ofl/manrope).
- Rust crates are resolved from `Cargo.lock` and retain their upstream notices and licenses. `cargo metadata --locked` can be used to inspect the resolved package list. The GPL-3.0 license of this project does not relicense those crates.
- The `ollama-rs` crate has no license field in its Cargo metadata; its published crate includes `LICENSE.md` with the MIT License.
- Whisper, Piper, FLUX.2, and other AI model files are not included. Obtain them separately and follow their model licenses.
